# ⚡ Helix

An autonomous, high-performance, tool-calling AI agent CLI built in Rust. 
It acts as a research-grade engine designed to give you absolute control over agent loops, context windows, execution tracing, and now, **Self-Evolution**.

## 🧬 Self-Evolving Architecture (Security First)

Helix is designed to optimize its own codebase without ever regressing or compromising your machine's security.

- **Metrics Tracking**: Records detailed telemetry for every agent turn (latency, token efficiency, tool accuracy, heuristic compaction events, and syntax retries).
- **Automated Proposer (`/evolve`)**: Analyzes recent session bottlenecks and uses an LLM to propose a focused, high-value unified diff to improve the engine.
- **Immutable Security Gates**: Proposed patches are scanned to reject unsafe operations (e.g., `unsafe` blocks, raw process spawns, network clients) and prevent mutations to the security logic itself via `.evolution-lock`.
- **No-Regression Contract**: If a patch passes security and compilation gates, it runs against the headless Benchmark Suite. If any metric drops below the `baseline.json`, Helix automatically issues a `git revert` to undo the damage.
- **Human-in-the-Loop (`/approve`)**: Helix can *never* self-authorize code changes. It prepares the environment and tests everything, but a human must explicitly type `/approve` to finalize the evolution.

## 🚀 Core Features

- **Local Semantic Memory**: Powered by `turbovec` (4-bit Lloyd-Max quantized SIMD index) and `fastembed` (offline ONNX model running `BAAI/bge-small-en-v1.5`), paired with SQLite metadata storage. Seamlessly retrieves past workspace memories to enrich prompts and saves new conversation turns automatically. Highly memory-efficient: operates with a baseline footprint of **~227 MiB RSS** (17 threads initialized on startup) and scales at only **~3.2 MiB per 100,000 memories**, comfortably fitting in a 256 MiB container limit. For details and verification commands, see [docs/memory_architecture.md#4-memory-footprint-and-resource-verification](docs/memory_architecture.md#4-memory-footprint-and-resource-verification).
- **Model Agnostic & Seamless Swapping**: Fully compatible with any OpenAI-compatible API format (OpenAI, OpenRouter) and Ollama Native format. Switch mid-session using `/config`.
- **"Local Healer" Engine**: Built to orchestrate fragile open-weights models. If a model hallucinates malformed tool-calling JSON, the engine intercepts the parse error and automatically prompts the model to self-correct its syntax (up to 3 retries).
- **Secure Tool Execution**: Features a built-in `bash` tool with strict interactive user confirmation, ensuring the agent cannot execute unsafe shell operations without your explicit approval.
- **Markdown Fallback Parsing**: Even if a model fails to use official API tool calls, the engine can manually extract and execute tool calls encoded in standard ````json ```` markdown blocks.
- **Zero-Leak Async Execution**: Powered by `tokio`. Pressing `Ctrl+C` instantly safely tears down the execution future, killing hanging network requests without memory leaks.

## 🛠️ Usage

### Building

```bash
cargo build --release
```

### Running the Engine

```bash
cargo run
# Or with verbose logging
cargo run -- -v
cargo run -- -vv
```

### REPL Commands

- `/memory [query]` — search/list workspace memories (use `/memory --clear` to clear)
- `/status` — show engine status, memory utilization bar, compaction stats, and active configurations
- `/save-baseline` — save the current benchmark run as the new performance baseline
- `/evolve [--auto-approve]` — analyze metrics, check compilation/security gates, and propose/apply self-evolution patch
- `/approve` — apply and commit the pending evolution diff
- `/reject [reason]` — discard the pending evolution diff
- `/config` — reconfigure active provider and model
- `/sessions` — list recent chat sessions
- `/exit` | `/quit` — exit the REPL loop

## 🔌 Model Context Protocol (MCP)

Helix natively supports dynamic integration with external tool servers conforming to the Model Context Protocol (MCP) over `stdio` transport. Specify your servers in `mcp_config.json` in the current directory or config directory, and Helix will spawn them, initialize connection, and automatically load/register their tool schemas on boot.

For more details, see [docs/mcp_integration.md](docs/mcp_integration.md).

## 📖 Documentation

For detailed design, data flow diagrams, and sequence diagrams of Helix's subsystems (including the local memory store, self-evolution loop, and MCP tool clients), see the [docs/](docs/) directory.
