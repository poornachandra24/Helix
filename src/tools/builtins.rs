#![allow(clippy::items_after_test_module, clippy::collapsible_if)]

use crate::tools::Tool;
use crate::tools::sandbox::{SandboxBackend, SandboxMode, SharedSandbox};
use anyhow::Result;
use async_trait::async_trait;
use console::style;
use serde_json::{Value, json};

const OUTPUT_MAX_BYTES: usize = 8_000;

fn truncate(s: String, label: &str) -> String {
    if s.len() > OUTPUT_MAX_BYTES {
        format!(
            "{}...\n[{} TRUNCATED at {} bytes]",
            &s[..OUTPUT_MAX_BYTES],
            label,
            OUTPUT_MAX_BYTES
        )
    } else {
        s
    }
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
    fn name(&self) -> &str {
        "bash"
    }
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
    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<String> {
        let cmd = args["cmd"].as_str().unwrap_or("").trim().to_string();
        if cmd.is_empty() {
            anyhow::bail!("'cmd' parameter is required and must not be empty");
        }

        if !crate::cli::helpers::confirm_agent_action(
            "bash",
            "Execute shell command in active sandbox",
            Some(&cmd),
        )? {
            return Ok("User denied execution. Try a different approach.".into());
        }

        let mode = self.sandbox.get_mode();
        if mode == SandboxMode::Docker {
            println!(
                "🐳 {}",
                style("Running command in sandboxed Docker container (rust:latest)...").cyan()
            );
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
    fn name(&self) -> &str {
        "read_file"
    }
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
        let start = start.min(lines.len());
        let end = end_raw
            .map(|e| (e as usize).min(lines.len()))
            .unwrap_or(lines.len())
            .max(start);
        let slice = lines[start..end].join("\n");

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
    fn name(&self) -> &str {
        "write_file"
    }
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
    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<String> {
        let path = args["path"].as_str().context("'path' is required")?;
        let content = args["content"].as_str().context("'content' is required")?;

        let desc = format!("Write {} bytes to file", content.len());
        let target_details = format!(
            "Path: {}\n---\n{}",
            path,
            if content.len() > 200 {
                format!("{}...", &content[..200])
            } else {
                content.to_string()
            }
        );
        if !crate::cli::helpers::confirm_agent_action("write_file", &desc, Some(&target_details))? {
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
    fn name(&self) -> &str {
        "list_dir"
    }
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
// WasmExecuteTool
// ──────────────────────────────────────────────

pub struct WasmExecuteTool {
    #[allow(dead_code)]
    sandbox: SharedSandbox,
}

impl WasmExecuteTool {
    pub fn new(sandbox: SharedSandbox) -> Self {
        Self { sandbox }
    }
}

#[async_trait]
impl Tool for WasmExecuteTool {
    fn name(&self) -> &str {
        "wasm_execute"
    }
    fn description(&self) -> &str {
        "Execute a WebAssembly (.wasm) file within a secure, highly-sandboxed guest VM. The WASM module must be saved in the workspace first, and have an entrypoint (like 'main' or '_start'). Requires 'wasm_file' parameter."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "wasm_file": { "type": "string", "description": "The path to the compiled .wasm file to run" }
            },
            "required": ["wasm_file"]
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let wasm_file = args["wasm_file"].as_str().unwrap_or("").trim().to_string();
        if wasm_file.is_empty() {
            anyhow::bail!("'wasm_file' parameter is required");
        }

        let wasm_sandbox = crate::tools::sandbox::WasmSandbox::new();
        let res = wasm_sandbox.execute_command(&wasm_file).await?;
        Ok(format!("stdout:\n{}\nstderr:\n{}", res.stdout, res.stderr))
    }
}

// ──────────────────────────────────────────────
// AddSkillTool
// ──────────────────────────────────────────────

pub struct AddSkillTool {
    skills_dir: std::path::PathBuf,
}

impl AddSkillTool {
    pub fn new(skills_dir: std::path::PathBuf) -> Self {
        Self { skills_dir }
    }
}

#[async_trait]
impl Tool for AddSkillTool {
    fn name(&self) -> &str {
        "add_skill"
    }
    fn description(&self) -> &str {
        "Save a new domain-specific skill (rules, guidelines, checklist, or quick-start guide) to the agent's long-term memory. The skill will automatically load into the system prompt for all future sessions. Requires 'name' (valid filename without extension, lowercase letters, numbers, and hyphens only, max 64 chars), 'description' (brief summary of what the skill does and when to use it, max 1024 chars), and 'content' (detailed description or markdown content)."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "The name of the skill (lowercase letters, numbers, and hyphens only, no file extension, max 64 chars)" },
                "description": { "type": "string", "description": "Brief description of what this Skill does and when to use it (max 1024 chars)" },
                "content": { "type": "string", "description": "The markdown or text contents detailing how to perform the skill" }
            },
            "required": ["name", "description", "content"]
        })
    }
    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn call(&self, args: Value) -> Result<String> {
        let name = args["name"]
            .as_str()
            .context("'name' is required")?
            .trim()
            .to_string();
        let description = args["description"]
            .as_str()
            .context("'description' is required")?
            .trim()
            .to_string();
        let content = args["content"]
            .as_str()
            .context("'content' is required")?
            .trim()
            .to_string();

        if name.is_empty() || content.is_empty() || description.is_empty() {
            anyhow::bail!(
                "All parameters ('name', 'description', and 'content') must be non-empty"
            );
        }

        // Validate name according to Anthropic format: 1-64 chars, lowercase, numbers, and hyphens only
        if name.len() > 64
            || name
                .chars()
                .any(|c| !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '-')
        {
            anyhow::bail!(
                "Skill name must be 1-64 characters and contain only lowercase letters, numbers, and hyphens"
            );
        }

        // Cannot contain reserved words: "anthropic", "claude"
        let name_lower = name.to_lowercase();
        if name_lower.contains("anthropic") || name_lower.contains("claude") {
            anyhow::bail!("Skill name cannot contain reserved words: 'anthropic' or 'claude'");
        }

        // Validate description according to Anthropic format: 1-1024 chars, no XML tags
        if description.len() > 1024 || description.contains('<') || description.contains('>') {
            anyhow::bail!(
                "Skill description must be 1-1024 characters and cannot contain XML tags"
            );
        }

        // Store metadata in Anthropic format (YAML frontmatter at the top of the file)
        let timestamp = chrono::Local::now().to_rfc3339();
        let formatted_content = format!(
            "---\nname: {}\ndescription: {}\ncreated_at: {}\n---\n\n{}",
            name, description, timestamp, content
        );

        let desc = format!("Save a new domain-specific skill: {}", description);
        let target_details = format!(
            "Filename: {}.md\nContent Preview:\n---\n{}",
            name,
            if formatted_content.len() > 300 {
                format!("{}...", &formatted_content[..300])
            } else {
                formatted_content.to_string()
            }
        );
        if !crate::cli::helpers::confirm_agent_action("add_skill", &desc, Some(&target_details))? {
            return Ok("User denied adding skill. No changes made.".into());
        }

        let skill_file_name = format!("{}.md", name);
        let path = self.skills_dir.join(skill_file_name);

        tokio::fs::create_dir_all(&self.skills_dir).await?;
        tokio::fs::write(&path, formatted_content).await?;

        Ok(format!(
            "✅ Skill '{}' successfully saved! It will be active in all future sessions.",
            name
        ))
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
                .user_agent("helix-cli/0.1 (research agent)")
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
    fn name(&self) -> &str {
        "web_fetch"
    }
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

        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Request failed for '{}': {}", url, e))?;

        let status = resp.status();
        let body = resp.text().await?;

        if !status.is_success() {
            return Ok(format!(
                "HTTP {} for {}\n{}",
                status,
                url,
                truncate(body, "BODY")
            ));
        }

        let text = strip_html_tags(&body);
        Ok(truncate(text, "PAGE"))
    }
}

