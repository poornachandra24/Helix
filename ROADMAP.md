# Helix Project Roadmap

This document outlines the short-term, mid-term, and long-term milestones for the Helix agent harness, focusing on developer extensibility, performance optimization, and robust observability.

---

## Completed Milestones & Capabilities

*   **Offline Semantic Memory & Hybrid Retrieval**: Combined SQLite metadata with a 4-bit Lloyd-Max quantized SIMD vector index (`turbovec`) and offline ONNX `BAAI/bge-small-en-v1.5` embeddings (`fastembed`). Includes lexical FTS5 queries using Reciprocal Rank Fusion (RRF).
*   **Dynamic MCP Hot-Reloading**: A polling-based file integrity monitor that watches `mcp_config.json` post-turn, safely shuts down obsolete server processes, registers updated schemas, and updates LLM capabilities.
*   **WASM & Docker Sandboxing**: Secure, persistent code execution backends utilizing Docker tailing and WASI-metered runtimes with fuel limits.
*   **BLAS Hardware Acceleration**: Static linking with OpenBLAS (Linux) and Accelerate framework (macOS) to speed up similarity calculations.
*   **Session Post-Mortem Reflective Memory**: Automated background agent reflection summarization and index-serialization on session exit or clear.

---

##  Future Milestones

###  Short-Term (Next 1-3 Months): Observability & Diagnostics
*   **OTLP Tracing & Observability**: Integrate native OpenTelemetry (`opentelemetry` & `tracing-opentelemetry`) to export spans for model latencies, vector DB search queries, and sandbox executions to external collectors (e.g. Jaeger, Honeycomb).
*   **WASM Sandbox Resource Profiling**: Expose WASM guest execution resource statistics (fuel/gas consumed via `wasmi` and sandbox directory disk footprint) directly into the telemetry subscriber.
*   **TUI Performance Overlay**: Add a lightweight, interactive terminal visualization showing real-time token budgets, memory retrieval quality scores, and tool execution cycles.

###  Mid-Term (Next 3-6 Months): Multimodal & Developer Extensibility
*   **Multimodal Input Support**: Refactor the inner message structure from a plain `String` to content-part enums, allowing users to attach screenshots or file buffers, and update the API drivers (Ollama/OpenAI) to support vision models.
*   **WASM/WASI Plugin SDK**: Develop a structured SDK and manifest format allowing developers to write custom tools in Rust, C, or Go, compile them to WebAssembly, and dynamically register them in the sandbox.
*   **Dynamic SLM-Based Context Compaction**: Implement smart context compression utilizing a local Small Language Model (SLM) to summarize old chat turns instead of simple turn omission.

###  Long-Term (6+ Months): Concurrency & GPU Acceleration
*   **Provider-Agnostic Heterogeneous Multi-Agent Orchestration**: Support spawning concurrent, isolated child agents inside separate WASI sandboxes, utilizing smaller, specialized models or large models across heterogeneous providers (OpenAI, Anthropic, Ollama, Groq) to collaborate on sub-tasks. The orchestration engine dynamically handles task routing and communication, abstracting away model selection and provider configurations so the user can focus purely on task outcomes regardless of token consumption.
*   **Semantic Knowledge Graph Index**: Transition from flat vector memories to a structured, local semantic knowledge graph (SQLite metadata + vector index) to capture entities and relationships.
*   **GPU & Hardware Inference Acceleration**: Optimize ONNX runtime GPU execution providers for local embedding generation.
