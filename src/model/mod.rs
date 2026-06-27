//! # Model Adapters & Provider Integration Layer
//!
//! This module provides abstractions and adapters to interact with OpenAI-compatible API providers,
//! Ollama native endpoints, and other LLMs. It defines the [`ModelAdapter`] trait and handles request
//! payload generation, response parsing (including streaming and Markdown fallback), and reasoning parameters.

pub mod registry;

use anyhow::Result;
use async_trait::async_trait;
use futures_util::StreamExt;

use reqwest::Client;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{ApiFormat, AppConfig, Provider};
use crate::tools::ToolDescriptor;

// ──────────────────────────────────────────────
// Domain types
// ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ToolCall {
    /// The tool_call_id from the API response (for structured tool-result messages).
    pub id: String,
    pub name: String,
    pub args: Value,
}

#[derive(Debug)]
pub enum ModelResponse {
    /// One or more tool invocations requested by the model.
    ToolCalls(Vec<ToolCall>, Value),
    /// The model has finished and returned a final answer.
    EndTurn(String),
    /// The model returned malformed tool-call JSON — triggers the Local Healer.
    ParseError { raw_text: String, error: String },
}

// ──────────────────────────────────────────────
// ModelAdapter trait
// ──────────────────────────────────────────────

/// Abstraction over any LLM backend.
/// Implement this trait to add a new provider without touching `Engine`.
#[async_trait]
pub trait ModelAdapter: Send + Sync {
    /// Send a chat request and return a structured response.
    /// If `stream_tx` is `Some`, text tokens are sent through the channel
    /// as they arrive (only for `EndTurn` responses; tool calls are not streamed).
    async fn call(
        &self,
        system_prompt: &str,
        messages: &[Value],
        tools: Vec<ToolDescriptor>,
        stream_tx: Option<UnboundedSender<String>>,
    ) -> Result<ModelResponse>;

    fn provider_name(&self) -> &str;
    fn model_name(&self) -> &str;
    fn set_thinking_level(&self, _level: Option<String>) {}
}

// ──────────────────────────────────────────────
// OpenAI-compatible adapter (default)
// ──────────────────────────────────────────────

pub struct OpenAiCompatibleAdapter {
    pub config: AppConfig,
    client: Client,
    resolved_provider: tokio::sync::Mutex<Option<Provider>>,
    thinking_level: std::sync::RwLock<Option<String>>,
}

