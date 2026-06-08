# Helix Plugin & Extension Development Guide

This guide details how to extend the Helix agent's capabilities by writing custom Rust Tools, connecting external Model Context Protocol (MCP) servers, or adding new Markdown Skills.

---

## 1. Writing Custom Rust Tools

Every native tool in Helix implements the `Tool` trait defined in `src/tools/mod.rs`.

### The `Tool` Trait Definition
```rust
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    fn requires_confirmation(&self) -> bool { false }
    async fn call(&self, args: Value) -> Result<String>;
}
```

### Implementing a Custom Tool
Below is an example of creating a custom calculator tool:

```rust
use crate::tools::Tool;
use serde_json::json;

pub struct AddTool;

#[async_trait]
impl Tool for AddTool {
    fn name(&self) -> &str { "add" }

    fn description(&self) -> &str {
        "Adds two numbers together. Requires 'a' and 'b' parameters."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "a": { "type": "number" },
                "b": { "type": "number" }
            },
            "required": ["a", "b"]
        })
    }

    // Set to true if this action is potentially destructive and needs confirmation
    fn requires_confirmation(&self) -> bool { false }

    async fn call(&self, args: Value) -> Result<String> {
        let a = args["a"].as_f64().ok_or_else(|| anyhow::anyhow!("'a' must be a number"))?;
        let b = args["b"].as_f64().ok_or_else(|| anyhow::anyhow!("'b' must be a number"))?;
        
        let sum = a + b;
        Ok(format!("Result: {}", sum))
    }
}
```

### Registering the Tool
Instantiate your tool and add it to the `ToolRegistry` within `src/cli/helpers.rs`'s `build_tool_registry` function:
```rust
let mut registry = ToolRegistry::new();
registry.register(Box::new(AddTool));
```

---

## 2. Model Context Protocol (MCP) Integration

External tools running in separate processes (Node, Python, Go, etc.) can be attached to Helix using the Model Context Protocol.

### 2.1 Configuration
Add the external server specifications to your `mcp_config.json` (located in the current working directory or under `~/.config/helix/mcp_config.json`):

```json
{
  "mcpServers": {
    "sqlite": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-sqlite", "--db", "/home/user/my_db.db"]
    },
    "memory": {
      "command": "node",
      "args": ["/path/to/mcp-server/index.js"]
    }
  }
}
```

### 2.2 Execution Flow
1. At REPL startup, Helix parses `mcp_config.json`.
2. For each server configured, it spawns a background child process (`tokio::process::Child`).
3. Communicates via stdin/stdout JSON-RPC protocol.
4. Automatically reads the tools list exposed by the server and wraps them as native Helix tools.

---

## 3. Creating Markdown Skills

Skills are modular, natural-language behavioral guidelines stored in `~/.config/helix/skills/`. They allow you to inject expert behaviors dynamically without writing Rust code.

### 3.1 Skill Structure
Create a markdown file with YAML frontmatter at the top:

```markdown
---
name: rust-audit-expert
description: Guidelines for auditing Rust source code for memory safety and panics
created_at: 2026-06-04T00:00:00Z
---

# Rust Security & Safety Audit Guidelines

When auditing Rust code, check for the following anti-patterns:
1. Unnecessary usage of `.unwrap()` or `.expect()` where error propagation could be used.
2. Insecure index access `slice[i]` instead of `.get(i)`.
3. Unsafe blocks violating pointer alignment.
```

### 3.2 Dynamic Ingest
* Helix's `SkillRegistry` automatically scans the skills directory on startup.
* All `.txt` and `.md` files found in the skills directory are compiled.
* The combined skills block is automatically appended into the system prompt context for the session.
