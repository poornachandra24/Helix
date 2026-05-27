/// Automated gate runner for the self-evolution loop.
///
/// ⚠️  THIS FILE IS LISTED IN `.evolution-lock` AND MAY NEVER BE MODIFIED
///     BY THE EVOLUTION LOOP. Any diff touching this file is automatically
///     rejected before it reaches the human gate.
use anyhow::Result;
use console::style;
use std::path::{Path, PathBuf};
use tokio::process::Command;

use super::security::{read_locked_files, scan_diff, SecurityScanResult};

// ──────────────────────────────────────────────
// Gate report
// ──────────────────────────────────────────────

#[derive(Debug)]
pub struct GateReport {
    pub patch_applies:   GateResult,
    pub compiles:        GateResult,
    pub clippy_clean:    GateResult,
    pub security_clean:  GateResult,
    pub locked_files_ok: GateResult,
    pub security_detail: SecurityScanResult,
    /// Stable hash of the validated diff — embedded in commit footer.
    pub gate_hash:       String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GateResult {
    Pass,
    Fail(String),
    Skipped,
}

impl GateResult {
    pub fn icon(&self) -> &'static str {
        match self {
            GateResult::Pass       => "✔",
            GateResult::Fail(_)   => "✘",
            GateResult::Skipped   => "⤳",
        }
    }
    pub fn is_pass(&self) -> bool { *self == GateResult::Pass }
}

impl GateReport {
    pub fn all_pass(&self) -> bool {
        self.patch_applies.is_pass()
            && self.compiles.is_pass()
            && self.clippy_clean.is_pass()
            && self.security_clean.is_pass()
            && self.locked_files_ok.is_pass()
    }

    pub fn print(&self) {
        println!("\n{}", style("── Evolution Gate Results ─────────────────────").bold());
        print_gate("Patch applies cleanly", &self.patch_applies);
        print_gate("cargo build",           &self.compiles);
        print_gate("cargo clippy",          &self.clippy_clean);
        print_gate("Security scan",         &self.security_clean);
        print_gate("Locked files untouched",&self.locked_files_ok);

        if !self.security_detail.is_clean() {
            println!("\n{}", style("Security violations:").red().bold());
            println!("{}", self.security_detail.report());
        }

        if self.all_pass() {
            println!("\n{}", style("All gates passed ✔").green().bold());
            println!("Gate hash: {}", style(&self.gate_hash).dim());
        } else {
            println!("\n{}", style("One or more gates failed — diff rejected ✘").red().bold());
        }
        println!("{}", style("────────────────────────────────────────────────").bold());
    }
}

fn print_gate(label: &str, result: &GateResult) {
    match result {
        GateResult::Pass        => println!("  {} {}", result.icon(), style(label).green()),
        GateResult::Skipped     => println!("  {} {} (skipped)", result.icon(), style(label).dim()),
        GateResult::Fail(msg)   => {
            println!("  {} {}", result.icon(), style(label).red());
            for line in msg.lines().take(10) {
                println!("      {}", style(line).dim());
            }
        }
    }
}

// ──────────────────────────────────────────────
// Validator
// ──────────────────────────────────────────────

pub struct Validator {
    project_root: PathBuf,
}

impl Validator {
    pub fn new(project_root: &Path) -> Self {
        Self { project_root: project_root.to_path_buf() }
    }

    /// Run all automated gates against a proposed unified diff.
    ///
    /// The diff is applied to a temp directory. The temp directory is
    /// cleaned up on success or failure.
    pub async fn run_gates(&self, diff: &str) -> Result<GateReport> {
        // Gate 1 — Security scan (cheap, run first)
        let locked_files = read_locked_files(&self.project_root);
        let security_detail = scan_diff(diff, &locked_files);

        let security_clean = if security_detail.violations.is_empty() {
            GateResult::Pass
        } else {
            GateResult::Fail(security_detail.report())
        };

        let locked_files_ok = if security_detail.locked_file_touches.is_empty() {
            GateResult::Pass
        } else {
            GateResult::Fail(format!(
                "Diff touches locked files: {}",
                security_detail.locked_file_touches.join(", ")
            ))
        };

        // If security gates fail, skip expensive compile gates
        if !security_clean.is_pass() || !locked_files_ok.is_pass() {
            let gate_hash = compute_gate_hash(diff);
            return Ok(GateReport {
                patch_applies: GateResult::Skipped,
                compiles:      GateResult::Skipped,
                clippy_clean:  GateResult::Skipped,
                security_clean,
                locked_files_ok,
                security_detail,
                gate_hash,
            });
        }

        // Gate 2 — Apply patch to temp directory
        let temp_dir = tempfile::Builder::new()
            .prefix("harness-evolve-")
            .tempdir()?;
        let temp_path = temp_dir.path();

        let patch_applies = match apply_patch_to_temp(&self.project_root, temp_path, diff).await {
            Ok(())  => GateResult::Pass,
            Err(e)  => GateResult::Fail(e.to_string()),
        };

        // Gate 3 — Compile in temp dir (only if patch applied)
        let compiles = if patch_applies.is_pass() {
            match run_cargo(temp_path, &["build", "--quiet"]).await {
                Ok(_)   => GateResult::Pass,
                Err(e)  => GateResult::Fail(e),
            }
        } else {
            GateResult::Skipped
        };

        // Gate 4 — Clippy in temp dir (only if compiled)
        let clippy_clean = if compiles.is_pass() {
            match run_cargo(temp_path, &["clippy", "--", "-D", "warnings"]).await {
                Ok(_)   => GateResult::Pass,
                Err(e)  => GateResult::Fail(e),
            }
        } else {
            GateResult::Skipped
        };

        let gate_hash = compute_gate_hash(diff);

        Ok(GateReport {
            patch_applies,
            compiles,
            clippy_clean,
            security_clean,
            locked_files_ok,
            security_detail,
            gate_hash,
        })
    }
}

// ──────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────

/// Copy the project to `dest` then apply `diff` via `patch -p1`.
async fn apply_patch_to_temp(src: &Path, dest: &Path, diff: &str) -> Result<()> {
    // Copy only source files (skip target/ which is huge)
    let status = Command::new("rsync")
        .args(["-a", "--exclude=target/", "--exclude=.git/"])
        .arg(format!("{}/", src.display()))
        .arg(dest.display().to_string())
        .status()
        .await?;

    if !status.success() {
        anyhow::bail!("rsync failed with status {}", status);
    }

    // Apply the patch
    let output = Command::new("patch")
        .args(["-p1", "--dry-run"])
        .current_dir(dest)
        .stdin(std::process::Stdio::piped())
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("patch --dry-run failed:\n{}", stderr);
    }

    // Real apply
    let mut child = Command::new("patch")
        .args(["-p1"])
        .current_dir(dest)
        .stdin(std::process::Stdio::piped())
        .spawn()?;

    if let Some(stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let mut stdin = stdin;
        stdin.write_all(diff.as_bytes()).await?;
    }

    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("patch apply failed");
    }

    Ok(())
}

/// Run `cargo <args>` in a directory. Returns stderr on failure.
async fn run_cargo(dir: &Path, args: &[&str]) -> std::result::Result<(), String> {
    let output = Command::new("cargo")
        .args(args)
        .current_dir(dir)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        Err(stderr)
    }
}

/// Compute a stable SHA-256 hex hash of the diff to embed in commit messages.
fn compute_gate_hash(diff: &str) -> String {
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;

    let mut hasher = DefaultHasher::new();
    diff.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}
