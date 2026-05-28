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

    let mut current_file = String::new();
    let mut file_additions = std::collections::HashMap::new();

    for (i, line) in diff.lines().enumerate() {
        let line_no = i + 1;

        // Check for locked file in diff header
        if line.starts_with("+++ b/") {
            let path = line.trim_start_matches("+++ b/").to_string();
            current_file = path.clone();
            for locked in locked_files {
                if path == *locked || path.ends_with(locked.as_str()) {
                    locked_file_touches.push(locked.clone());
                }
            }
            continue;
        }

        // Only accumulate addition lines for forbidden patterns
        if !line.starts_with('+') || line.starts_with("+++") {
            continue;
        }

        let content = &line[1..]; // strip the leading `+`
        file_additions
            .entry(current_file.clone())
            .or_insert_with(Vec::new)
            .push((line_no, content.to_string()));
    }

    // Now, scan the accumulated additions for each file
    for (file_path, additions) in file_additions {
        // Construct the full additions block
        let mut full_additions = String::new();
        for (_, content) in &additions {
            full_additions.push_str(content);
            full_additions.push('\n');
        }

        // Preprocess to strip all comments and whitespace
        let cleaned = preprocess_string(&full_additions);

        // Check against patterns
        for pat in FORBIDDEN_PATTERNS {
            // Clean the pattern itself to match
            let clean_pat = preprocess_string(pat.pattern);
            if pat.hard_block && (cleaned.contains(&clean_pat) || full_additions.contains(pat.pattern)) {
                // Find the first line number that contributed to this block
                let first_line_no = additions.first().map(|(n, _)| *n).unwrap_or(0);
                violations.push(SecurityViolation {
                    line_number: first_line_no,
                    pattern: pat.pattern,
                    reason: pat.reason,
                    line_content: format!("File: {} (detected pattern '{}' in additions)", file_path, pat.pattern),
                });
            }
        }
    }

    SecurityScanResult { violations, locked_file_touches }
}

fn preprocess_string(s: &str) -> String {
    let mut cleaned = String::new();
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if in_line_comment {
            if c == '\n' {
                in_line_comment = false;
            }
        } else if in_block_comment {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block_comment = false;
            }
        } else {
            if c == '/' && chars.peek() == Some(&'/') {
                chars.next();
                in_line_comment = true;
            } else if c == '/' && chars.peek() == Some(&'*') {
                chars.next();
                in_block_comment = true;
            } else if !c.is_whitespace() {
                cleaned.push(c);
            }
        }
    }
    cleaned
}

