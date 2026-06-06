pub mod skills;

use std::path::Path;
use turbovec::IdMapIndex;
use fastembed::{TextEmbedding, InitOptions, EmbeddingModel};
use rusqlite::Connection;

#[derive(Debug, Clone)]
pub struct MemoryMatch {
    pub text: String,
    pub file_path: Option<String>,
    pub score: f32,
}

pub struct HelixMemoryEngine {
    index: IdMapIndex,
    pub db: Connection,
    embedder: TextEmbedding,
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
        
        // Load default local embedding model (BGESmallENV15 - 384 dimensions)
        let embedder = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::BGESmallENV15)
                .with_show_download_progress(false)
        )?;
        
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

    /// Embed a single text string using fastembed.
    pub fn embed_text(&mut self, text: &str) -> anyhow::Result<Vec<f32>> {
        let embeddings = self.embedder.embed(vec![text], None)?;
        if embeddings.is_empty() {
            anyhow::bail!("Failed to generate embedding");
        }
        Ok(embeddings[0].clone())
    }
    
    /// Insert a memory item: generates an embedding, stores metadata in SQLite,
    /// and indexes the quantized vector under the generated ID.
    pub fn insert(
        &mut self,
        text: &str,
        file_path: Option<&str>,
        workspace_path: &str,
    ) -> anyhow::Result<()> {
        let embeddings = self.embedder.embed(vec![text], None)?;
        if embeddings.is_empty() {
            anyhow::bail!("Failed to generate embedding");
        }
        let embedding = &embeddings[0];
        
        self.db.execute(
            "INSERT INTO memory_metadata (text, file_path, workspace_path) VALUES (?, ?, ?)",
            rusqlite::params![text, file_path, workspace_path],
        )?;
        let row_id = self.db.last_insert_rowid() as u64;
        
        self.index.add_with_ids(embedding, &[row_id]).map_err(|e| anyhow::anyhow!("{:?}", e))?;
        self.persist()?;
        
        Ok(())
    }
    
    /// Retrieve memories similar to the query, restricted to the active workspace.
    pub fn search(
        &mut self,
        query: &str,
        sona: Option<&ruvector_sona::SonaEngine>,
        workspace_path: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<MemoryMatch>> {
        if self.index.is_empty() {
            return Ok(Vec::new());
        }
        
        let embeddings = self.embedder.embed(vec![query], None)?;
        if embeddings.is_empty() {
            anyhow::bail!("Failed to embed search query");
        }
        let mut query_vector = embeddings[0].clone();
        
        if let Some(sona_engine) = sona {
            let mut optimized = vec![0.0f32; 384];
            sona_engine.apply_micro_lora(&query_vector, &mut optimized);
            query_vector = optimized;
        }
        
        // Retrieve candidate row IDs for the active workspace
        let mut stmt = self.db.prepare(
            "SELECT id FROM memory_metadata WHERE workspace_path = ?"
        )?;
        let sqlite_ids: Vec<u64> = stmt
            .query_map([workspace_path], |row| row.get(0))?
            .filter_map(Result::ok)
            .collect();
            
        // Filter allowed IDs so we don't pass non-existent keys to turbovec (which would panic)
        let allowed_ids: Vec<u64> = sqlite_ids
            .into_iter()
            .filter(|&id| self.index.contains(id))
            .collect();
            
        if allowed_ids.is_empty() {
            return Ok(Vec::new());
        }
        
        let (scores, ids) = self.index.search_with_allowlist(&query_vector, limit, Some(&allowed_ids));
        
        let mut matches = Vec::new();
        for (score, id) in scores.into_iter().zip(ids) {
            let mut stmt = self.db.prepare(
                "SELECT text, file_path FROM memory_metadata WHERE id = ?"
            )?;
            let mut rows = stmt.query([id])?;
            if let Some(row) = rows.next()? {
                let text: String = row.get(0)?;
                let file_path: Option<String> = row.get(1)?;
                matches.push(MemoryMatch {
                    text,
                    file_path,
                    score,
                });
            }
        }
        
        Ok(matches)
    }
    
    /// Write the current turbovec index state to disk.
    pub fn persist(&self) -> anyhow::Result<()> {
        self.index.write(&self.index_path)?;
        Ok(())
    }
    
    /// Delete all indexed vectors and metadata rows associated with a workspace.
    pub fn clear_workspace(&mut self, workspace_path: &str) -> anyhow::Result<()> {
        let mut stmt = self.db.prepare(
            "SELECT id FROM memory_metadata WHERE workspace_path = ?"
        )?;
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
        engine.insert("Rust is a systems programming language focusing on safety and speed.", None, workspace_a).unwrap();
        engine.insert("Python is an interpreted high-level general-purpose programming language.", None, workspace_a).unwrap();

        // Insert memories in workspace B
        engine.insert("Cargo is the Rust package manager that downloads dependencies.", None, workspace_b).unwrap();

        // Test search in workspace A
        let matches_a = engine.search("What is safety and speed in programming?", None, workspace_a, 5).unwrap();
        assert!(!matches_a.is_empty(), "Should return matches in workspace A");
        assert!(matches_a[0].text.contains("Rust"), "Best match should be about Rust");

        // Test workspace isolation: search for Cargo in workspace A (should find nothing from workspace B)
        let matches_cargo_a = engine.search("package manager", None, workspace_a, 5).unwrap();
        for m in &matches_cargo_a {
            assert!(!m.text.contains("Cargo"), "Should not find workspace B memories in workspace A");
        }

        // Test search in workspace B
        let matches_cargo_b = engine.search("package manager", None, workspace_b, 5).unwrap();
        assert!(!matches_cargo_b.is_empty(), "Should find matches in workspace B");
        assert!(matches_cargo_b[0].text.contains("Cargo"), "Best match should be about Cargo");

        // Test persistence and reload
        engine.persist().unwrap();
        drop(engine);

        let mut reloaded = HelixMemoryEngine::new(temp_dir.path()).unwrap();
        let matches_reload = reloaded.search("What is safety and speed in programming?", None, workspace_a, 5).unwrap();
        assert!(!matches_reload.is_empty());
        assert!(matches_reload[0].text.contains("Rust"));

        // Test clearing workspace A
        reloaded.clear_workspace(workspace_a).unwrap();
        let matches_cleared = reloaded.search("safety and speed", None, workspace_a, 5).unwrap();
        assert!(matches_cleared.is_empty(), "Workspace A should be empty");

        // Workspace B should still be intact
        let matches_intact = reloaded.search("package manager", None, workspace_b, 5).unwrap();
        assert!(!matches_intact.is_empty(), "Workspace B should still have memories");
    }
}
