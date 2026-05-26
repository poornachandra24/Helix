use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;
use rmcp::{
    serve_client,
    service::{RoleClient, RunningService},
    transport::TokioChildProcess,
    Peer,
    model::{CallToolRequestParams, RawContent, ResourceContents},
};
use crate::tools::Tool;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: Option<HashMap<String, String>>,
    /// Optional field to force confirmation for all tools in this server.
    pub requires_confirmation: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

/// A wrapper that makes an MCP tool look like a native Helix tool.
pub struct McpWrappedTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub peer: Peer<RoleClient>,
    pub requires_confirmation: bool,
}

#[async_trait]
impl Tool for McpWrappedTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.input_schema.clone()
    }

    fn requires_confirmation(&self) -> bool {
        self.requires_confirmation
    }

    async fn call(&self, args: Value) -> Result<String> {
        let json_args = match args {
            Value::Object(map) => Some(map),
            _ => None,
        };

        let mut params = CallToolRequestParams::new(self.name.clone());
        if let Some(args) = json_args {
            params = params.with_arguments(args);
        }

        let result = self.peer.call_tool(params).await?;

        let mut text = String::new();
        for content_block in &result.content {
            match &content_block.raw {
                RawContent::Text(t) => {
                    text.push_str(&t.text);
                }
                RawContent::Resource(r) => {
                    match &r.resource {
                        ResourceContents::TextResourceContents { text: t, .. } => {
                            text.push_str(t);
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        if result.is_error.unwrap_or(false) {
            anyhow::bail!("MCP tool error: {}", text);
        }

        Ok(text)
    }
}

/// Global registry/holder for spawned MCP services.
pub struct McpRegistry {
    pub services: Vec<RunningService<RoleClient, ()>>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self { services: Vec::new() }
    }

    /// Load config from a file, spawn servers, and return wrapped tools.
    pub async fn load_and_initialize(&mut self, config_path: &Path) -> Result<Vec<McpWrappedTool>> {
        if !config_path.exists() {
            return Ok(Vec::new());
        }

        let content = std::fs::read_to_string(config_path)?;
        let config: McpConfig = serde_json::from_str(&content)?;
        let mut wrapped_tools = Vec::new();

        for (name, server_config) in config.mcp_servers {
            println!("🔌 [MCP] Initializing server '{}'...", name);
            let mut cmd = Command::new(&server_config.command);
            cmd.args(&server_config.args);
            if let Some(env) = &server_config.env {
                cmd.envs(env);
            }

            match TokioChildProcess::new(cmd) {
                Ok(transport) => {
                    match serve_client((), transport).await {
                        Ok(running_service) => {
                            let peer = running_service.peer().clone();
                            match peer.list_all_tools().await {
                                Ok(tools) => {
                                    let req_confirm = server_config.requires_confirmation.unwrap_or(false);
                                    for t in tools {
                                        // Convert rmcp tool fields to serde_json::Value
                                        let schema_val = serde_json::to_value(&t.input_schema)?;
                                        wrapped_tools.push(McpWrappedTool {
                                            name: t.name.to_string(),
                                            description: t.description.unwrap_or_default().to_string(),
                                            input_schema: schema_val,
                                            peer: peer.clone(),
                                            requires_confirmation: req_confirm,
                                        });
                                        println!("   ↳ Registered MCP tool: {}", t.name);
                                    }
                                    self.services.push(running_service);
                                }
                                Err(e) => {
                                    eprintln!("⚠️  [MCP] Failed to list tools for server '{}': {}", name, e);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("⚠️  [MCP] Failed to serve client handshake for server '{}': {}", name, e);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("⚠️  [MCP] Failed to spawn child process for server '{}': {}", name, e);
                }
            }
        }

        Ok(wrapped_tools)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_mcp_integration() {
        let temp_dir = TempDir::new().unwrap();
        let script_path = temp_dir.path().join("mock_mcp.py");
        let config_path = temp_dir.path().join("mcp_config.json");

        let mock_python = r#"
import sys
import json

for line in sys.stdin:
    if not line.strip():
        continue
    try:
        req = json.loads(line)
        method = req.get("method")
        req_id = req.get("id")

        if method == "initialize":
            res = {
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "mock-mcp-server",
                        "version": "1.0.0"
                    }
                }
            }
            sys.stdout.write(json.dumps(res) + "\n")
            sys.stdout.flush()
        elif method == "tools/list":
            res = {
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "tools": [
                        {
                            "name": "mock_echo",
                            "description": "Echoes back the input",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "message": {
                                        "type": "string"
                                    }
                                }
                            }
                        }
                    ]
                }
            }
            sys.stdout.write(json.dumps(res) + "\n")
            sys.stdout.flush()
        elif method == "tools/call":
            args = req.get("params", {}).get("arguments", {})
            msg = args.get("message", "default")
            res = {
                "jsonrpc": "2.0",
                "id": req_id,
                "result": {
                    "content": [
                        {
                            "type": "text",
                            "text": f"Echo: {msg}"
                        }
                    ],
                    "isError": False
                }
            }
            sys.stdout.write(json.dumps(res) + "\n")
            sys.stdout.flush()
    except Exception as e:
        sys.stderr.write(f"Error: {e}\n")
        sys.stderr.flush()
"#;

        std::fs::write(&script_path, mock_python).unwrap();

        let config_json = format!(
            r#"{{
                "mcpServers": {{
                    "mock-server": {{
                        "command": "python3",
                        "args": ["{}"]
                    }}
                }}
            }}"#,
            script_path.display()
        );
        std::fs::write(&config_path, config_json).unwrap();

        let mut registry = McpRegistry::new();
        let tools = registry.load_and_initialize(&config_path).await.unwrap();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "mock_echo");
        assert_eq!(tools[0].description(), "Echoes back the input");

        // Call the tool
        let args = serde_json::json!({
            "message": "Hello MCP!"
        });
        let result = tools[0].call(args).await.unwrap();
        assert_eq!(result, "Echo: Hello MCP!");
    }
}
