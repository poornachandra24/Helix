use chrono::Local;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Instant;

// ──────────────────────────────────────────────
// Per-turn metrics
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnMetrics {
    pub turn_index: usize,
    pub prompt_chars: usize,
    pub duration_ms: u64,
    /// How many agent iterations (model calls) this turn took.
    pub agent_steps: usize,
    /// Total tool dispatches across all steps.
    pub tool_calls: usize,
    /// Times the Local Healer retried due to a parse error.
    pub healer_retries: usize,
    /// Whether context compaction fired at least once during this turn.
    pub compaction_fired: bool,
    pub ended_with_error: bool,
    pub timestamp: String,
}

// ──────────────────────────────────────────────
// Session-level summary (written to disk)
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionSummary {
    pub session_id: String,
    pub turn_count: usize,
    pub avg_duration_ms: f64,
    pub p95_duration_ms: u64,
    pub avg_agent_steps: f64,
    pub total_tool_calls: usize,
    pub total_healer_retries: usize,
    /// Compaction events per 10 turns.
    pub compaction_rate_per_10: f64,
    /// Fraction of turns that ended with an error.
    pub error_rate: f64,
}

// ──────────────────────────────────────────────
// Collector
// ──────────────────────────────────────────────

pub struct MetricsCollector {
    pub session_id: String,
    turns: Vec<TurnMetrics>,
}

impl MetricsCollector {
    pub fn new(session_id: &str) -> Self {
        Self { session_id: session_id.to_string(), turns: Vec::new() }
    }

    pub fn record(&mut self, metrics: TurnMetrics) {
        self.turns.push(metrics);
    }

    pub fn turns(&self) -> &[TurnMetrics] {
        &self.turns
    }

    pub fn turn_count(&self) -> usize {
        self.turns.len()
    }

    pub fn summary(&self) -> SessionSummary {
        let n = self.turns.len();
        if n == 0 {
            return SessionSummary { session_id: self.session_id.clone(), ..Default::default() };
        }

        let mut durations: Vec<u64> = self.turns.iter().map(|t| t.duration_ms).collect();
        durations.sort_unstable();
        let p95_idx = ((n as f64 * 0.95) as usize).min(n - 1);

        let compaction_count = self.turns.iter().filter(|t| t.compaction_fired).count();
        let error_count = self.turns.iter().filter(|t| t.ended_with_error).count();

        SessionSummary {
            session_id:          self.session_id.clone(),
            turn_count:          n,
            avg_duration_ms:     durations.iter().sum::<u64>() as f64 / n as f64,
            p95_duration_ms:     durations[p95_idx],
            avg_agent_steps:     self.turns.iter().map(|t| t.agent_steps).sum::<usize>() as f64 / n as f64,
            total_tool_calls:    self.turns.iter().map(|t| t.tool_calls).sum(),
            total_healer_retries:self.turns.iter().map(|t| t.healer_retries).sum(),
            compaction_rate_per_10: compaction_count as f64 / n as f64 * 10.0,
            error_rate:          error_count as f64 / n as f64,
        }
    }

    /// Write session summary to `<sessions_dir>/<session_id>.metrics.json`.
    pub fn flush_to_disk(&self, sessions_dir: &Path) -> anyhow::Result<()> {
        let summary = self.summary();
        let path = sessions_dir.join(format!("{}.metrics.json", self.session_id));
        let json = serde_json::to_string_pretty(&summary)?;
        std::fs::write(&path, json)?;
        Ok(())
    }
}

// ──────────────────────────────────────────────
// Timer helper — started at the top of run_turn
// ──────────────────────────────────────────────

pub struct TurnTimer {
    start: Instant,
    pub prompt_chars: usize,
}

impl TurnTimer {
    pub fn start(prompt: &str) -> Self {
        Self { start: Instant::now(), prompt_chars: prompt.len() }
    }

    pub fn finish(
        self,
        turn_index: usize,
        agent_steps: usize,
        tool_calls: usize,
        healer_retries: usize,
        compaction_fired: bool,
        ended_with_error: bool,
    ) -> TurnMetrics {
        TurnMetrics {
            turn_index,
            prompt_chars: self.prompt_chars,
            duration_ms: self.start.elapsed().as_millis() as u64,
            agent_steps,
            tool_calls,
            healer_retries,
            compaction_fired,
            ended_with_error,
            timestamp: Local::now().to_rfc3339(),
        }
    }
}
