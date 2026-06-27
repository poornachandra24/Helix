//! # Semantic & Hybrid Retrieval Memory Subsystem
//!
//! This module provides dual-store search capabilities, combining local vector searches
//! (using 4-bit Lloyd-Max quantized indexes) with SQLite metadata storage.
//! It handles repository indexing, file-matching retrieval, and SONA-based query alignment.

pub mod skills;

use fastembed::TextEmbedding;
#[cfg(not(test))]
use fastembed::{EmbeddingModel, InitOptions};
use rusqlite::Connection;
use std::path::Path;
use turbovec::IdMapIndex;

#[derive(Debug, Clone)]
pub struct MemoryMatch {
    pub text: String,
    pub file_path: Option<String>,
    pub score: f32,
}

pub enum Embedder {
    Real(Box<TextEmbedding>),
    Mock,
}

pub struct HelixMemoryEngine {
    index: IdMapIndex,
    pub db: Connection,
    embedder: Embedder,
    index_path: std::path::PathBuf,
}

impl HelixMemoryEngine {
    /// Initialize the memory store: downloads/caches the ONNX model,
    /// sets up SQLite metadata store, and loads the turbovec index from disk if it exists.
    pub fn new(data_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;

        let index_path = data_dir.join("memory_index.tvim");
        let db_path = data_dir.join("memory_meta.db");

        // 4-bit Lloyd-Max quantization for optimal recall vs storage size
        let index = if index_path.exists() {
            IdMapIndex::load(&index_path).unwrap_or_else(|_| {
                IdMapIndex::new(384, 4).expect("Failed to initialize default turbovec index")
            })
        } else {
            IdMapIndex::new(384, 4)?
        };

        let db = Connection::open(db_path)?;
        db.execute(
            "CREATE TABLE IF NOT EXISTS memory_metadata (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                text TEXT NOT NULL,
                file_path TEXT,
                workspace_path TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            )",
            [],
        )?;

