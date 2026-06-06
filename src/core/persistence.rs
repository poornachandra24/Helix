use anyhow::Result;
use chrono::{DateTime, Local};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

// ──────────────────────────────────────────────
// Session
// ──────────────────────────────────────────────

pub struct Session {
    pub id: String,
    pub path: PathBuf,
}

impl Session {
    pub fn new(session_id: Option<&str>) -> Result<Self> {
        let state_dir = crate::config::get_state_dir()?.join("sessions");
        fs::create_dir_all(&state_dir)?;

        let id = session_id
            .map(|s| s.to_string())
            .unwrap_or_else(|| Local::now().format("%Y%m%d_%H%M%S").to_string());

        let path = state_dir.join(format!("{}.jsonl", id));
        Ok(Self { id, path })
    }

    /// Append an event to the session log, stamping it with the current time.
    pub fn append(&self, mut event: Value) -> Result<()> {
        event["ts"] = serde_json::json!(Local::now().to_rfc3339());
        let line = serde_json::to_string(&event)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", line)?;
        Ok(())
    }

    /// Reconstruct a conversation message history from a session file.
    /// Only `user_input` and `end_turn` events are converted to messages;
    /// tool events are summarised so context is manageable on resume.
    pub fn load_messages(path: &Path) -> Result<Vec<Value>> {
        let file = fs::File::open(path)?;
        let reader = BufReader::new(file);
        let mut messages = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };

            match event["event"].as_str() {
                Some("user_input") => {
                    if let Some(content) = event["content"].as_str() {
                        messages.push(serde_json::json!({ "role": "user", "content": content }));
                    }
                }
                Some("end_turn") => {
                    if let Some(content) = event["content"].as_str() {
                        messages
                            .push(serde_json::json!({ "role": "assistant", "content": content }));
                    }
                }
                _ => {}
            }
        }

        Ok(messages)
    }
}

// ──────────────────────────────────────────────
// Session listing
// ──────────────────────────────────────────────

#[derive(Debug)]
pub struct SessionMeta {
    pub id: String,
    pub path: PathBuf,
    pub modified_at: DateTime<Local>,
    pub event_count: usize,
}

/// List all saved sessions, sorted by most-recently-modified first.
pub fn list_sessions() -> Result<Vec<SessionMeta>> {
    let state_dir = crate::config::get_state_dir()?.join("sessions");
    if !state_dir.exists() {
        return Ok(vec![]);
    }

    let mut sessions: Vec<SessionMeta> = fs::read_dir(&state_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("jsonl"))
        .filter_map(|entry| {
            let path = entry.path();
            let id = path.file_stem()?.to_string_lossy().to_string();
            let meta = entry.metadata().ok()?;
            let modified: DateTime<Local> = meta.modified().ok()?.into();

            // Count events by counting lines
            let count = fs::read_to_string(&path)
                .map(|s| s.lines().count())
                .unwrap_or(0);

            Some(SessionMeta {
                id,
                path,
                modified_at: modified,
                event_count: count,
            })
        })
        .collect();

    sessions.sort_by_key(|b| std::cmp::Reverse(b.modified_at));
    Ok(sessions)
}
