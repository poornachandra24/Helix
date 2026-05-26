use anyhow::Result;
use std::path::{Path, PathBuf};
use crate::core::metrics::SessionSummary;
use std::fs;

pub struct Analyzer {
    sessions_dir: PathBuf,
}

impl Analyzer {
    pub fn new(sessions_dir: &Path) -> Self {
        Self { sessions_dir: sessions_dir.to_path_buf() }
    }

    /// Read the last N metrics files and summarize the biggest bottleneck.
    pub fn analyze_recent_metrics(&self, limit: usize) -> Result<String> {
        let mut entries: Vec<PathBuf> = fs::read_dir(&self.sessions_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            .map(|e| e.path())
            .collect();
        
        entries.sort(); // oldest to newest
        entries.reverse(); // newest to oldest
        entries.truncate(limit);
        
        let mut summaries = Vec::new();
        for path in entries {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(summary) = serde_json::from_str::<SessionSummary>(&content) {
                    summaries.push(summary);
                }
            }
        }

        if summaries.is_empty() {
            anyhow::bail!("No recent sessions found to analyze. Run some tasks or /benchmark first.");
        }

        // Just aggregate metrics for the LLM to read
        let avg_latency: f64 = summaries.iter().map(|s| s.avg_duration_ms).sum::<f64>() / summaries.len() as f64;
        let avg_retries: f64 = summaries.iter().map(|s| s.total_healer_retries as f64).sum::<f64>() / summaries.len() as f64;
        
        let report = format!(
            "Analyzed {} recent sessions.\nAverage latency: {:.0}ms\nAverage healer retries: {:.2}",
            summaries.len(), avg_latency, avg_retries
        );
        Ok(report)
    }
}
