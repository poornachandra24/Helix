use anyhow::Result;
use async_trait::async_trait;

use reqwest::Client;
use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::{ApiFormat, AppConfig};
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
    ToolCalls(Vec<ToolCall>),
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
}

// ──────────────────────────────────────────────
// OpenAI-compatible adapter (default)
// ──────────────────────────────────────────────

pub struct OpenAiCompatibleAdapter {
    pub config: AppConfig,
    client: Client,
}

impl OpenAiCompatibleAdapter {
    pub fn new(config: AppConfig) -> Self {
        Self { config, client: Client::new() }
    }

    /// Build the full endpoint URL based on the provider's `ApiFormat`.
    fn endpoint(&self, provider_base: &str, format: &ApiFormat) -> String {
        match format {
            ApiFormat::OpenAiCompatible => format!("{}/chat/completions", provider_base),
            ApiFormat::OllamaNative     => format!("{}/api/chat", provider_base),
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
                msgs.extend_from_slice(messages);

                let openai_tools: Vec<Value> = tools.into_iter().map(|t| json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })).collect();

                json!({
                    "model":       self.config.active_model,
                    "messages":    msgs,
                    "tools":       openai_tools,
                    "temperature": 0.1,
                    "stream":      streaming,
                })
            }
            ApiFormat::OllamaNative => {
                let mut msgs = vec![json!({"role": "system", "content": system_prompt})];
                msgs.extend_from_slice(messages);

                // Ollama native tool format mirrors OpenAI's tool schema
                let ollama_tools: Vec<Value> = tools.into_iter().map(|t| json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })).collect();

                json!({
                    "model":    self.config.active_model,
                    "messages": msgs,
                    "tools":    ollama_tools,
                    "stream":   streaming,
                    "options":  { "temperature": 0.1 },
                })
            }
        }
    }

    /// Parse a complete (non-streaming) response from either format.
    fn parse_response(&self, resp: &Value, format: &ApiFormat) -> Result<ModelResponse> {
        if let Some(err) = resp.get("error") {
            let msg = err["message"].as_str()
                .or_else(|| err.as_str())
                .unwrap_or("unknown API error");
            anyhow::bail!("API error: {}", msg);
        }

        let message = match format {
            ApiFormat::OpenAiCompatible => &resp["choices"][0]["message"],
            ApiFormat::OllamaNative     => &resp["message"],
        };

        // ── Tool calls ────────────────────────────────────────────
        if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
            if !tool_calls.is_empty() {
                let mut calls = Vec::new();
                for tc in tool_calls {
                    let func = &tc["function"];
                    let args_str = func["arguments"].as_str().unwrap_or("{}");
                    match serde_json::from_str::<Value>(args_str) {
                        Ok(args) => calls.push(ToolCall {
                            id:   tc["id"].as_str().unwrap_or("0").to_string(),
                            name: func["name"].as_str().unwrap_or("").to_string(),
                            args,
                        }),
                        Err(e) => return Ok(ModelResponse::ParseError {
                            raw_text: args_str.to_string(),
                            error:    format!("Failed to parse tool args: {}", e),
                        }),
                    }
                }
                if !calls.is_empty() {
                    return Ok(ModelResponse::ToolCalls(calls));
                }
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
                let args = parsed.get("arguments")
                    .or_else(|| parsed.get("args"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                Some(ModelResponse::ToolCalls(vec![ToolCall {
                    id: "md-0".into(),
                    name,
                    args,
                }]))
            }
            Err(e) => Some(ModelResponse::ParseError {
                raw_text: text.to_string(),
                error: format!("Invalid JSON in markdown block: {}", e),
            }),
        }
    }

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
        let provider = self.config.get_active_provider()?;
        let format = &provider.api_format;
        let endpoint = self.endpoint(&provider.base_url, format);
        let api_key = provider.api_key.as_deref();

        // Always use non-streaming. Streaming APIs return NDJSON/SSE which is
        // format-specific and fragile to parse mid-stream when the model may
        // return either text or tool calls. Instead we receive the complete
        // response and replay text tokens through the channel for the REPL.
        let payload = self.build_payload(system_prompt, messages, tools, format, false);

        let mut req = self.client.post(&endpoint).json(&payload);
        if let Some(key) = api_key {
            req = req.bearer_auth(key);
        }

        tracing::debug!(endpoint = %endpoint, "Calling model");

        let resp: Value = req.send().await?.json().await?;
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

    fn provider_name(&self) -> &str { &self.config.active_provider }
    fn model_name(&self)    -> &str { &self.config.active_model }
}
