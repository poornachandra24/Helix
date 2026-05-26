use anyhow::Result;
use console::style;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::config::AppConfig;
use crate::core::context::{ContextManager, ContextBudget};
use crate::core::engine::Engine;
use crate::core::metrics::{MetricsCollector, SessionSummary};
use crate::model::OpenAiCompatibleAdapter;
use crate::core::persistence::Session;
use crate::tools::ToolRegistry;
use crate::tools::builtins;

// ──────────────────────────────────────────────
// Benchmark case schema
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchCase {
    pub id: String,
    pub description: String,
    pub prompt: String,
    /// At least one of these tool names must appear in the session's tool_calls events.
    pub expected_tools: Vec<String>,
    /// Fail if agent exceeds this many iterations.
    pub max_iterations: usize,
    /// Fail if total wall-clock time exceeds this.
    pub timeout_ms: u64,
}

// ──────────────────────────────────────────────
// Per-case result
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseResult {
    pub id: String,
    pub passed: bool,
    pub duration_ms: u64,
    pub tools_used: Vec<String>,
    pub expected_tools_found: bool,
    pub timeout_exceeded: bool,
    pub error: Option<String>,
}

// ──────────────────────────────────────────────
// Baseline schema
// ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    pub version: String,
    pub model: String,
    pub provider: String,
    pub metrics: BaselineMetrics,
    pub case_latencies: std::collections::HashMap<String, u64>, // case_id → p50 ms
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BaselineMetrics {
    pub p95_latency_ms: u64,
    pub avg_tokens_per_turn: f64,
    pub tool_accuracy_pct: f64,
    pub avg_healer_retries: f64,
    pub compaction_rate_per_10: f64,
    pub error_rate: f64,
}

// ──────────────────────────────────────────────
// Regression check thresholds
// ──────────────────────────────────────────────

struct Thresholds;
impl Thresholds {
    const LATENCY_SLACK:      f64 = 1.10; // up to 10% slower is OK
    const RETRY_SLACK:        f64 = 1.20; // up to 20% more retries OK
    const COMPACTION_SLACK:   f64 = 1.20; // up to 20% more compaction OK
    const TOOL_ACCURACY_FLOOR: f64 = -0.0; // no drop in accuracy allowed
}

// ──────────────────────────────────────────────
// Runner
// ──────────────────────────────────────────────

pub struct BenchmarkRunner {
    suite_dir:    PathBuf,
    baseline_path: PathBuf,
    config:       AppConfig,
}

impl BenchmarkRunner {
    pub fn new(suite_dir: &Path, baseline_path: &Path, config: AppConfig) -> Self {
        Self {
            suite_dir:     suite_dir.to_path_buf(),
            baseline_path: baseline_path.to_path_buf(),
            config,
        }
    }

    /// Load all `.json` cases from the suite directory, sorted by filename.
    pub fn load_cases(&self) -> Result<Vec<BenchCase>> {
        let mut cases = Vec::new();
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&self.suite_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            .map(|e| e.path())
            .collect();
        entries.sort();

        for path in entries {
            let content = std::fs::read_to_string(&path)?;
            let case: BenchCase = serde_json::from_str(&content)
                .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", path.display(), e))?;
            cases.push(case);
        }

        Ok(cases)
    }

