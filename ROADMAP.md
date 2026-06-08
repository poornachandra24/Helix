# Helix Project Roadmap

This document outlines the short-term, mid-term, and long-term milestones for the Helix agent harness, focusing on developer extensibility, performance optimization, and robust observability.

---

## 🗺️ Milestones

### 🟢 Short-Term (Next 1-3 Months): Observability & Diagnostics
*   **OTLP Tracing & Observability**: Integrate native OpenTelemetry (`opentelemetry` & `tracing-opentelemetry`) to export spans for model latencies, vector DB search queries, and sandbox executions to external collectors (e.g. Jaeger, Honeycomb).
*   **WASM Sandbox Resource Profiling**: Expose WASM guest execution resource statistics (fuel/gas consumed via `wasmi` and sandbox directory disk footprint) directly into the telemetry subscriber.
*   **TUI Performance Overlay**: Add a lightweight, interactive terminal visualization showing real-time token budgets, memory retrieval quality scores, and tool execution cycles.

### 🟡 Mid-Term (Next 3-6 Months): Multimodal & Developer Extensibility
*   **Multimodal Input Support**: Refactor the inner message structure from a plain `String` to content-part enums, allowing users to attach screenshots or file buffers, and update the API drivers (Ollama/OpenAI) to support vision models.
*   **WASM/WASI Plugin SDK**: Develop a structured SDK and manifest format allowing developers to write custom tools in Rust, C, or Go, compile them to WebAssembly, and dynamically register them in the sandbox.
*   **Dynamic Context Compaction**: Implement smart context compression utilizing a local Small Language Model (SLM) to summarize old chat turns instead of hard-evicting tokens.

### 🔴 Long-Term (6+ Months): Concurrency & GPU Acceleration
*   **Specialized Sub-Agent Orchestration**: Support spawning and managing concurrent, isolated child agents inside separate WASI sandboxes to perform sub-tasks in parallel (e.g., compile-test loops).
*   **Semantic Knowledge Graph Index**: Transition from flat vector memories to a structured, local semantic knowledge graph (SQLite metadata + vector index) to capture entities and relationships.
*   **GPU & Hardware Inference Acceleration**: Optimize local embedding generation and similarity calculations by linking against system-level BLAS/LAPACK runtimes and supporting ONNX runtime GPU execution providers.
