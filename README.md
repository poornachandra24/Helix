# Harness CLI

An autonomous, high-performance, tool-calling AI agent CLI built in Rust. 
It acts as a research-grade engine designed to give you absolute control over agent loops, context windows, and execution tracing.

## Features

- **Model Agnostic & Seamless Swapping**: Fully compatible with any OpenAI-compatible API format. Effortlessly switch between local inference (Ollama, vLLM), proxies (OpenRouter), or frontier endpoints (OpenAI) mid-session using `/config`.
- **"Local Healer" Engine**: Built to orchestrate fragile open-weights models. If a model hallucinates malformed tool-calling JSON, the engine intercepts the parse error and automatically prompts the model to self-correct its syntax (up to 3 retries).
- **Secure Tool Execution**: Features a built-in `bash` tool with strict interactive user confirmation (`dialoguer`), ensuring the agent cannot execute unsafe shell operations without your explicit approval.
- **Markdown Fallback Parsing**: Even if a model fails to use official API tool calls, the engine can manually extract and execute tool calls encoded in standard ````json ```` markdown blocks.
- **Zero-Leak Async Execution**: Powered by `tokio`. Pressing `Ctrl+C` instantly safely tears down the execution future, instantly killing hanging network requests without memory leaks.
- **Skill Extensibility**: Dynamically read and inject custom behavior text from a `skills/` directory, allowing you to quickly define domain-specific guidelines.
- **Deep Tracing**: Leverage `tracing` and `tracing-subscriber`. Pass `-v` or `-vv` to introspect chronological execution states, context compaction operations, and raw API payloads.

## Security

By default, any tool that can modify your system (e.g., executing Bash commands) will halt and prompt the user for interactive (`Y/n`) confirmation.

## Building

```bash
cargo build --release
```

## Running

```bash
cargo run
# Or with verbose logging
cargo run -- -v
cargo run -- -vv
```