// ──────────────────────────────────────────────
// WebSearchTool
// ──────────────────────────────────────────────

pub struct WebSearchTool {
    client: reqwest::Client,
}

impl WebSearchTool {
    pub fn new() -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::USER_AGENT, reqwest::header::HeaderValue::from_static(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
        ));
        headers.insert(reqwest::header::ACCEPT, reqwest::header::HeaderValue::from_static(
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8"
        ));
        headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            reqwest::header::HeaderValue::from_static("en-US,en;q=0.9"),
        );
        headers.insert(
            reqwest::header::REFERER,
            reqwest::header::HeaderValue::from_static("https://duckduckgo.com/"),
        );

        Self {
            client: reqwest::Client::builder()
                .default_headers(headers)
                .timeout(std::time::Duration::from_secs(15))
                .build()
                .expect("Failed to build HTTP client"),
        }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
fn url_encode(s: &str) -> String {
    let mut encoded = String::new();
    for b in s.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            b' ' => {
                encoded.push('+');
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
}

fn percent_decode(s: &str) -> String {
    let mut bytes = Vec::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(c1), Some(c2)) = (h1, h2) {
                if let Ok(b) = u8::from_str_radix(&format!("{}{}", c1, c2), 16) {
                    bytes.push(b);
                    continue;
                }
            }
        } else if c == '+' {
            bytes.push(b' ');
            continue;
        }
        bytes.push(c as u8);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }
    fn description(&self) -> &str {
        "Search the web using DuckDuckGo for queries, news, or general information. Returns search result snippets and URLs."
    }
    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "The search query" }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let query = args["query"].as_str().context("'query' is required")?;

        let mut params = std::collections::HashMap::new();
        params.insert("q", query);

        let resp = self
            .client
            .post("https://html.duckduckgo.com/html/")
            .form(&params)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Search request failed for '{}': {}", query, e))?;

        let status = resp.status();
        let body = resp.text().await?;

        if !status.is_success() {
            return Ok(format!(
                "HTTP {} when searching the web for '{}'",
                status, query
            ));
        }

        // Parse DDG HTML results
        let mut results = Vec::new();
        let mut search_str = body.as_str();

        while let Some(start_idx) = search_str.find("result__body") {
            search_str = &search_str[start_idx + 12..];

            // Find result__title
            let title_start = match search_str.find("result__title") {
                Some(idx) => idx,
                None => {
                    search_str = &search_str[20..];
                    continue;
                }
            };
            let sub_str = &search_str[title_start..];

            // Find href="..."
            let href_tag = "href=\"";
            let href_start = match sub_str.find(href_tag) {
                Some(idx) => idx + href_tag.len(),
                None => {
                    search_str = &search_str[20..];
                    continue;
                }
            };
            let href_end = match sub_str[href_start..].find('"') {
                Some(idx) => href_start + idx,
                None => {
                    search_str = &search_str[20..];
                    continue;
                }
            };
            let url_raw = &sub_str[href_start..href_end];

            // Find the title text (inside the <a> tag)
            let a_close = match sub_str[href_end..].find('>') {
                Some(idx) => href_end + idx + 1,
                None => {
                    search_str = &search_str[20..];
                    continue;
                }
            };
            let a_end = match sub_str[a_close..].find("</a>") {
                Some(idx) => a_close + idx,
                None => {
                    search_str = &search_str[20..];
                    continue;
                }
            };
            let title_raw = &sub_str[a_close..a_end];
            let title = strip_html_tags(title_raw).trim().to_string();

            // Find the result__snippet
            let snippet_start = match sub_str.find("result__snippet") {
                Some(idx) => idx,
                None => {
                    search_str = &search_str[20..];
                    continue;
                }
            };
            let snippet_sub = &sub_str[snippet_start..];
            let snippet_close = match snippet_sub.find('>') {
                Some(idx) => idx + 1,
                None => {
                    search_str = &search_str[20..];
                    continue;
                }
            };
            let snippet_end = match snippet_sub[snippet_close..].find("</a>") {
                Some(idx) => snippet_close + idx,
                None => {
                    search_str = &search_str[20..];
                    continue;
                }
            };
            let snippet_raw = &snippet_sub[snippet_close..snippet_end];
            let snippet = strip_html_tags(snippet_raw).trim().to_string();

            // Clean URL (DuckDuckGo URLs are often /l/?kh=-1&uddg=ENCODED_URL)
            let clean_url = if url_raw.contains("uddg=") {
                let parts: Vec<&str> = url_raw.split("uddg=").collect();
                if parts.len() > 1 {
                    let encoded_url = parts[1].split('&').next().unwrap_or("");
                    percent_decode(encoded_url)
                } else {
                    url_raw.to_string()
                }
            } else {
                url_raw.to_string()
            };

            if !title.is_empty() && !snippet.is_empty() {
                results.push(format!(
                    "Title: {}\nURL: {}\nSnippet: {}\n",
                    title, clean_url, snippet
                ));
            }

            search_str = &search_str[20..]; // Advance past current block
            if results.len() >= 8 {
                break;
            }
        }

        if results.is_empty() {
            Ok(format!(
                "No search results found for '{}'. Try modifying the query.",
                query
            ))
        } else {
            Ok(results.join("\n---\n"))
        }
    }
}