impl OpenAiCompatibleAdapter {
    pub fn new(config: AppConfig) -> Self {
        let level = config.thinking_level.clone();
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            config,
            client,
            resolved_provider: tokio::sync::Mutex::new(None),
            thinking_level: std::sync::RwLock::new(level),
        }
    }

    /// Build the full endpoint URL based on the provider's `ApiFormat`.
    fn endpoint(&self, provider_base: &str, format: &ApiFormat) -> String {
        let base = provider_base.trim_end_matches('/');
        match format {
            ApiFormat::OpenAiCompatible => {
                if base.ends_with("/chat/completions") {
                    base.to_string()
                } else {
                    format!("{}/chat/completions", base)
                }
            }
            ApiFormat::OllamaNative => {
                if base.ends_with("/api/chat") {
                    base.to_string()
                } else if base.ends_with("/api") {
                    format!("{}/chat", base)
                } else {
                    format!("{}/api/chat", base)
                }
            }
        }
    }

    /// Build the request payload, adapting field names for each format.
    fn build_payload(
        &self,
        system_prompt: &str,
        messages: &[Value],
        tools: Vec<ToolDescriptor>,
        format: &ApiFormat,
        streaming: bool,
    ) -> Value {
        match format {
            ApiFormat::OpenAiCompatible => {
                let mut msgs = vec![json!({"role": "system", "content": system_prompt})];
                for m in messages {
                    let mut modified = m.clone();
                    if let Some(tool_calls) = modified
                        .get_mut("tool_calls")
                        .and_then(|v| v.as_array_mut())
                    {
                        for tc in tool_calls {
                            if let Some(func) = tc.get_mut("function")
                                && let Some(args) = func.get("arguments")
                                && args.is_object()
                            {
                                func["arguments"] = json!(args.to_string());
                            }
                        }
                    }
                    msgs.push(modified);
                }

                let openai_tools: Vec<Value> = tools
                    .into_iter()
                    .map(|t| {
                        json!({
                            "type": "function",
                            "function": {
                                "name": t.name,
                                "description": t.description,
                                "parameters": t.parameters,
                            }
                        })
                    })
                    .collect();

                let mut payload = json!({
                    "model":       self.config.active_model,
                    "messages":    msgs,
                    "tools":       openai_tools,
                    "temperature": 0.1,
                    "stream":      streaming,
                });

                let active_level = self.thinking_level.read().ok().and_then(|r| r.clone());
                if let Some(level) = &active_level {
                    let model_lower = self.config.active_model.to_lowercase();
                    let provider_lower = self.config.active_provider.to_lowercase();
                    let is_gemini =
                        model_lower.contains("gemini") || provider_lower.contains("gemini");
                    if !is_gemini {
                        let level_lower = level.to_lowercase();
                        if level_lower == "off" || level_lower == "disabled" {
                            payload["reasoning_effort"] = json!("low");
                            payload["thinking_config"] = json!({ "thinking_budget": 0 });
                        } else if level_lower == "low" {
                            payload["reasoning_effort"] = json!("low");
                            payload["thinking_config"] = json!({ "thinking_budget": 1024 });
                        } else if level_lower == "medium" {
                            payload["reasoning_effort"] = json!("medium");
                            payload["thinking_config"] = json!({ "thinking_budget": 4096 });
                        } else if level_lower == "high" {
                            payload["reasoning_effort"] = json!("high");
                            payload["thinking_config"] = json!({ "thinking_budget": 16384 });
                        } else if let Ok(budget) = level.parse::<u64>() {
                            payload["thinking_config"] = json!({ "thinking_budget": budget });
                            let effort = if budget < 2048 {
                                "low"
                            } else if budget < 8192 {
                                "medium"
                            } else {
                                "high"
                            };
                            payload["reasoning_effort"] = json!(effort);
                        }
                    }
                }

                payload
            }
            ApiFormat::OllamaNative => {
                let mut msgs = vec![json!({"role": "system", "content": system_prompt})];
                for m in messages {
                    let mut modified = m.clone();
                    if let Some(tool_calls) = modified
                        .get_mut("tool_calls")
                        .and_then(|v| v.as_array_mut())
                    {
                        for tc in tool_calls {
                            if let Some(func) = tc.get_mut("function")
                                && let Some(args_str) =
                                    func.get("arguments").and_then(|v| v.as_str())
                                && let Ok(obj) = serde_json::from_str::<Value>(args_str)
                            {
                                func["arguments"] = obj;
                            }
                        }
                    }
                    msgs.push(modified);
                }

                // Ollama native tool format mirrors OpenAI's tool schema
                let ollama_tools: Vec<Value> = tools
                    .into_iter()
                    .map(|t| {
                        json!({
                            "type": "function",
                            "function": {
                                "name": t.name,
                                "description": t.description,
                                "parameters": t.parameters,
                            }
                        })
                    })
                    .collect();

                let mut payload = json!({
                    "model":    self.config.active_model,
                    "messages": msgs,
                    "tools":    ollama_tools,
                    "stream":   streaming,
                    "options":  { "temperature": 0.1 },
                });

                let active_level = self.thinking_level.read().ok().and_then(|r| r.clone());
                if let (Some(level), Some(opts)) =
                    (&active_level, payload["options"].as_object_mut())
                {
                    let level_lower = level.to_lowercase();
                    if level_lower == "off" || level_lower == "disabled" {
                        opts.insert("thinking_budget".to_string(), json!(0));
                    } else if level_lower == "low" {
                        opts.insert("thinking_budget".to_string(), json!(1024));
                    } else if level_lower == "medium" {
                        opts.insert("thinking_budget".to_string(), json!(4096));
                    } else if level_lower == "high" {
                        opts.insert("thinking_budget".to_string(), json!(16384));
                    } else if let Ok(budget) = level.parse::<u64>() {
                        opts.insert("thinking_budget".to_string(), json!(budget));
                    }
                }

                payload
            }
        }
    }

    /// Parse a complete (non-streaming) response from either format.
    fn parse_response(&self, resp: &Value, format: &ApiFormat) -> Result<ModelResponse> {
        if let Some(err) = resp.get("error") {
            let msg = err["message"]
                .as_str()
                .or_else(|| err.as_str())
                .unwrap_or("unknown API error");
            anyhow::bail!("API error: {}", msg);
        }

        let message = match format {
            ApiFormat::OpenAiCompatible => &resp["choices"][0]["message"],
            ApiFormat::OllamaNative => &resp["message"],
        };

        // ── Tool calls ────────────────────────────────────────────
        if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array())
            && !tool_calls.is_empty()
        {
            let mut calls = Vec::new();
            for tc in tool_calls {
                let func = &tc["function"];

                let args = if let Some(args_str) = func["arguments"].as_str() {
                    match serde_json::from_str::<Value>(args_str) {
                        Ok(v) => v,
                        Err(e) => {
                            return Ok(ModelResponse::ParseError {
                                raw_text: args_str.to_string(),
                                error: format!("Failed to parse tool args: {}", e),
                            });
                        }
                    }
                } else if func["arguments"].is_object() {
                    func["arguments"].clone()
                } else {
                    json!({})
                };

                calls.push(ToolCall {
                    id: tc["id"].as_str().unwrap_or("0").to_string(),
                    name: func["name"].as_str().unwrap_or("").to_string(),
                    args,
                });
            }
            if !calls.is_empty() {
                let normalized_message = match format {
                    ApiFormat::OpenAiCompatible => message.clone(),
                    ApiFormat::OllamaNative => {
                        let tool_calls_json: Vec<Value> = calls
                            .iter()
                            .map(|c| {
                                json!({
                                    "id": c.id,
                                    "type": "function",
                                    "function": { "name": c.name, "arguments": c.args.to_string() }
                                })
                            })
                            .collect();
                        json!({
                            "role": "assistant",
                            "content": message.get("content").cloned().unwrap_or(Value::Null),
                            "tool_calls": tool_calls_json
                        })
                    }
                };
                return Ok(ModelResponse::ToolCalls(calls, normalized_message));
            }
        }

        // ── Text content ──────────────────────────────────────────
        let text = message["content"].as_str().unwrap_or("").to_string();

        // Markdown fallback: extract tool call from ```json block
        if let Some(resp) = self.try_parse_markdown_tool_call(&text) {
            return Ok(resp);
        }

        Ok(ModelResponse::EndTurn(text))
    }

    /// Extract a tool call from a ```json ... ``` block in model output.
    fn try_parse_markdown_tool_call(&self, text: &str) -> Option<ModelResponse> {
        if !text.contains("```json") {
            return None;
        }
        let start = text.find("```json")? + 7;
        let end = text[start..].find("```")?;
        let json_str = &text[start..start + end];

        match serde_json::from_str::<Value>(json_str) {
            Ok(parsed) => {
                let name = parsed.get("name")?.as_str()?.to_string();
                let args = parsed
                    .get("arguments")
                    .or_else(|| parsed.get("args"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let calls = vec![ToolCall {
                    id: "md-0".into(),
                    name,
                    args,
                }];
                let tool_calls_json: Vec<Value> = calls
                    .iter()
                    .map(|c| {
                        json!({
                            "id": c.id,
                            "type": "function",
                            "function": { "name": c.name, "arguments": c.args.to_string() }
                        })
                    })
                    .collect();
                let normalized_message = json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": tool_calls_json
                });
                Some(ModelResponse::ToolCalls(calls, normalized_message))
            }
            Err(e) => Some(ModelResponse::ParseError {
                raw_text: text.to_string(),
                error: format!("Invalid JSON in markdown block: {}", e),
            }),
        }
    }

    fn process_stream_line(
        &self,
        line: &str,
        format: &ApiFormat,
        full_text: &mut String,
        acc_tool_calls: &mut Vec<AccumulatedToolCall>,
        stream_tx: &Option<UnboundedSender<String>>,
    ) -> Result<()> {
        let json_str = if line.starts_with("data: ") {
            let content = line.strip_prefix("data: ").unwrap().trim();
            if content == "[DONE]" {
                return Ok(());
            }
            content
        } else {
            line.trim()
        };

        if json_str.is_empty() {
            return Ok(());
        }

        let parsed: Value = match serde_json::from_str(json_str) {
            Ok(v) => v,
            Err(_) => return Ok(()), // ignore malformed/non-JSON lines
        };

        // Extract error
        if let Some(err) = parsed.get("error") {
            let msg = err["message"]
                .as_str()
                .or_else(|| err.as_str())
                .unwrap_or("unknown API error");
            anyhow::bail!("API error in stream: {}", msg);
        }

        match format {
            ApiFormat::OpenAiCompatible => {
                if let Some(choices) = parsed.get("choices").and_then(|v| v.as_array())
                    && !choices.is_empty()
                {
                    let delta = &choices[0]["delta"];

                    // Text content
                    if let Some(content) = delta.get("content").and_then(|v| v.as_str()) {
                        full_text.push_str(content);
                        if let Some(tx) = stream_tx {
                            let _ = tx.send(content.to_string());
                        }
                    }

                    // Tool calls
                    if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
                        for tc in tool_calls {
                            let idx =
                                tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                            if idx >= acc_tool_calls.len() {
                                acc_tool_calls.resize(idx + 1, AccumulatedToolCall::default());
                            }

                            if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                                acc_tool_calls[idx].id = Some(id.to_string());
                            }
                            if let Some(sig) = tc
                                .get("thought_signature")
                                .or_else(|| tc.get("thoughtSignature"))
                                .and_then(|v| v.as_str())
                            {
                                acc_tool_calls[idx].thought_signature = Some(sig.to_string());
                            }
                            if let Some(extra) = tc.get("extra_content") {
                                acc_tool_calls[idx].extra_content = Some(extra.clone());
                            }
                            if let Some(func) = tc.get("function") {
                                if let Some(name) = func.get("name").and_then(|v| v.as_str()) {
                                    acc_tool_calls[idx].name = Some(name.to_string());
                                }
                                if let Some(args) = func.get("arguments").and_then(|v| v.as_str()) {
                                    acc_tool_calls[idx].arguments.push_str(args);
                                }
                            }
                        }
                    }
                }
            }
            ApiFormat::OllamaNative => {
                let message = &parsed["message"];
                if let Some(content) = message.get("content").and_then(|v| v.as_str()) {
                    full_text.push_str(content);
                    if let Some(tx) = stream_tx {
                        let _ = tx.send(content.to_string());
                    }
                }

                if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
                    for tc in tool_calls {
                        let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                        let args_val = &tc["function"]["arguments"];
                        let args_str = if args_val.is_string() {
                            args_val.as_str().unwrap_or("").to_string()
                        } else {
                            args_val.to_string()
                        };
                        let id = tc
                            .get("id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();

                        acc_tool_calls.push(AccumulatedToolCall {
                            id: Some(id),
                            name: Some(name),
                            arguments: args_str,
                            thought_signature: None,
                            extra_content: None,
                        });
                    }
                }
            }
        }

        Ok(())
    }
}

