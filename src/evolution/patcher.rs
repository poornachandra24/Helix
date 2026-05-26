use anyhow::Result;
use console::style;
use std::path::Path;
use tokio::process::Command;

use crate::cli::bench::BenchmarkRunner;
use crate::config::AppConfig;

pub struct Patcher;

impl Patcher {
    pub async fn apply_and_commit(
        config: &AppConfig,
        project_root: &Path,
        diff: &str,
        gate_hash: &str,
    ) -> Result<()> {
        println!("{}", style("Applying patch...").cyan());
        
        let mut child = Command::new("patch")
            .args(["-p1"])
            .current_dir(project_root)
            .stdin(std::process::Stdio::piped())
            .spawn()?;

        if let Some(stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let mut stdin = stdin;
            stdin.write_all(diff.as_bytes()).await?;
        }
        let status = child.wait().await?;
        if !status.success() {
            anyhow::bail!("Failed to apply patch to working directory");
        }

        println!("{}", style("Building release binary...").cyan());
        let build = Command::new("cargo")
            .args(["build", "--release"])
            .current_dir(project_root)
            .output()
            .await?;
            
        if !build.status.success() {
            // Revert changes
            let _ = Command::new("git").args(["checkout", "."]).current_dir(project_root).output().await;
            anyhow::bail!("Release build failed after patching. Reverted.");
        }

        // Commit it locally first
        let commit_msg = format!("evolve(auto): self-evolution improvement\n\nGate-Hash: {}", gate_hash);
        let _ = Command::new("git").args(["add", "."]).current_dir(project_root).output().await?;
        let _ = Command::new("git").args(["commit", "-m", &commit_msg]).current_dir(project_root).output().await?;

        println!("{}", style("Running post-deploy benchmark...").cyan());
        let suite_dir = project_root.join("benchmarks/suite");
        let baseline_path = project_root.join("benchmarks/baseline.json");
        let runner = BenchmarkRunner::new(&suite_dir, &baseline_path, config.clone());
        
        let (results, summary) = runner.run().await?;
        let report = runner.check_regression(&results, &summary);
        report.print();

        if report.is_regression {
            println!("{}", style("Regression detected! Auto-reverting...").red().bold());
            let _ = Command::new("git").args(["revert", "HEAD", "--no-edit"]).current_dir(project_root).output().await?;
            anyhow::bail!("Mutation reverted due to benchmark regression.");
        }

        println!("{}", style("Evolution cycle complete! Baseline updated.").green().bold());
        runner.update_baseline(&results, &summary)?;

        Ok(())
    }
}