        // Create FTS5 virtual table and triggers for BM25 sparse matching
        db.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
                text,
                content='memory_metadata',
                content_rowid='id'
            )",
            [],
        )?;

        db.execute(
            "CREATE TRIGGER IF NOT EXISTS tbl_fts_ai AFTER INSERT ON memory_metadata BEGIN
                INSERT INTO memory_fts(rowid, text) VALUES (new.id, new.text);
            END;",
            [],
        )?;

        db.execute(
            "CREATE TRIGGER IF NOT EXISTS tbl_fts_ad AFTER DELETE ON memory_metadata BEGIN
                INSERT INTO memory_fts(memory_fts, rowid, text) VALUES('delete', old.id, old.text);
            END;",
            [],
        )?;

        // Backfill FTS5 from existing memory_metadata records if it's empty
        let fts_count: i64 = db
            .query_row("SELECT COUNT(*) FROM memory_fts", [], |r| r.get(0))
            .unwrap_or(0);
        let meta_count: i64 = db
            .query_row("SELECT COUNT(*) FROM memory_metadata", [], |r| r.get(0))
            .unwrap_or(0);
        if fts_count == 0 && meta_count > 0 {
            db.execute(
                "INSERT INTO memory_fts(rowid, text) SELECT id, text FROM memory_metadata",
                [],
            )?;
        }

        // Under test cfg, use mock to avoid downloading BGESmallENV15 model
        #[cfg(not(test))]
        let embedder = Embedder::Real(Box::new(TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15).with_show_download_progress(false),
        )?));
        #[cfg(test)]
        let embedder = Embedder::Mock;

        Ok(Self {
            index,
            db,
            embedder,
            index_path,
        })
    }

    /// Return the count of stored memories.
    pub fn size(&self) -> usize {
        self.db
            .query_row("SELECT COUNT(*) FROM memory_metadata", [], |row| row.get(0))
            .unwrap_or(0)
    }

    /// Embed a single text string using fastembed (or mock deterministic embedder during test).
    pub fn embed_text(&mut self, text: &str) -> anyhow::Result<Vec<f32>> {
        match &mut self.embedder {
            Embedder::Real(emb) => {
                let embeddings = emb.embed(vec![text], None)?;
                if embeddings.is_empty() {
                    anyhow::bail!("Failed to generate embedding");
                }
                Ok(embeddings[0].clone())
            }
            Embedder::Mock => {
                #[cfg(test)]
                {
                    Ok(mock_embed(text))
                }
                #[cfg(not(test))]
                {
                    anyhow::bail!("Mock embedder is not available in production builds");
                }
            }
        }
    }

    /// Insert a memory item: generates an embedding, stores metadata in SQLite,
    /// and indexes the quantized vector under the generated ID.
    pub fn insert(
        &mut self,
        text: &str,
        file_path: Option<&str>,
        workspace_path: &str,
    ) -> anyhow::Result<()> {
        let embedding = self.embed_text(text)?;

        self.db.execute(
            "INSERT INTO memory_metadata (text, file_path, workspace_path) VALUES (?, ?, ?)",
            rusqlite::params![text, file_path, workspace_path],
        )?;
        let row_id = self.db.last_insert_rowid() as u64;

        self.index
            .add_with_ids(&embedding, &[row_id])
            .map_err(|e| anyhow::anyhow!("{:?}", e))?;
        self.persist()?;

        Ok(())
    }

    /// Delete the last recorded memory item for this workspace.
    pub fn delete_last_memory(&mut self, workspace_path: &str) -> anyhow::Result<bool> {
        use rusqlite::OptionalExtension;
        let last_row: Option<(i64, String)> = self.db.query_row(
            "SELECT id, text FROM memory_metadata WHERE workspace_path = ? ORDER BY id DESC LIMIT 1",
            rusqlite::params![workspace_path],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        ).optional()?;

        if let Some((id, _text)) = last_row {
            self.db.execute(
                "DELETE FROM memory_metadata WHERE id = ?",
                rusqlite::params![id],
            )?;
            self.persist()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Retrieves memories matching the query, restricted to the active workspace.
    ///
    /// Executes a hybrid search combining:
    /// 1. **Dense Semantic Search**: Generates vector embeddings via `fastembed` and queries `turbovec` (optionally adjusted via Sona Micro-LoRA).
    /// 2. **Sparse Keyword Search**: Queries a SQLite FTS5 virtual table using the native `bm25` scoring algorithm.
    ///
    /// The dense and sparse ranks are fused using the **Reciprocal Rank Fusion (RRF)** formula
    /// to return the final list of high-signal memories.
    pub fn search(
        &mut self,
        query: &str,
        sona: Option<&ruvector_sona::SonaEngine>,
        workspace_path: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryMatch>> {
        use std::collections::HashMap;

        // 1. Run Sparse FTS5 BM25 matching
        let mut sparse_matches = Vec::new();
        let parsed_query = query
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>();
        let fts_query = parsed_query
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" OR ");

        if !fts_query.is_empty() {
            let prep_res = self.db.prepare(
                "SELECT id, text, file_path, bm25(memory_fts) as score \
                 FROM memory_fts \
                 JOIN memory_metadata ON memory_metadata.id = memory_fts.rowid \
                 WHERE (memory_metadata.workspace_path = ? OR memory_metadata.workspace_path = 'global') AND memory_fts MATCH ? \
                 ORDER BY score ASC LIMIT ?",
            );
            if let Ok(mut stmt) = prep_res {
                let query_res = stmt.query(rusqlite::params![workspace_path, fts_query, limit]);
                if let Ok(mut rows) = query_res {
                    while let Ok(Some(row)) = rows.next() {
                        if let (Ok(id), Ok(text), Ok(file_path), Ok(score)) = (
                            row.get::<_, u64>(0),
                            row.get::<_, String>(1),
                            row.get::<_, Option<String>>(2),
                            row.get::<_, f32>(3),
                        ) {
                            sparse_matches.push((id, text, file_path, score));
                        }
                    }
                }
            }
        }

        // 2. Run Dense Vector matching
        let mut dense_matches = Vec::new();
        if !self.index.is_empty() {
            let query_vec_res = self.embed_text(query);
            if let Ok(query_vector) = query_vec_res {
                let mut adjusted_vector = query_vector;
                if let Some(sona_engine) = sona {
                    let mut shift = vec![0.0f32; 384];
                    sona_engine.apply_micro_lora(&adjusted_vector, &mut shift);
                    for (q, s) in adjusted_vector.iter_mut().zip(shift) {
                        *q += s;
                    }
                }

                // Retrieve candidate row IDs for the active workspace and global memories
                let prep_res = self.db.prepare("SELECT id FROM memory_metadata WHERE workspace_path = ? OR workspace_path = 'global'");
                if let Ok(mut stmt) = prep_res {
                    let query_res = stmt.query_map([workspace_path], |row| row.get::<_, u64>(0));
                    if let Ok(sqlite_ids) = query_res {
                        let allowed_ids: Vec<u64> = sqlite_ids
                            .filter_map(Result::ok)
                            .filter(|&id| self.index.contains(id))
                            .collect();

                        if !allowed_ids.is_empty() {
                            let (scores, ids) = self.index.search_with_allowlist(
                                &adjusted_vector,
                                limit,
                                Some(&allowed_ids),
                            );
                            for (score, id) in scores.into_iter().zip(ids) {
                                if let Ok((text, file_path)) = self.db.query_row(
                                    "SELECT text, file_path FROM memory_metadata WHERE id = ?",
                                    [id],
                                    |row| {
                                        let t: String = row.get(0).unwrap_or_default();
                                        let f: Option<String> = row.get(1).unwrap_or(None);
                                        Ok((t, f))
                                    },
                                ) {
                                    dense_matches.push((id, text, file_path, score));
                                }
                            }
                        }
                    }
                }
            }
        }

        // 3. Reciprocal Rank Fusion (RRF)
        let k = 60.0f32;
        let mut rrf_scores: HashMap<u64, (String, Option<String>, f32)> = HashMap::new();

        // Accumulate RRF points from Dense Rank
        for (rank, (id, text, file_path, _score)) in dense_matches.iter().enumerate() {
            let rank_score = 1.0 / (k + (rank + 1) as f32);
            rrf_scores
                .entry(*id)
                .or_insert_with(|| (text.clone(), file_path.clone(), 0.0))
                .2 += rank_score;
        }

        // Accumulate RRF points from Sparse Rank
        for (rank, (id, text, file_path, _score)) in sparse_matches.iter().enumerate() {
            let rank_score = 1.0 / (k + (rank + 1) as f32);
            rrf_scores
                .entry(*id)
                .or_insert_with(|| (text.clone(), file_path.clone(), 0.0))
                .2 += rank_score;
        }

        // Sort by RRF score descending and limit results
        let mut combined_matches: Vec<MemoryMatch> = rrf_scores
            .into_iter()
            .map(|(_, (text, file_path, rrf_score))| MemoryMatch {
                text,
                file_path,
                score: rrf_score,
            })
            .collect();

        combined_matches.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        combined_matches.truncate(limit);

        Ok(combined_matches)
    }

    /// Write the current turbovec index state to disk.
    pub fn persist(&self) -> anyhow::Result<()> {
        self.index.write(&self.index_path)?;
        Ok(())
    }

    /// Delete all indexed vectors and metadata rows associated with a workspace.
    pub fn clear_workspace(&mut self, workspace_path: &str) -> anyhow::Result<()> {
        let mut stmt = self
            .db
            .prepare("SELECT id FROM memory_metadata WHERE workspace_path = ?")?;
        let ids: Vec<u64> = stmt
            .query_map([workspace_path], |row| row.get(0))?
            .filter_map(Result::ok)
            .collect();

        for id in ids {
            self.index.remove(id);
        }

        self.db.execute(
            "DELETE FROM memory_metadata WHERE workspace_path = ?",
            [workspace_path],
        )?;

        self.persist()?;
        Ok(())
    }
}

#[cfg(test)]
fn mock_embed(text: &str) -> Vec<f32> {
    let mut vec = vec![0.0; 384];
    let text_lower = text.to_lowercase();
    if text_lower.contains("rust") || text_lower.contains("safety") || text_lower.contains("speed")
    {
        vec[0] = 1.0;
    } else if text_lower.contains("python") || text_lower.contains("interpreted") {
        vec[1] = 1.0;
    } else if text_lower.contains("cargo") || text_lower.contains("package manager") {
        vec[2] = 1.0;
    } else {
        // Fallback deterministic vector
        let bytes = text.as_bytes();
        for i in 0..384 {
            if !bytes.is_empty() {
                let b = bytes[i % bytes.len()] as f32;
                vec[i] = (b * (i as f32 + 1.0)).sin();
            } else {
                vec[i] = (i as f32).cos();
            }
        }
    }
    // Normalize
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for val in &mut vec {
            *val /= norm;
        }
    }
    vec
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_helix_memory_engine() {
        let temp_dir = TempDir::new().unwrap();
        let mut engine = HelixMemoryEngine::new(temp_dir.path()).unwrap();

        let workspace_a = "/path/to/project_a";
        let workspace_b = "/path/to/project_b";

        // Insert memories in workspace A
        engine
            .insert(
                "Rust is a systems programming language focusing on safety and speed.",
                None,
                workspace_a,
            )
            .unwrap();
        engine
            .insert(
                "Python is an interpreted high-level general-purpose programming language.",
                None,
                workspace_a,
            )
            .unwrap();

        // Insert memories in workspace B
        engine
            .insert(
                "Cargo is the Rust package manager that downloads dependencies.",
                None,
                workspace_b,
            )
            .unwrap();

        // Test search in workspace A
        let matches_a = engine
            .search(
                "What is safety and speed in programming?",
                None,
                workspace_a,
                5,
            )
            .unwrap();
        assert!(
            !matches_a.is_empty(),
            "Should return matches in workspace A"
        );
        assert!(
            matches_a[0].text.contains("Rust"),
            "Best match should be about Rust"
        );

        // Test workspace isolation: search for Cargo in workspace A (should find nothing from workspace B)
        let matches_cargo_a = engine
            .search("package manager", None, workspace_a, 5)
            .unwrap();
        for m in &matches_cargo_a {
            assert!(
                !m.text.contains("Cargo"),
                "Should not find workspace B memories in workspace A"
            );
        }

        // Test search in workspace B
        let matches_cargo_b = engine
            .search("package manager", None, workspace_b, 5)
            .unwrap();
        assert!(
            !matches_cargo_b.is_empty(),
            "Should find matches in workspace B"
        );
        assert!(
            matches_cargo_b[0].text.contains("Cargo"),
            "Best match should be about Cargo"
        );

        // Test persistence and reload
        engine.persist().unwrap();
        drop(engine);

        let mut reloaded = HelixMemoryEngine::new(temp_dir.path()).unwrap();
        let matches_reload = reloaded
            .search(
                "What is safety and speed in programming?",
                None,
                workspace_a,
                5,
            )
            .unwrap();
        assert!(!matches_reload.is_empty());
        assert!(matches_reload[0].text.contains("Rust"));

        // Test clearing workspace A
        reloaded.clear_workspace(workspace_a).unwrap();
        let matches_cleared = reloaded
            .search("safety and speed", None, workspace_a, 5)
            .unwrap();
        assert!(matches_cleared.is_empty(), "Workspace A should be empty");

        // Workspace B should still be intact
        let matches_intact = reloaded
            .search("package manager", None, workspace_b, 5)
            .unwrap();
        assert!(
            !matches_intact.is_empty(),
            "Workspace B should still have memories"
        );
    }
}
