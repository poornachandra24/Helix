use anyhow::Result;
use reqwest::Client;
use serde_json::{json, Value};
use crate::config::AppConfig;
use crate::tools::ToolDescriptor;

#[derive(Debug)]
pub enum ModelResponse {
    ToolCall { name: String, args: Value },
    EndTurn(String),
    ParseError { raw_text: String, error: String },
}

pub struct OllamaAdapter {
    pub config: AppConfig,
    client: Client,
}

impl OllamaAdapter {
    pub fn new(config: AppConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }

    pub async fn call(&self, system_prompt: &str, messages: &[Value], tools: Vec<ToolDescriptor>) -> Result<ModelResponse> {
        let provider = self.config.get_active_provider()?;
        
        let mut formatted_messages = vec![
            json!({"role": "system", "content": system_prompt})
        ];
        formatted_messages.extend_from_slice(messages);

        let openai_tools: Vec<Value> = tools.into_iter().map(|t| {
            json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                }
            })
        }).collect();

        let payload = json!({
            "model": self.config.active_model,
            "messages": formatted_messages,
            "tools": openai_tools,
            "temperature": 0.1,
        });

        let mut req = self.client.post(format!("{}/chat/completions", provider.base_url))
            .json(&payload);
            
        if let Some(key) = &provider.api_key {
            req = req.bearer_auth(key);
        }

        let resp: Value = req.send().await?.json().await?;
        
        if resp.get("error").is_some() {
            anyhow::bail!("API Error: {}", serde_json::to_string_pretty(&resp)?);
        }
        
        let message = &resp["choices"][0]["message"];

        if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
            if !tool_calls.is_empty() {
                let func = &tool_calls[0]["function"];
                let args_str = func["arguments"].as_str().unwrap_or("{}");
                match serde_json::from_str::<Value>(args_str) {
                    Ok(args) => {
                        return Ok(ModelResponse::ToolCall {
                            name: func["name"].as_str().unwrap_or("").to_string(),
                            args,
                        });
                    }
                    Err(e) => {
                        return Ok(ModelResponse::ParseError {
                            raw_text: args_str.to_string(),
                            error: format!("Failed to parse formal tool arguments: {}", e),
                        });
                    }
                }
            }
        }
        
        let text_content = message["content"].as_str().unwrap_or("");
        
        // Fallback parser
        if text_content.contains("```json") {
            if let Some(start) = text_content.find("```json") {
                let json_start = start + 7;
                if let Some(end) = text_content[json_start..].find("```") {
                    let json_str = &text_content[json_start..json_start + end];
                    match serde_json::from_str::<Value>(json_str) {
                        Ok(parsed) => {
                            if let Some(name) = parsed.get("name").and_then(|v| v.as_str()) {
                                let args = parsed.get("arguments").or(parsed.get("args")).cloned().unwrap_or_else(|| json!({}));
                                return Ok(ModelResponse::ToolCall {
                                    name: name.to_string(),
                                    args,
                                });
                            } else {
                                return Ok(ModelResponse::ParseError {
                                    raw_text: text_content.to_string(),
                                    error: "JSON is missing 'name' field for tool call".to_string(),
                                });
                            }
                        }
                        Err(e) => {
                            return Ok(ModelResponse::ParseError {
                                raw_text: text_content.to_string(),
                                error: format!("Invalid JSON syntax in markdown block: {}", e),
                            });
                        }
                    }
                }
            }
        }

        Ok(ModelResponse::EndTurn(text_content.to_string()))
    }
}
