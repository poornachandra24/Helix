use crate::model::{OllamaAdapter, ModelResponse};
use crate::tools::ToolRegistry;
use crate::context::ContextManager;
use crate::persistence::Session;
use serde_json::{json, Value};
use anyhow::Result;
use console::style;

pub struct Engine {
    pub model: OllamaAdapter,
    context: ContextManager,
    tools: ToolRegistry,
    pub session: Session,
    pub global_messages: Vec<Value>,
    max_iterations: usize,
    max_retries: usize,
}

impl Engine {
    pub fn new(model: OllamaAdapter, context: ContextManager, tools: ToolRegistry, session: Session) -> Self {
        Self {
            model,
            context,
            tools,
            session,
            global_messages: vec![],
            max_iterations: 15,
            max_retries: 3,
        }
    }

    async fn get_valid_action(&self, system_prompt: &str, global_messages: &[Value]) -> Result<ModelResponse> {
        let mut local_messages = global_messages.to_vec();
        
        for attempt in 1..=self.max_retries {
            if attempt > 1 {
                tracing::warn!("Local Healer attempt {}/{}", attempt, self.max_retries);
            }
            
            let response = self.model.call(system_prompt, &local_messages, self.tools.descriptors()).await?;
            
            match response {
                ModelResponse::ToolCall { name, args } => {
                    return Ok(ModelResponse::ToolCall { name, args });
                }
                ModelResponse::EndTurn(text) => {
                    return Ok(ModelResponse::EndTurn(text));
                }
                ModelResponse::ParseError { raw_text, error } => {
                    println!("⚠️  {}", style("[Syntax Error Detected: Healing automatically...]").yellow());
                    tracing::warn!("Syntax error detected: {}. Healing...", error);
                    
                    local_messages.push(json!({
                        "role": "assistant",
                        "content": raw_text
                    }));
                    local_messages.push(json!({
                        "role": "user",
                        "content": format!("Your tool call failed to parse with error: {}. Please output valid JSON.", error)
                    }));
                }
            }
        }
        
        anyhow::bail!("Local Healer failed to get valid JSON after {} retries", self.max_retries)
    }

    pub async fn run_turn(&mut self, system_prompt: &str, input: &str) -> Result<String> {
        self.global_messages.push(json!({"role": "user", "content": input}));
        self.session.append(json!({"event": "user_input", "content": input}))?;

        for step in 1..=self.max_iterations {
            tracing::info!("Global Step {}/{}", step, self.max_iterations);
            
            self.global_messages = self.context.compact_if_needed(self.global_messages.clone());
            
            let valid_action = self.get_valid_action(system_prompt, &self.global_messages).await?;
            
            match valid_action {
                ModelResponse::EndTurn(text) => {
                    self.session.append(json!({"event": "end_turn", "content": &text}))?;
                    self.global_messages.push(json!({"role": "assistant", "content": &text}));
                    return Ok(text);
                }
                ModelResponse::ToolCall { name, args } => {
                    println!("🔧 {} {} with args: {}", 
                        style("[Dispatching Tool]").magenta(),
                        style(&name).bold().cyan(),
                        style(&args).dim()
                    );
                    self.session.append(json!({
                        "event": "tool_call",
                        "tool": &name,
                        "args": &args
                    }))?;
                    
                    let args_str = serde_json::to_string_pretty(&args).unwrap_or_default();
                    self.global_messages.push(json!({
                        "role": "assistant",
                        "content": format!("```json\n{{\n  \"name\": \"{}\",\n  \"arguments\": {}\n}}\n```", name, args_str)
                    }));
                    
                    let result = match self.tools.dispatch(&name, args) {
                        Ok(res) => res,
                        Err(e) => format!("Error executing tool: {}", e),
                    };
                    
                    tracing::debug!("Tool result: {}", result);
                    self.session.append(json!({"event": "tool_result", "tool": name, "result": &result}))?;
                    
                    self.global_messages.push(json!({
                        "role": "user",
                        "content": format!("tool_result: {}", result)
                    }));
                }
                _ => unreachable!()
            }
        }
        
        Ok(format!("Stopped after {} iterations", self.max_iterations))
    }
}
