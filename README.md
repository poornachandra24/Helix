# ⚡ Helix

An autonomous, high-performance, tool-calling AI agent CLI built in Rust.  
Designed as a research-grade engine giving you absolute control over agent loops, context windows, execution tracing, and **Self-Evolution** — wrapped in a premium terminal UI.

---

## 🧬 Self-Evolving Architecture (Security First)

Helix is designed to optimize its own codebase without ever regressing or compromising your machine's security.

- **Metrics Tracking**: Records detailed telemetry for every agent turn (latency, token efficiency, tool accuracy, heuristic compaction events, and syntax retries).
- **Automated Proposer (`/evolve`)**: Analyzes recent session bottlenecks and uses an LLM to propose a focused unified diff to improve the engine.
- **Immutable Security Gates**: Proposed patches are scanned to reject unsafe operations (e.g., `unsafe` blocks, raw process spawns, network clients) and prevent mutations to security logic itself via `.evolution-lock`.
- **No-Regression Contract**: If a patch passes security and compilation gates, it runs against the headless Benchmark Suite. If any metric drops below `baseline.json`, Helix automatically issues a `git revert`.
- **Human-in-the-Loop (`/approve`)**: Helix can *never* self-authorize code changes. A human must explicitly type `/approve` to finalize the evolution.

---

## 🚀 Core Features

- **Local Semantic Memory**: Powered by `turbovec` (4-bit Lloyd-Max quantized SIMD index) and `fastembed` (offline ONNX `BAAI/bge-small-en-v1.5`), paired with SQLite metadata. Baseline footprint ~227 MiB RSS, scaling at ~3.2 MiB per 100k memories. See [docs/memory_architecture.md](docs/memory_architecture.md).
- **SONA Neural Adaptation**: After every turn, the SONA engine records a trajectory quality score (0–1) and applies Micro-LoRA weight updates to the semantic memory query path — making retrieval progressively better-tuned to your workspace over time.
- **Model Agnostic & Seamless Swapping**: Compatible with any OpenAI-compatible API (OpenAI, OpenRouter) and Ollama native format. Switch mid-session using `/use`.
- **"Local Healer" Engine**: Intercepts malformed tool-calling JSON from open-weights models and automatically prompts for self-correction (up to 3 retries).
- **Secure Tool Execution**: Built-in `bash` tool with strict interactive confirmation — the agent cannot execute shell operations without explicit approval.
- **Markdown Fallback Parsing**: Extracts and executes tool calls encoded in ` ```json ``` ` markdown blocks if a model bypasses official API tool calls.
- **Zero-Leak Async Execution**: Powered by `tokio`. `Ctrl+C` instantly tears down the execution future without memory leaks.
- **Terminal-Adaptive Layout**: All UI boxes dynamically read terminal column width (`console::Term::stdout().size_checked()`) and clamp content between 50–110 columns. Unicode characters are measured by display width (`.chars().count()`), not byte count, to prevent overflow.

---

## 🖥️ Terminal UI Design

Helix uses a two-tone boxed conversation layout:

| Box | Color scheme | Label |
|-----|-------------|-------|
| User input | Cool blue/cyan (adaptive, `blue()` border and header) | `You` |
| Agent response | Warm amber/gold (adaptive, `yellow()` border and header, markdown formatted) | `Helix` |

Both boxes use rounded Unicode corners (`╭ ╮ ╰ ╯`) and display a `[Elapsed: X.XXs]` timer on the closing border.

**Post-response footnotes** (dim `color256(240)`, printed after the box closes):
```
  ◆ Memory: Found N relevant workspace memories (adapted via Micro-LoRA)
  · loop 1/20
  · [sona] quality 0.95
```

When tools are called mid-turn:
```
  ⦿ Tool: dispatching 1 → bash
  ✓ 'bash' completed (219 bytes)
```

The `· loop N/20` label shows which agent tool-use iteration is active. The max is 20 iterations per turn to prevent runaway loops.

For full color token definitions and box math, see [docs/ux_design.md](docs/ux_design.md).

---

## 🛠️ Usage

### Building

```bash
cargo build --release
```

### Running the Engine

```bash
cargo run
# With verbose logging
cargo run -- -v
cargo run -- -vv
```

---

## 💬 REPL Commands

### General Chat & Context

| Command | Description |
|---------|-------------|
| `/help` | Show this command guide |
| `/status` | Show active model, context budget, SONA & evolution stats |
| `/clear` | Reset current chat history context |
| `/config` | Reconfigure active provider / model |
| `/providers` | List configured API providers |
| `/use <name> [model]` | Hot-switch provider/model in the current session |
| `/sessions` | List previous chat sessions |
| `/resume <id>` | Load a past session into the active context |
| `/memory [query]` | Search/manage semantic memory (`--clear` to wipe) |
| `/exit` \| `/quit` | Exit the REPL session |

### Codebase Optimization (Self-Evolution)

| Command | Description |
|---------|-------------|
| `/evolve` | Analyze logs and propose a self-evolution patch |
| `/evolve --auto-approve` | Analyze, check gates, test, and apply automatically if safe |
| `/evolve --dry-run` | Analyze metrics but skip proposing edits |
| `/approve` | Apply the pending evolution diff |
| `/reject [reason]` | Discard the pending evolution patch |

### Agent Performance Benchmarking

| Command | Description |
|---------|-------------|
| `/benchmark` | Run the benchmark suite against the current engine |
| `/save-baseline` | Save current benchmark results as the new performance baseline |

---

## 🔌 Model Context Protocol (MCP)

Helix natively supports external tool servers conforming to MCP over `stdio` transport. Specify servers in `mcp_config.json` in the current directory or config directory, and Helix will spawn, initialize, and register their tool schemas on boot.

See [docs/mcp_integration.md](docs/mcp_integration.md) for details.

---

## 📖 Documentation

| Document | Contents |
|----------|----------|
| [docs/memory_architecture.md](docs/memory_architecture.md) | Dual-store memory design, data flow, footprint profiles |
| [docs/self_evolution.md](docs/self_evolution.md) | Evolution loop, security gates, regression contract |
| [docs/mcp_integration.md](docs/mcp_integration.md) | MCP client subsystem, process lifecycle, config |
| [docs/ux_design.md](docs/ux_design.md) | Terminal UI system, color tokens, box math, telemetry format |
