use anyhow::Result;
use std::path::{Path, PathBuf};
use crate::core::metrics::SessionSummary;
use std::fs;
use std::collections::HashMap;

pub struct Analyzer {
    sessions_dir: PathBuf,
}

impl Analyzer {
    pub fn new(sessions_dir: &Path) -> Self {
        Self { sessions_dir: sessions_dir.to_path_buf() }
    }

    /// Read the last N metrics and logs, and summarize the biggest bottlenecks.
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
            if let Ok(content) = fs::read_to_string(&path)
                && let Ok(summary) = serde_json::from_str::<SessionSummary>(&content) {
                    summaries.push(summary);
                }
        }

        if summaries.is_empty() {
            anyhow::bail!("No recent sessions found to analyze. Run some tasks or /benchmark first.");
        }

        // Aggregate session metrics
        let avg_latency = summaries.iter().map(|s| s.avg_duration_ms).sum::<f64>() / summaries.len() as f64;
        let p95_latency = summaries.iter().map(|s| s.p95_duration_ms).max().unwrap_or(0);
        let avg_steps = summaries.iter().map(|s| s.avg_agent_steps).sum::<f64>() / summaries.len() as f64;
        let total_retries: usize = summaries.iter().map(|s| s.total_healer_retries).sum();
        let avg_compaction = summaries.iter().map(|s| s.compaction_rate_per_10).sum::<f64>() / summaries.len() as f64;

        // Tool telemetry
        let mut tool_runs = HashMap::new();
        let mut tool_failures = HashMap::new();
        let mut sample_errors = Vec::new();

        for s in &summaries {
            let log_path = self.sessions_dir.join(format!("{}.jsonl", s.session_id));
            if log_path.exists()
                && let Ok(file_content) = fs::read_to_string(&log_path) {
                    for line in file_content.lines() {
                        if let Ok(event) = serde_json::from_str::<serde_json::Value>(line) {
                            match event["event"].as_str() {
                                Some("tool_calls") => {
                                    if let Some(arr) = event["tools"].as_array() {
                                        for t in arr {
                                            if let Some(name) = t.as_str() {
                                                *tool_runs.entry(name.to_string()).or_insert(0) += 1;
                                            }
                                        }
                                    }
                                }
                                Some("tool_result") => {
                                    let tool_name = event["tool"].as_str().unwrap_or("").to_string();
                                    let result_str = event["result"].as_str().unwrap_or("");
                                    if result_str.contains("Error") || result_str.contains("failed") || result_str.contains("stderr") {
                                        *tool_failures.entry(tool_name.clone()).or_insert(0) += 1;
                                        if sample_errors.len() < 5 {
                                            // Extract a snippet of the error
                                            let snippet: String = result_str.chars().take(200).collect();
                                            sample_errors.push(format!("Tool '{}' failed: {}", tool_name, snippet.trim()));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
        }

        let mut report = String::new();
        report.push_str(&format!("Analyzed {} recent sessions.\n", summaries.len()));
        report.push_str("Performance Metrics:\n");
        report.push_str(&format!("  Average Turn Latency: {:.0}ms\n", avg_latency));
        report.push_str(&format!("  P95 Max Latency: {}ms\n", p95_latency));
        report.push_str(&format!("  Average Steps/Turn: {:.2}\n", avg_steps));
        report.push_str(&format!("  Total Healer Retries: {}\n", total_retries));
        report.push_str(&format!("  Compaction Rate per 10 Turns: {:.2}\n\n", avg_compaction));

        report.push_str("Tool Usage Stats:\n");
        if tool_runs.is_empty() {
            report.push_str("  No tool runs recorded.\n");
        } else {
            for (tool, runs) in &tool_runs {
                let fails = tool_failures.get(tool).unwrap_or(&0);
                report.push_str(&format!("  - {}: {} runs, {} failed\n", tool, runs, fails));
            }
        }

        if !sample_errors.is_empty() {
            report.push_str("\nRecent Error Snippets / Failures:\n");
            for err in &sample_errors {
                report.push_str(&format!("  - {}\n", err));
            }
        }

        Ok(report)
    }
}