    /// Load the committed baseline, if it exists.
    pub fn load_baseline(&self) -> Option<Baseline> {
        let content = std::fs::read_to_string(&self.baseline_path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Run all cases and return per-case results + aggregate session summary.
    pub async fn run(&self) -> Result<(Vec<CaseResult>, SessionSummary)> {
        let cases = self.load_cases()?;
        if cases.is_empty() {
            anyhow::bail!("No benchmark cases found in {}", self.suite_dir.display());
        }

        println!("\n{}", style("═══ Benchmark Suite ═══════════════════════════════").bold().cyan());
        println!("  {} cases | provider: {} | model: {}",
            style(cases.len()).bold(),
            style(&self.config.active_provider).green(),
            style(&self.config.active_model).green(),
        );
        println!("{}", style("───────────────────────────────────────────────────").dim());

        let mut collector = MetricsCollector::new("benchmark");
        let mut results  = Vec::new();

        for case in &cases {
            let result = self.run_case(case, &mut collector).await;
            print_case_result(&result);
            results.push(result);
        }

        let summary = collector.summary();
        println!("{}", style("═══════════════════════════════════════════════════").bold().cyan());
        self.print_summary(&summary, &results);

        Ok((results, summary))
    }

    async fn run_case(
        &self,
        case: &BenchCase,
        collector: &mut MetricsCollector,
    ) -> CaseResult {
        let tools   = build_bench_registry();
        let system  = "You are an autonomous AI agent. Use the provided tools to answer the user. \
                       Be concise. When you have the answer, respond without calling any more tools.";

        let lookup  = crate::model::registry::build_lookup_client();
        let context = build_bench_context(&self.config, system, &tools.descriptors(), &lookup).await;
        let model   = Box::new(OpenAiCompatibleAdapter::new(self.config.clone()));

        // Use a fresh session per case so we can read its tool_calls events
        let case_session = match Session::new(Some(&format!("bench_{}", &case.id))) {
            Ok(s)  => s,
            Err(e) => {
                return CaseResult {
                    id:                   case.id.clone(),
                    passed:               false,
                    duration_ms:          0,
                    tools_used:           vec![],
                    expected_tools_found: false,
                    timeout_exceeded:     false,
                    error:               Some(format!("session error: {}", e)),
                };
            }
        };

        let mut engine = Engine::new(model, context, tools, case_session)
            .with_metrics(MetricsCollector::new(&case.id));

        let t0 = Instant::now();
        let timeout = std::time::Duration::from_millis(case.timeout_ms);

        let run_result = tokio::time::timeout(
            timeout,
            engine.run_turn(system, &case.prompt, None),
        ).await;

        let duration_ms = t0.elapsed().as_millis() as u64;
        let timeout_exceeded = run_result.is_err(); // Elapsed = timeout error
        let error = match &run_result {
            Err(_)              => Some(format!("timeout after {}ms", case.timeout_ms)),
            Ok(Err(e))          => Some(e.to_string()),
            Ok(Ok(_))           => None,
        };

        // Collect tool names from engine's session (read the JSONL)
        let tools_used = read_tools_from_session(&engine.session.path);
        let expected_tools_found = case.expected_tools.iter().any(|et| tools_used.contains(et));

        // Record per-case metrics into the aggregate collector
        if let Some(ref m) = engine.metrics {
            for turn in m.turns() {
                collector.record(turn.clone());
            }
        }

        let passed = error.is_none() && expected_tools_found && !timeout_exceeded;

        CaseResult {
            id: case.id.clone(),
            passed,
            duration_ms,
            tools_used,
            expected_tools_found,
            timeout_exceeded,
            error,
        }
    }

    /// Compare a new run's summary against the stored baseline.
    pub fn check_regression(
        &self,
        results: &[CaseResult],
        summary: &SessionSummary,
    ) -> RegressionReport {
        let Some(baseline) = self.load_baseline() else {
            println!("{}", style("ℹ️  No baseline found — run with --update-baseline to set one").dim());
            return RegressionReport { regressions: vec![], is_regression: false };
        };

        let mut regressions = Vec::new();
        let bm = &baseline.metrics;

        macro_rules! check {
            ($label:expr, $new:expr, $old:expr, $slack:expr, $higher_is_worse:expr) => {
                let threshold = if $higher_is_worse {
                    $old * $slack
                } else {
                    $old * (2.0 - $slack) // lower is worse, slack is a floor
                };
                let regressed = if $higher_is_worse {
                    $new > threshold
                } else {
                    $new < threshold
                };
                if regressed {
                    regressions.push(format!(
                        "{}: baseline={:.2} new={:.2} (threshold={:.2})",
                        $label, $old, $new, threshold
                    ));
                }
            };
        }

        check!("P95 latency ms",       summary.p95_duration_ms as f64, bm.p95_latency_ms as f64, Thresholds::LATENCY_SLACK, true);
        check!("Healer retries",        summary.total_healer_retries as f64, bm.avg_healer_retries, Thresholds::RETRY_SLACK, true);
        check!("Compaction rate/10",    summary.compaction_rate_per_10, bm.compaction_rate_per_10, Thresholds::COMPACTION_SLACK, true);

        let tool_accuracy = results.iter().filter(|r| r.expected_tools_found).count() as f64
            / results.len() as f64 * 100.0;
        if tool_accuracy < bm.tool_accuracy_pct + Thresholds::TOOL_ACCURACY_FLOOR {
            regressions.push(format!(
                "Tool accuracy: baseline={:.1}% new={:.1}%",
                bm.tool_accuracy_pct, tool_accuracy
            ));
        }

        let is_regression = !regressions.is_empty();
        RegressionReport { regressions, is_regression }
    }

    /// Capture the current results as the new baseline.
    pub fn update_baseline(&self, results: &[CaseResult], summary: &SessionSummary) -> Result<()> {
        let tool_accuracy = results.iter().filter(|r| r.expected_tools_found).count() as f64
            / results.len().max(1) as f64 * 100.0;

        let mut case_latencies = std::collections::HashMap::new();
        for r in results {
            case_latencies.insert(r.id.clone(), r.duration_ms);
        }

        let baseline = Baseline {
            version:  chrono::Local::now().format("%Y%m%d_%H%M%S").to_string(),
            model:    self.config.active_model.clone(),
            provider: self.config.active_provider.clone(),
            metrics: BaselineMetrics {
                p95_latency_ms:       summary.p95_duration_ms,
                avg_tokens_per_turn:  0.0, // filled in Phase 3 with token counting
                tool_accuracy_pct:    tool_accuracy,
                avg_healer_retries:   summary.total_healer_retries as f64 / summary.turn_count.max(1) as f64,
                compaction_rate_per_10: summary.compaction_rate_per_10,
                error_rate:           summary.error_rate,
            },
            case_latencies,
        };

        let json = serde_json::to_string_pretty(&baseline)?;
        std::fs::write(&self.baseline_path, json)?;
        println!("{}", style(format!("✅ Baseline saved to {}", self.baseline_path.display())).green());
        Ok(())
    }

    fn print_summary(&self, summary: &SessionSummary, results: &[CaseResult]) {
        let passed  = results.iter().filter(|r| r.passed).count();
        let total   = results.len();
        let accuracy = passed as f64 / total.max(1) as f64 * 100.0;

        println!("  Passed:            {}/{}", style(passed).bold(), total);
        println!("  Tool accuracy:     {:.1}%", accuracy);
        println!("  Avg latency:       {:.0}ms", summary.avg_duration_ms);
        println!("  P95 latency:       {}ms", summary.p95_duration_ms);
        println!("  Healer retries:    {}", summary.total_healer_retries);
        println!("  Compaction/10:     {:.2}", summary.compaction_rate_per_10);
    }
}

#[derive(Debug)]
pub struct RegressionReport {
    pub regressions: Vec<String>,
    pub is_regression: bool,
}

impl RegressionReport {
    pub fn print(&self) {
        if !self.is_regression {
            println!("{}", style("✅ No regressions detected").green().bold());
        } else {
            println!("{}", style("🔴 REGRESSION DETECTED:").red().bold());
            for r in &self.regressions {
                println!("  ❌ {}", style(r).red());
            }
        }
    }
}

// ──────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────

fn print_case_result(r: &CaseResult) {
    let status = if r.passed { style("PASS").green() } else { style("FAIL").red() };
    print!("  [{}] {:30}  {:>6}ms", status, r.id, r.duration_ms);
    if !r.expected_tools_found && !r.tools_used.is_empty() {
        print!("  tools: {}", r.tools_used.join(","));
    }
    if let Some(ref e) = r.error {
        print!("  err: {}", style(e).dim());
    }
    println!();
}

/// Build a tool registry safe for headless benchmark runs.
/// Excludes BashTool (requires confirmation) and WriteFileTool (mutates state).
fn build_bench_registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();
    let sandbox = crate::tools::sandbox::SharedSandbox::new(crate::tools::sandbox::SandboxMode::Local);
    r.register(builtins::ReadFileTool::new(sandbox.clone()));
    r.register(builtins::ListDirTool::new(sandbox));
    // BashTool and WriteFileTool intentionally excluded from benchmarks
    r
}

async fn build_bench_context(
    config: &AppConfig,
    system: &str,
    tools: &[crate::tools::ToolDescriptor],
    client: &reqwest::Client,
) -> ContextManager {
    let window = crate::model::registry::resolve_context_window(config, client).await;
    let budget = ContextBudget::new(window, system, tools, config.effective_headroom());
    ContextManager::new(budget)
}

/// Read which tools were dispatched by parsing the session JSONL.
fn read_tools_from_session(path: &Path) -> Vec<String> {
    let Ok(content) = std::fs::read_to_string(path) else { return vec![] };
    let mut tools = Vec::new();
    for line in content.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if event["event"].as_str() == Some("tool_calls") {
            if let Some(arr) = event["tools"].as_array() {
                for t in arr {
                    if let Some(name) = t.as_str() {
                        if !tools.contains(&name.to_string()) {
                            tools.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    tools
}

pub async fn run_benchmark(config: &crate::config::AppConfig, update_baseline: bool) -> Result<()> {
    let project_root = std::env::current_dir()?;
    let suite_dir = project_root.join("benchmarks").join("suite");
    let baseline_path = project_root.join("benchmarks").join("baseline.json");

    let runner = BenchmarkRunner::new(&suite_dir, &baseline_path, config.clone());
    let (results, summary) = runner.run().await?;

    if update_baseline {
        runner.update_baseline(&results, &summary)?;
    } else {
        let report = runner.check_regression(&results, &summary);
        report.print();
    }
    Ok(())
}