#[derive(Default, Clone)]
struct AccumulatedToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
    thought_signature: Option<String>,
    extra_content: Option<Value>,
}

#[async_trait]
impl ModelAdapter for OpenAiCompatibleAdapter {
    #[tracing::instrument(skip(self, messages, tools, stream_tx),
        fields(provider = %self.config.active_provider, model = %self.config.active_model))]
    async fn call(
        &self,
        system_prompt: &str,
        messages: &[Value],
        tools: Vec<ToolDescriptor>,
        stream_tx: Option<UnboundedSender<String>>,
    ) -> Result<ModelResponse> {
        let mut cache = self.resolved_provider.lock().await;
        let provider = if let Some(ref p) = *cache {
            p.clone()
        } else {
            let p = self.config.resolve_best_provider(&self.client).await;
            *cache = Some(p.clone());
            p
        };
        // Drop the lock so it is not held during the request execution
        drop(cache);

        let format = &provider.api_format;
        let endpoint = self.endpoint(&provider.base_url, format);
        let api_key = provider.api_key.as_deref();

        let streaming = stream_tx.is_some();
        let payload = self.build_payload(system_prompt, messages, tools, format, streaming);

        let mut req = self.client.post(&endpoint).json(&payload);
        if let Some(key) = api_key {
            req = req.bearer_auth(key);
        }

        tracing::debug!(endpoint = %endpoint, "Calling model");

        if let Some(ref tx) = stream_tx {
            let _ = tx.send("\x02".to_string());
        }

        let resp_res = req.send().await;

        if let Some(ref tx) = stream_tx {
            let _ = tx.send("\x03".to_string());
        }

        let resp_val = resp_res?;
        if !resp_val.status().is_success() {
            let status = resp_val.status();
            let body = resp_val.text().await.unwrap_or_default();
            if status == reqwest::StatusCode::UNAUTHORIZED && provider.name == "Ollama Cloud" {
                anyhow::bail!(
                    "Ollama Cloud authentication failed (401 Unauthorized).\n\n\
                     Please verify that:\n\
                     1. You have a valid Ollama account on https://ollama.com\n\
                     2. You have configured a valid API key for Ollama Cloud in Helix (using `/config` or via config.toml)."
                );
            }
            anyhow::bail!("API returned error status {}: {}", status, body);
        }

        if streaming {
            let mut stream = resp_val.bytes_stream();
            let mut full_text = String::new();
            let mut acc_tool_calls: Vec<AccumulatedToolCall> = Vec::new();
            let mut line_buf = Vec::new();

            while let Some(chunk_res) = stream.next().await {
                let chunk = chunk_res?;
                for &byte in chunk.iter() {
                    if byte == b'\n' {
                        let line = String::from_utf8_lossy(&line_buf).to_string();
                        line_buf.clear();
                        if !line.trim().is_empty() {
                            self.process_stream_line(
                                &line,
                                format,
                                &mut full_text,
                                &mut acc_tool_calls,
                                &stream_tx,
                            )?;
                        }
                    } else {
                        line_buf.push(byte);
                    }
                }
            }
            if !line_buf.is_empty() {
                let line = String::from_utf8_lossy(&line_buf).to_string();
                if !line.trim().is_empty() {
                    self.process_stream_line(
                        &line,
                        format,
                        &mut full_text,
                        &mut acc_tool_calls,
                        &stream_tx,
                    )?;
                }
            }

            if !acc_tool_calls.is_empty() {
                let mut calls = Vec::new();
                let mut tool_calls_json = Vec::new();
                for (idx, atc) in acc_tool_calls.into_iter().enumerate() {
                    let name = atc.name.clone().unwrap_or_default();
                    let id = atc.id.clone().unwrap_or_else(|| format!("call_{}", idx));
                    let args_str = atc.arguments.trim();
                    let args = if args_str.is_empty() {
                        json!({})
                    } else {
                        match serde_json::from_str::<Value>(args_str) {
                            Ok(v) => v,
                            Err(e) => {
                                return Ok(ModelResponse::ParseError {
                                    raw_text: args_str.to_string(),
                                    error: format!("Failed to parse tool args: {}", e),
                                });
                            }
                        }
                    };
                    calls.push(ToolCall {
                        id: id.clone(),
                        name: name.clone(),
                        args: args.clone(),
                    });

                    let mut tc_val = json!({
                        "id": id,
                        "type": "function",
                        "function": { "name": name, "arguments": args.to_string() }
                    });
                    if let Some(sig) = atc.thought_signature {
                        tc_val["thought_signature"] = json!(sig);
                    }
                    if let Some(extra) = atc.extra_content {
                        tc_val["extra_content"] = extra;
                    }
                    tool_calls_json.push(tc_val);
                }

                let normalized_message = json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": tool_calls_json,
                });
                Ok(ModelResponse::ToolCalls(calls, normalized_message))
            } else {
                if let Some(resp) = self.try_parse_markdown_tool_call(&full_text) {
                    return Ok(resp);
                }
                Ok(ModelResponse::EndTurn(full_text))
            }
        } else {
            let resp: Value = resp_val.json().await?;
            tracing::debug!(response = %resp, "Raw model response");
            let model_response = self.parse_response(&resp, format)?;

            // Replay text through the streaming channel so the REPL gets a
            // typewriter effect without requiring real SSE parsing.
            if let (ModelResponse::EndTurn(text), Some(tx)) = (&model_response, stream_tx) {
                // Split on whitespace boundaries to send word-sized chunks.
                // split_inclusive keeps the delimiter (space/newline) with the preceding word.
                for chunk in text.split_inclusive(|c: char| c.is_whitespace()) {
                    if tx.send(chunk.to_string()).is_err() {
                        break; // receiver dropped (Ctrl-C)
                    }
                }
            }

            Ok(model_response)
        }
    }

    fn provider_name(&self) -> &str {
        &self.config.active_provider
    }
    fn model_name(&self) -> &str {
        &self.config.active_model
    }
    fn set_thinking_level(&self, level: Option<String>) {
        if let Ok(mut w) = self.thinking_level.write() {
            *w = level;
        }
    }
}
