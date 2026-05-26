use crate::tools::Tool;
use crate::tools::sandbox::{SharedSandbox, SandboxBackend, SandboxMode};
use anyhow::Result;
use async_trait::async_trait;
use console::style;
use dialoguer::Confirm;
use serde_json::{json, Value};

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

// BashTool
// ──────────────────────────────────────────────

pub struct BashTool {
    sandbox: SharedSandbox,
}

impl BashTool {
    pub fn new(sandbox: SharedSandbox) -> Self {
        Self { sandbox }
    }
}

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

        let mode = self.sandbox.get_mode();
        if mode == SandboxMode::Docker {
            println!("🐳 {}", style("Running command in sandboxed Docker container (rust:latest)...").cyan());
        }

        let output = self.sandbox.execute_command(&cmd).await?;

        let stdout = truncate(output.stdout, "STDOUT");
        let stderr = truncate(output.stderr, "STDERR");
        let exit_code = output.exit_code;

        Ok(format!("exit_code={}\n{}{}", exit_code, stdout, stderr))
    }
}

// ──────────────────────────────────────────────
// ReadFileTool
// ──────────────────────────────────────────────

pub struct ReadFileTool {
    sandbox: SharedSandbox,
}

impl ReadFileTool {
    pub fn new(sandbox: SharedSandbox) -> Self {
        Self { sandbox }
    }
}

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
        let content = self.sandbox.read_file(path).await?;

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

pub struct WriteFileTool {
    sandbox: SharedSandbox,
}

impl WriteFileTool {
    pub fn new(sandbox: SharedSandbox) -> Self {
        Self { sandbox }
    }
}

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

        self.sandbox.write_file(path, content).await?;
        Ok(format!("✅ Written {} bytes to '{}'", content.len(), path))
    }
}

// ──────────────────────────────────────────────
// ListDirTool
// ──────────────────────────────────────────────

pub struct ListDirTool {
    sandbox: SharedSandbox,
}

impl ListDirTool {
    pub fn new(sandbox: SharedSandbox) -> Self {
        Self { sandbox }
    }
}

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
        let out = self.sandbox.list_dir(path, max_depth).await?;
        Ok(truncate(out, "LISTING"))
    }
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

        let text = strip_html_tags(&body);
        Ok(truncate(text, "PAGE"))
    }
}

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
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
