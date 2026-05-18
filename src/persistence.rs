use anyhow::Result;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use serde_json::Value;
use chrono::Local;

pub struct Session {
    path: PathBuf,
}

impl Session {
    pub fn new(session_id: Option<&str>) -> Result<Self> {
        let state_dir = crate::config::get_state_dir()?.join("sessions");
        fs::create_dir_all(&state_dir)?;
        
        let id = session_id.map(|s| s.to_string()).unwrap_or_else(|| {
            Local::now().format("%Y%m%d_%H%M%S").to_string()
        });
        
        let path = state_dir.join(format!("{}.jsonl", id));
        Ok(Self { path })
    }

    pub fn append(&self, event: Value) -> Result<()> {
        let line = serde_json::to_string(&event)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
            
        writeln!(file, "{}", line)?;
        Ok(())
    }
}
