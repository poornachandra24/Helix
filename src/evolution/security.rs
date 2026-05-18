/// Security policy for the self-evolution loop.
///
/// ⚠️  THIS FILE IS LISTED IN `.evolution-lock` AND MAY NEVER BE MODIFIED
///     BY THE EVOLUTION LOOP. Any diff touching this file is automatically
///     rejected by the validator before it reaches the human gate.
///
/// If a rule needs to change, a human must edit this file directly and
/// commit it outside the evolution loop.
use std::path::Path;

// ──────────────────────────────────────────────
// Forbidden pattern definition
// ──────────────────────────────────────────────

pub struct ForbiddenPattern {
    /// Plain-text substring to scan for in `+` diff lines.
    /// We use substring matching (not regex) to avoid false negatives from
    /// clever escaping or regex-injection in LLM-generated code.
    pub pattern: &'static str,
    /// Human-readable explanation shown in the gate report.
    pub reason: &'static str,
    /// Whether this is a hard block (true) or a warning that still allows
    /// human to override. Currently all patterns are hard blocks.
    pub hard_block: bool,
}

/// The canonical list of patterns that the evolution loop may never introduce.
///
/// These are checked against every `+` (addition) line in a proposed diff.
/// Removal lines (`-`) are not checked — removing dangerous code is always fine.
pub const FORBIDDEN_PATTERNS: &[ForbiddenPattern] = &[
    // ── Execution escalation ────────────────────────────────────
    ForbiddenPattern {
        pattern:    "unsafe {",
        reason:     "New unsafe block — Rust safety guarantees must be preserved",
        hard_block: true,
    },
    ForbiddenPattern {
        pattern:    "unsafe{",
        reason:     "New unsafe block (no space variant)",
        hard_block: true,
    },
    ForbiddenPattern {
        pattern:    "std::process::exit",
        reason:     "Process termination — only the REPL exit path is permitted",
        hard_block: true,
    },
    ForbiddenPattern {
        pattern:    "std::process::Command",
        reason:     "Raw process spawn — use builtins::BashTool which has confirmation gates",
        hard_block: true,
    },
    // ── File system danger ───────────────────────────────────────
    ForbiddenPattern {
        pattern:    "fs::remove_",
        reason:     "File/directory deletion — not permitted in evolution patches",
        hard_block: true,
    },
    ForbiddenPattern {
        pattern:    "remove_dir_all",
        reason:     "Recursive directory removal",
        hard_block: true,
    },
    // ── Secret exfiltration ──────────────────────────────────────
    ForbiddenPattern {
        pattern:    "api_key",
        reason:     "References to api_key in new code risk secret leakage in logs — \
                     use provider.api_key only within model.rs bearer_auth call sites",
        hard_block: true,
    },
    // ── Self-referential bypass ──────────────────────────────────
    ForbiddenPattern {
        pattern:    "evolution::apply",
        reason:     "The evolution loop cannot call itself to apply changes",
        hard_block: true,
    },
    ForbiddenPattern {
        pattern:    "FORBIDDEN_PATTERNS",
        reason:     "The security policy cannot be modified by the evolution loop",
        hard_block: true,
    },
    ForbiddenPattern {
        pattern:    "evolution-lock",
        reason:     "The locked file list cannot be modified by the evolution loop",
        hard_block: true,
    },
    // ── Network escalation ───────────────────────────────────────
    ForbiddenPattern {
        pattern:    "reqwest::Client::new()",
        reason:     "New HTTP client instances must go through the established client builders",
        hard_block: true,
    },
    // ── Dependency mutation ──────────────────────────────────────
    ForbiddenPattern {
        pattern:    "[dependencies]",
        reason:     "Cargo dependencies cannot be mutated by the evolution loop",
        hard_block: true,
    },
];

// ──────────────────────────────────────────────
// Lock file
// ──────────────────────────────────────────────

/// Read the `.evolution-lock` file from the project root.
/// Returns the list of file paths that may never be touched by a diff.
pub fn read_locked_files(project_root: &Path) -> Vec<String> {
    let lock_path = project_root.join(".evolution-lock");
    match std::fs::read_to_string(&lock_path) {
        Ok(content) => content
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(String::from)
            .collect(),
        Err(e) => {
            tracing::warn!("Could not read .evolution-lock: {} — treating all files as unlocked", e);
            vec![]
        }
    }
}

// ──────────────────────────────────────────────
// Diff scanner
// ──────────────────────────────────────────────

/// Result of scanning a proposed diff for security violations.
#[derive(Debug)]
pub struct SecurityScanResult {
    pub violations: Vec<SecurityViolation>,
    pub locked_file_touches: Vec<String>,
}

#[derive(Debug)]
pub struct SecurityViolation {
    pub line_number: usize,
    pub pattern: &'static str,
    pub reason: &'static str,
    pub line_content: String,
}

impl SecurityScanResult {
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty() && self.locked_file_touches.is_empty()
    }

    pub fn report(&self) -> String {
        if self.is_clean() {
            return "✅ Security scan: clean".to_string();
        }
        let mut out = String::new();
        for v in &self.violations {
            out.push_str(&format!(
                "❌ Line {}: [{}] — {}\n   > {}\n",
                v.line_number, v.pattern, v.reason, v.line_content.trim()
            ));
        }
        for f in &self.locked_file_touches {
            out.push_str(&format!("🔒 Locked file touched: {}\n", f));
        }
        out
    }
}

/// Scan a unified diff for forbidden patterns and locked-file touches.
///
/// Only `+` lines (additions) are scanned for forbidden patterns.
/// File headers (`+++ b/...`) are checked against the lock list.
pub fn scan_diff(diff: &str, locked_files: &[String]) -> SecurityScanResult {
    let mut violations = Vec::new();
    let mut locked_file_touches = Vec::new();

    for (i, line) in diff.lines().enumerate() {
        let line_no = i + 1;

        // Check for locked file in diff header
        if line.starts_with("+++ b/") {
            let path = line.trim_start_matches("+++ b/");
            for locked in locked_files {
                if path == locked || path.ends_with(locked.as_str()) {
                    locked_file_touches.push(locked.clone());
                }
            }
            continue;
        }

        // Only scan addition lines for forbidden patterns
        if !line.starts_with('+') || line.starts_with("+++") {
            continue;
        }

        let content = &line[1..]; // strip the leading `+`
        for pat in FORBIDDEN_PATTERNS {
            if pat.hard_block && content.contains(pat.pattern) {
                violations.push(SecurityViolation {
                    line_number: line_no,
                    pattern: pat.pattern,
                    reason: pat.reason,
                    line_content: line.to_string(),
                });
            }
        }
    }

    SecurityScanResult { violations, locked_file_touches }
}
