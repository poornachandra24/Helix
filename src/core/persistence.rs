#![allow(clippy::collapsible_if)]

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
        redact_value(&mut event);
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

    /// Remove the last turn (from the last user_input onwards) from the session file.
    pub fn forget_last_turn(&self) -> Result<bool> {
        if !self.path.exists() {
            return Ok(false);
        }
        let content = fs::read_to_string(&self.path)?;
        let lines: Vec<&str> = content.lines().collect();
        let mut last_user_idx = None;
        for (i, line) in lines.iter().enumerate() {
            if let Ok(event) = serde_json::from_str::<Value>(line) {
                if event["event"].as_str() == Some("user_input") {
                    last_user_idx = Some(i);
                }
            }
        }

        if let Some(idx) = last_user_idx {
            let kept_lines = &lines[..idx];
            let mut new_content = kept_lines.join("\n");
            if !kept_lines.is_empty() {
                new_content.push('\n');
            }
            fs::write(&self.path, new_content)?;
            Ok(true)
        } else {
            Ok(false)
        }
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

fn redact_value(value: &mut Value) {
    match value {
        Value::String(s) => {
            *s = redact_secrets(s);
        }
        Value::Array(arr) => {
            for v in arr {
                redact_value(v);
            }
        }
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                let lower_k = k.to_lowercase();
                let is_secret = lower_k.contains("api_key")
                    || lower_k.contains("secret")
                    || lower_k.contains("password");
                if is_secret {
                    let _ = ();
                    if let Value::String(s) = v {
                        let is_empty = s.is_empty();
                        if !is_empty {
                            *s = "[REDACTED_CREDENTIALS]".to_string();
                            continue;
                        }
                    }
                }
                redact_value(v);
            }
        }
        _ => {}
    }
}

pub fn redact_secrets(text: &str) -> String {
    let mut lines = Vec::new();
    for line in text.lines() {
        let mut words = Vec::new();
        for word in line.split_whitespace() {
            let clean_word = word.trim_matches(|c| {
                c == '"' || c == '\'' || c == ',' || c == ';' || c == '{' || c == '}'
            });
            if is_sensitive_key(clean_word) {
                words.push(word.replace(clean_word, "[REDACTED_API_KEY]"));
            } else {
                words.push(word.to_string());
            }
        }
        lines.push(words.join(" "));
    }
    lines.join("\n")
}

fn is_sensitive_key(word: &str) -> bool {
    if word.len() >= 20 {
        if word.starts_with("sk-proj-") || word.starts_with("sk-ant-") || word.starts_with("AIzaSy")
        {
            return true;
        }
        if word.starts_with("sk-")
            && word
                .chars()
                .skip(3)
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_redact_secrets() {
        let input = "Here is my key: sk-proj-123456789012345678901234567890 and anthropic key: \"sk-ant-123456789012345678901234567890\"";
        let expected =
            "Here is my key: [REDACTED_API_KEY] and anthropic key: \"[REDACTED_API_KEY]\"";
        assert_eq!(redact_secrets(input), expected);
    }

    #[test]
    fn test_redact_value_credentials() {
        let mut val = json!({
            "user_input": "connect",
            "api_key": "some_plain_text_api_key",
            "password": "mysecretpassword",
            "nested": {
                "secret_token": "some_value"
            }
        });

        redact_value(&mut val);

        assert_eq!(val["api_key"], "[REDACTED_CREDENTIALS]");
        assert_eq!(val["password"], "[REDACTED_CREDENTIALS]");
        assert_eq!(val["nested"]["secret_token"], "[REDACTED_CREDENTIALS]");
    }
}