fn strip_html_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();

    let mut in_script = false;
    let mut in_style = false;
    let mut tag_buffer = String::new();

    while let Some(c) = chars.next() {
        if c == '<' {
            tag_buffer.clear();
            let mut is_closing = false;
            if let Some(&'/') = chars.peek() {
                is_closing = true;
                chars.next();
            }
            while let Some(&tc) = chars.peek() {
                if tc == '>' || tc.is_whitespace() {
                    break;
                }
                tag_buffer.push(tc.to_ascii_lowercase());
                chars.next();
            }
            for tc in chars.by_ref() {
                if tc == '>' {
                    break;
                }
            }

            let mut added_block = false;
            if is_closing {
                if tag_buffer == "script" {
                    in_script = false;
                } else if tag_buffer == "style" {
                    in_style = false;
                } else if matches!(
                    tag_buffer.as_str(),
                    "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "tr" | "li"
                ) {
                    out.push('\n');
                    added_block = true;
                }
            } else if tag_buffer == "script" {
                in_script = true;
            } else if tag_buffer == "style" {
                in_style = true;
            } else if tag_buffer == "br"
                || matches!(
                    tag_buffer.as_str(),
                    "p" | "div" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "tr" | "li"
                )
            {
                out.push('\n');
                added_block = true;
            }

            if !added_block && !in_script && !in_style {
                out.push(' ');
            }
        } else if !in_script && !in_style {
            if c == '&' {
                let mut entity = String::new();
                while let Some(&ec) = chars.peek() {
                    if ec == ';' || entity.len() > 8 || ec.is_whitespace() {
                        break;
                    }
                    entity.push(ec);
                    chars.next();
                }
                if let Some(';') = chars.peek() {
                    chars.next();
                    match entity.as_str() {
                        "amp" => out.push('&'),
                        "lt" => out.push('<'),
                        "gt" => out.push('>'),
                        "quot" => out.push('"'),
                        "apos" | "#39" => out.push('\''),
                        "nbsp" => out.push(' '),
                        _ => {
                            out.push('&');
                            out.push_str(&entity);
                            out.push(';');
                        }
                    }
                } else {
                    out.push('&');
                    out.push_str(&entity);
                }
            } else {
                out.push(c);
            }
        }
    }

    let mut cleaned = String::new();
    let mut last_was_newline = false;
    let mut last_was_space = false;

    for c in out.chars() {
        if c == '\n' {
            if !last_was_newline {
                cleaned.push('\n');
                last_was_newline = true;
                last_was_space = false;
            }
        } else if c.is_whitespace() {
            if !last_was_space && !last_was_newline {
                cleaned.push(' ');
                last_was_space = true;
            }
        } else {
            cleaned.push(c);
            last_was_newline = false;
            last_was_space = false;
        }
    }

    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::sandbox::{SandboxMode, SharedSandbox};

    #[test]
    fn test_strip_html_tags() {
        let html = "<html><head><style>body { color: red; }</style></head><body><h1>Title</h1><p>Hello &amp; welcome to the &lt;Helix&gt; harness!&nbsp;</p><script>console.log('hi');</script><br>Goodbye.</body></html>";
        let text = strip_html_tags(html);
        assert_eq!(
            text,
            "Title\nHello & welcome to the <Helix> harness! \nGoodbye."
        );
    }

    #[tokio::test]
    async fn test_read_file_tool_range_handling() {
        let current_dir = std::env::current_dir().unwrap();
        let dir = tempfile::Builder::new().tempdir_in(&current_dir).unwrap();
        let file_path = dir.path().join("test.txt");
        let path_str = file_path.to_str().unwrap();

        let file_content = (1..=10)
            .map(|i| format!("Line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        tokio::fs::write(&file_path, file_content).await.unwrap();

        let sandbox = SharedSandbox::new(SandboxMode::Local);
        let tool = ReadFileTool::new(sandbox);

        let res = tool
            .call(json!({
                "path": path_str,
                "start_line": 2,
                "end_line": 5
            }))
            .await
            .unwrap();
        assert_eq!(res, "Line 2\nLine 3\nLine 4\nLine 5");

        let res = tool
            .call(json!({
                "path": path_str,
                "start_line": 8,
                "end_line": 4
            }))
            .await
            .unwrap();
        assert_eq!(res, "");

        let res = tool
            .call(json!({
                "path": path_str,
                "start_line": 50,
                "end_line": 60
            }))
            .await
            .unwrap();
        assert_eq!(res, "");

        let res = tool
            .call(json!({
                "path": path_str
            }))
            .await
            .unwrap();
        assert_eq!(
            res,
            (1..=10)
                .map(|i| format!("Line {}", i))
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[tokio::test]
    async fn test_web_search_tool() {
        if std::env::var("CI").is_ok() {
            println!(
                "Skipping web search test in CI environment to avoid rate-limiting/network blocks."
            );
            return;
        }

        let tool = WebSearchTool::new();
        let res = tool
            .call(json!({
                "query": "Lionel Messi"
            }))
            .await
            .unwrap();

        assert!(!res.is_empty(), "Search results should not be empty");
        assert!(
            !res.contains("No search results found"),
            "Should find results for popular query"
        );
        assert!(
            res.contains("Messi") || res.contains("messi") || res.contains("Lionel"),
            "Results should contain query term"
        );
    }
}

// Dynamic Loader Tools
// ──────────────────────────────────────────────

pub struct ListAvailableToolsAndSkillsTool {
    state: crate::tools::SharedRegistryState,
}

impl ListAvailableToolsAndSkillsTool {
    pub fn new(state: crate::tools::SharedRegistryState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for ListAvailableToolsAndSkillsTool {
    fn name(&self) -> &str {
        "list_available_tools_and_skills"
    }

    fn description(&self) -> &str {
        "List all available offline tools (e.g. MCP servers) and domain-specific skills (instructions/guidelines) that are in the registry but not currently active in the session context. You can load any of these tools/skills using the 'load_tool_or_skill' tool when needed, and you MUST unload them using the 'unload_tool_or_skill' tool once the target has been achieved to keep your context window clean."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn call(&self, _args: Value) -> Result<String> {
        let state = self.state.lock().unwrap();
        let mut output = String::new();
        output.push_str("--- AVAILABLE REGISTERED MCP TOOLS ---\n");
        if state.all_mcp_tools.is_empty() {
            output.push_str("  (None registered)\n");
        } else {
            for (name, tool) in &state.all_mcp_tools {
                let status = if state.active_tools.contains(name) {
                    "[ACTIVE]"
                } else {
                    "[UNLOADED]"
                };
                output.push_str(&format!(
                    "  - {} {}: {}\n",
                    name,
                    status,
                    tool.description()
                ));
            }
        }

        output.push_str("\n--- AVAILABLE DOMAIN-SPECIFIC SKILLS ---\n");
        if state.all_skills.is_empty() {
            output.push_str("  (None registered)\n");
        } else {
            for name in state.all_skills.keys() {
                let status = if state.active_skills.contains(name) {
                    "[ACTIVE]"
                } else {
                    "[UNLOADED]"
                };
                output.push_str(&format!("  - {} {}\n", name, status));
            }
        }

        output.push_str("\nNote: You can load any tool or skill into your context using 'load_tool_or_skill'. When you are done with the task, unload them using 'unload_tool_or_skill' to keep the context clean.");
        Ok(output)
    }
}

pub struct LoadToolOrSkillTool {
    state: crate::tools::SharedRegistryState,
}

impl LoadToolOrSkillTool {
    pub fn new(state: crate::tools::SharedRegistryState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for LoadToolOrSkillTool {
    fn name(&self) -> &str {
        "load_tool_or_skill"
    }

    fn description(&self) -> &str {
        "Load a registered tool (e.g. MCP wrapper) or skill (domain instructions) into your active context so you can use it. Returns confirmation."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "The exact name of the tool or skill to load" },
                "kind": { "type": "string", "enum": ["tool", "skill"], "description": "The type of registry item being loaded" }
            },
            "required": ["name", "kind"]
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let name = args["name"]
            .as_str()
            .context("'name' is required")?
            .trim()
            .to_string();
        let kind = args["kind"].as_str().context("'kind' is required")?.trim();

        let mut state = self.state.lock().unwrap();
        match kind {
            "tool" => {
                if !state.all_mcp_tools.contains_key(&name) {
                    anyhow::bail!("MCP tool '{}' not found in registry.", name);
                }
                if state.active_tools.insert(name.clone()) {
                    state.changed = true;
                    Ok(format!(
                        "Successfully loaded MCP tool '{}' into session context.",
                        name
                    ))
                } else {
                    Ok(format!("MCP tool '{}' is already loaded.", name))
                }
            }
            "skill" => {
                if !state.all_skills.contains_key(&name) {
                    anyhow::bail!("Skill '{}' not found in registry.", name);
                }
                if state.active_skills.insert(name.clone()) {
                    state.changed = true;
                    Ok(format!(
                        "Successfully loaded skill '{}' into session context.",
                        name
                    ))
                } else {
                    Ok(format!("Skill '{}' is already loaded.", name))
                }
            }
            _ => anyhow::bail!("Invalid kind '{}'. Must be 'tool' or 'skill'.", kind),
        }
    }
}

pub struct UnloadToolOrSkillTool {
    state: crate::tools::SharedRegistryState,
}

impl UnloadToolOrSkillTool {
    pub fn new(state: crate::tools::SharedRegistryState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl Tool for UnloadToolOrSkillTool {
    fn name(&self) -> &str {
        "unload_tool_or_skill"
    }

    fn description(&self) -> &str {
        "Unload a tool or skill from your active context once you have achieved the target for it. This cleans your prompt context window."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "The name of the tool or skill to unload" },
                "kind": { "type": "string", "enum": ["tool", "skill"], "description": "The type of registry item being unloaded" }
            },
            "required": ["name", "kind"]
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let name = args["name"]
            .as_str()
            .context("'name' is required")?
            .trim()
            .to_string();
        let kind = args["kind"].as_str().context("'kind' is required")?.trim();

        let mut state = self.state.lock().unwrap();
        match kind {
            "tool" => {
                if state.active_tools.remove(&name) {
                    state.changed = true;
                    Ok(format!(
                        "Successfully unloaded MCP tool '{}' from session context.",
                        name
                    ))
                } else {
                    Ok(format!("MCP tool '{}' was not loaded.", name))
                }
            }
            "skill" => {
                if state.active_skills.remove(&name) {
                    state.changed = true;
                    Ok(format!(
                        "Successfully unloaded skill '{}' from session context.",
                        name
                    ))
                } else {
                    Ok(format!("Skill '{}' was not loaded.", name))
                }
            }
            _ => anyhow::bail!("Invalid kind '{}'. Must be 'tool' or 'skill'.", kind),
        }
    }
}
