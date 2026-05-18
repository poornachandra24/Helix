use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// All tools must implement this trait.
/// `async fn call` is object-safe via `async_trait`.
/// Tools that modify the system should set `requires_confirmation` to `true`.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;

    /// If `true`, the engine serialises this tool's dispatch and prompts the user
    /// for confirmation before running. Defaults to `false` (read-only / safe).
    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn call(&self, args: Value) -> Result<String>;
}

/// Central registry for all available tools.
/// Uses `Arc<dyn Tool>` so individual tools can be cloned into
/// parallel `tokio::spawn` tasks without copying.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: HashMap::new() }
    }

    pub fn register(&mut self, tool: impl Tool + 'static) {
        let t: Arc<dyn Tool> = Arc::new(tool);
        self.tools.insert(t.name().to_string(), t);
    }

    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        let mut descs: Vec<ToolDescriptor> = self.tools.values().map(|t| ToolDescriptor {
            name: t.name().to_string(),
            description: t.description().to_string(),
            parameters: t.parameters_schema(),
        }).collect();
        // Stable ordering so the LLM sees tools in the same order every call
        descs.sort_by(|a, b| a.name.cmp(&b.name));
        descs
    }

    /// Get a cloneable `Arc` handle to a tool for parallel dispatch.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Sequential fallback — used when a tool requires confirmation.
    pub async fn dispatch(&self, name: &str, args: Value) -> Result<String> {
        match self.tools.get(name) {
            Some(tool) => tool.call(args).await,
            None => anyhow::bail!("Tool '{}' not found", name),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
