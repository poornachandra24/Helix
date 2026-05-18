use crate::tools::Tool;
use anyhow::Result;
use async_trait::async_trait;
use console::style;
use dialoguer::Confirm;
use serde_json::{json, Value};
use std::path::Path;
use tokio::process::Command;

const OUTPUT_MAX_BYTES: usize = 8_000;

fn truncate(s: String, label: &str) -> String {
    if s.len() > OUTPUT_MAX_BYTES {
        format!("{}...\n[{} TRUNCATED at {} bytes]", &s[..OUTPUT_MAX_BYTES], label, OUTPUT_MAX_BYTES)
    } else {
        s
    }
}

fn confirm_action(description: &str) -> Result<bool> {
    println!("\n⚠️   Agent wants to: {}", style(description).bold().yellow());
    Ok(Confirm::new()
        .with_prompt("Allow?")
        .default(false)
        .interact()?)
}

// ──────────────────────────────────────────────
// BashTool
// ──────────────────────────────────────────────

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str { "bash" }
    fn description(&self) -> &str {
        "Execute a bash shell command. Use for file operations, running programs, or system queries. Requires 'cmd' parameter."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cmd": { "type": "string", "description": "The bash command to execute" }
            },
            "required": ["cmd"]
        })
    }
    fn requires_confirmation(&self) -> bool { true }

    async fn call(&self, args: Value) -> Result<String> {
        let cmd = args["cmd"].as_str().unwrap_or("").trim().to_string();
        if cmd.is_empty() {
            anyhow::bail!("'cmd' parameter is required and must not be empty");
        }

        if !confirm_action(&format!("run bash: {}", cmd))? {
            return Ok("User denied execution. Try a different approach.".into());
        }

        tracing::debug!(cmd = %cmd, "Executing bash command");

        let output = Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
            .await?;

        let stdout = truncate(String::from_utf8_lossy(&output.stdout).into_owned(), "STDOUT");
        let stderr = truncate(String::from_utf8_lossy(&output.stderr).into_owned(), "STDERR");
        let exit_code = output.status.code().unwrap_or(-1);

        Ok(format!("exit_code={}\n{}{}", exit_code, stdout, stderr))
    }
}

// ──────────────────────────────────────────────
// ReadFileTool
// ──────────────────────────────────────────────

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str { "read_file" }
    fn description(&self) -> &str {
        "Read the contents of a file. Optionally specify start/end line numbers (1-indexed, inclusive). Returns raw text."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path":       { "type": "string",  "description": "Absolute or relative file path" },
                "start_line": { "type": "integer", "description": "First line to read (default: 1)" },
                "end_line":   { "type": "integer", "description": "Last line to read (default: all)" }
            },
            "required": ["path"]
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let path = args["path"].as_str().context("'path' is required")?;
        let content = tokio::fs::read_to_string(path).await
            .map_err(|e| anyhow::anyhow!("Cannot read '{}': {}", path, e))?;

        let start = args["start_line"].as_u64().unwrap_or(1).saturating_sub(1) as usize;
        let end_raw = args["end_line"].as_u64();

        let lines: Vec<&str> = content.lines().collect();
        let end = end_raw.map(|e| (e as usize).min(lines.len())).unwrap_or(lines.len());
        let slice = lines[start.min(lines.len())..end].join("\n");

        Ok(truncate(slice, "FILE"))
    }
}

use anyhow::Context as _;

// ──────────────────────────────────────────────
// WriteFileTool
// ──────────────────────────────────────────────

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str { "write_file" }
    fn description(&self) -> &str {
        "Write (or overwrite) a file with the given content. Creates parent directories as needed."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path":    { "type": "string", "description": "File path to write to" },
                "content": { "type": "string", "description": "Text content to write" }
            },
            "required": ["path", "content"]
        })
    }
    fn requires_confirmation(&self) -> bool { true }

    async fn call(&self, args: Value) -> Result<String> {
        let path = args["path"].as_str().context("'path' is required")?;
        let content = args["content"].as_str().context("'content' is required")?;

        if !confirm_action(&format!("write {} bytes to '{}'", content.len(), path))? {
            return Ok("User denied write. No changes made.".into());
        }

        if let Some(parent) = Path::new(path).parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, content).await?;
        Ok(format!("✅ Written {} bytes to '{}'", content.len(), path))
    }
}

// ──────────────────────────────────────────────
// ListDirTool
// ──────────────────────────────────────────────

pub struct ListDirTool;

#[async_trait]
impl Tool for ListDirTool {
    fn name(&self) -> &str { "list_dir" }
    fn description(&self) -> &str {
        "List the contents of a directory, showing names, types (file/dir), and sizes."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path":  { "type": "string",  "description": "Directory path (default: '.')" },
                "depth": { "type": "integer", "description": "Max recursion depth (default: 1)" }
            },
            "required": []
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let path = args["path"].as_str().unwrap_or(".");
        let max_depth = args["depth"].as_u64().unwrap_or(1) as usize;
        let mut out = String::new();
        list_dir_recursive(Path::new(path), 0, max_depth, &mut out)?;
        Ok(truncate(out, "LISTING"))
    }
}

fn list_dir_recursive(dir: &Path, depth: usize, max_depth: usize, out: &mut String) -> Result<()> {
    let indent = "  ".repeat(depth);
    let entries = std::fs::read_dir(dir)?;
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // Skip hidden and build artifacts
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            out.push_str(&format!("{}📁 {}/\n", indent, name));
            if depth < max_depth {
                list_dir_recursive(&path, depth + 1, max_depth, out)?;
            }
        } else {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push_str(&format!("{}📄 {} ({}B)\n", indent, name, size));
        }
    }
    Ok(())
}

// ──────────────────────────────────────────────
// WebFetchTool
// ──────────────────────────────────────────────

pub struct WebFetchTool {
    client: reqwest::Client,
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("harness-cli/0.1 (research agent)")
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("Failed to build HTTP client"),
        }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str { "web_fetch" }
    fn description(&self) -> &str {
        "Fetch a URL and return its plain-text content. Useful for reading documentation, APIs, or web pages."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "The URL to fetch" }
            },
            "required": ["url"]
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let url = args["url"].as_str().context("'url' is required")?;
        tracing::debug!(url = %url, "Fetching URL");

        let resp = self.client.get(url).send().await
            .map_err(|e| anyhow::anyhow!("Request failed for '{}': {}", url, e))?;

        let status = resp.status();
        let body = resp.text().await?;

        if !status.is_success() {
            return Ok(format!("HTTP {} for {}\n{}", status, url, truncate(body, "BODY")));
        }

        // Strip HTML tags with a simple pass (no extra dep needed for basic use)
        let text = strip_html_tags(&body);
        Ok(truncate(text, "PAGE"))
    }
}

/// Minimal HTML stripper — removes tags, decodes common entities.
fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    // Collapse whitespace
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
