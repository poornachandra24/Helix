pub mod analyzer;
pub mod proposer;
pub mod patcher;
pub mod security;
pub mod validator;

use validator::Validator;
pub use analyzer::Analyzer;
pub use proposer::Proposer;
pub use patcher::Patcher;

use crate::config::AppConfig;
use console::style;
use std::env;
use std::fs;

pub struct EvolutionState {
    pub pending_diff: Option<String>,
    pub pending_gate_hash: Option<String>,
}

impl EvolutionState {
    pub fn new() -> Self {
        Self { pending_diff: None, pending_gate_hash: None }
    }
}

pub async fn handle_evolve(config: &AppConfig, state: &mut EvolutionState, dry_run: bool, auto_approve: bool) {
    let project_root = match env::current_dir() {
        Ok(d) => d,
        Err(e) => { println!("✘ Could not get current dir: {}", e); return; }
    };
    let sessions_dir = match crate::config::get_state_dir() {
        Ok(d) => d.join("sessions"),
        Err(e) => { println!("✘ Could not get state dir: {}", e); return; }
    };

    println!("{}", style("◆ Analyzing recent metrics...").cyan());
    let analyzer = Analyzer::new(&sessions_dir);
    let bottleneck = match analyzer.analyze_recent_metrics(20) {
        Ok(b) => b,
        Err(e) => { println!("✘ Analyzer failed: {}", e); return; }
    };
    println!("{}", style(&bottleneck).dim());

    if dry_run {
        println!("{}", style("Dry-run only. Stopping here.").yellow());
        return;
    }

    println!("{}", style("◆ Proposing improvement...").cyan());
    let baseline_path = project_root.join("benchmarks/baseline.json");
    let baseline = fs::read_to_string(&baseline_path).unwrap_or_else(|_| "No baseline".to_string());
    
    let proposer = Proposer::new(config.clone());
    let diff = match proposer.propose(&project_root, &bottleneck, &baseline).await {
        Ok(d) => d,
        Err(e) => { println!("✘ Proposer failed: {}", e); return; }
    };

    println!("{}", style("◆ Running Security Gates...").cyan());
    let validator = Validator::new(&project_root);
    let report = match validator.run_gates(&diff).await {
        Ok(r) => r,
        Err(e) => { println!("✘ Gate execution failed: {}", e); return; }
    };

    report.print();

    if report.all_pass() {
        println!("\n{}", style("Proposed Diff:").bold());
        println!("{}", style(&diff).dim());
        
        state.pending_diff = Some(diff);
        state.pending_gate_hash = Some(report.gate_hash);

        if auto_approve {
            println!("{}", style("» [Auto-Approve] Gates passed. Applying change...").green().bold());
            handle_approve(config, state).await;
        } else {
            println!("\nType {} to apply or {} to discard.", style("/approve").green(), style("/reject <reason>").red());
        }
    } else {
        println!("{}", style("Mutation rejected by automated gates.").red());
    }
}

pub async fn handle_approve(config: &AppConfig, state: &mut EvolutionState) {
    let diff = match state.pending_diff.take() {
        Some(d) => d,
        None => { println!("✘ No pending evolution diff to approve."); return; }
    };
    let gate_hash = state.pending_gate_hash.take().unwrap_or_default();

    let project_root = match env::current_dir() {
        Ok(d) => d,
        Err(e) => { println!("✘ Could not get current dir: {}", e); return; }
    };

    if let Err(e) = Patcher::apply_and_commit(config, &project_root, &diff, &gate_hash).await {
        println!("✘ {}", style(e).red());
    }
}

pub fn handle_reject(state: &mut EvolutionState, reason: &str) {
    if state.pending_diff.is_none() {
        println!("✘ No pending evolution diff to reject.");
        return;
    }
    state.pending_diff = None;
    state.pending_gate_hash = None;
    println!("{} (Reason: {})", style("Mutation discarded.").yellow(), reason);
}
