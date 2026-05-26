# Model Context Protocol (MCP) Integration

Helix supports dynamic tool registration through the **Model Context Protocol (MCP)**. This allows the engine to load external tools (e.g., databases, browser controllers, search engines) at runtime without compiling new Rust code.

---

## Architecture & Data Flow

```mermaid
graph TD
    User[User REPL / CLI] --> HelixEngine[Helix Engine]
    
    subgraph Tooling Layer
        HelixEngine --> ToolRegistry[Tool Registry]
        ToolRegistry --> Native[Native Rust Tools: bash, read_file]
        ToolRegistry --> MCPClient[MCP Client: rmcp]
    end
    
    subgraph External MCP Servers
        MCPClient -->|Spawn subprocess| Postgres[Postgres MCP Server]
        MCPClient -->|Spawn subprocess| Puppeteer[Puppeteer Browser MCP]
    end
```

---

## Execution Flow

When Helix starts up, it reads `mcp_config.json`. The handshake, tools discovery, and execution follow this sequence:

```mermaid
sequenceDiagram
    autonumber
    participant Helix as Helix Engine
    participant SDK as rmcp (MCP Client)
    participant Server as External MCP Process (Node/Python)

    Helix->>SDK: Initialize MCP Registry
    SDK->>Server: Spawn subprocess (stdio piped)
    Server-->>SDK: JSON-RPC handshake established (initialize)
    
    Helix->>SDK: Query available tools (list_all_tools)
    SDK->>Server: {"jsonrpc": "2.0", "method": "tools/list", "id": 1}
    Server-->>SDK: Returns schemas for tools
    SDK-->>Helix: Wrap as dyn Tool & register in ToolRegistry
    
    %% Execution
    Helix->>SDK: call(tool_name, arguments)
    SDK->>Server: {"jsonrpc": "2.0", "method": "tools/call", "params": {...}, "id": 2}
    Server-->>SDK: Returns execution results (content blocks)
    SDK-->>Helix: Returns stringified results
```

---

## Configuration

Helix looks for an `mcp_config.json` configuration file in the following order of precedence:
1.  **Current Directory**: `./mcp_config.json`
2.  **Global Config Directory**: `<config_dir>/harness-cli/mcp_config.json`

### Example `mcp_config.json`

```json
{
  "mcpServers": {
    "postgres": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-postgres", "postgresql://localhost:5432/helix_db"],
      "requiresConfirmation": true
    },
    "puppeteer": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-puppeteer"]
    }
  }
}
```

### Configuration Fields
*   `command`: The executable to run (e.g., `npx`, `python3`, `node`).
*   `args`: Array of command-line arguments.
*   `env`: Optional map of environment variables to pass to the process.
*   `requiresConfirmation`: Optional boolean. If set to `true`, Helix will prompt the operator for confirmation before dispatching any tool from this server.
