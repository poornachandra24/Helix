//! # Helix
//!
//! Helix is a high-performance, self-optimizing autonomous AI agent harness and library built natively in Rust.
//! Designed as an interactive workspace companion, Helix provides fine-grained control over agent execution
//! loops, context window budget alignment, session persistence, and secure command execution.
//!
//! ## System Architecture
//!
//! For a high-level visual flowchart of data ingestion, search routing, reasoning loops, and tool execution, see the [System Architecture diagram in the README](https://github.com/poornachandra24/Helix#%EF%B8%8F-system-architecture).
//!
//! ## Mathematical Foundations
//!
//! Helix employs advanced mathematical modeling to optimize context retrieval, minimize memory footprints,
//! and align queries to workspace realities:
//!
//! ### 1. SONA Query Vector Projection (LoRA)
//! At turn completion, if the execution trajectory score <span class="math-inline">S \in [0, 1]</span> is high, the query representation
//! <span class="math-inline">q \in \mathbb{R}^d</span> is projectively adjusted using Low-Rank Adaptation (LoRA) matrices <span class="math-inline">W_A \in \mathbb{R}^{r \times d}</span>
//! and <span class="math-inline">W_B \in \mathbb{R}^{d \times r}</span> with rank <span class="math-inline">r \ll d</span> (typically <span class="math-inline">r=8</span>). This mathematical formulation is derived from the Low-Rank Adaptation framework first proposed in the paper ["LoRA: Low-Rank Adaptation of Large Language Models" by Edward Hu et al. (Microsoft Research)](https://arxiv.org/abs/2106.09685). In Helix, we utilize low-rank updates (<span class="math-inline">W_A</span> and <span class="math-inline">W_B</span>) to projectively shift the query vector closer to the semantic centroid of files associated with high-scoring trajectories:
//!
//! ```math
//! q' = q + \Delta q = q + \frac{\eta \cdot S}{r} (W_B W_A q)
//! ```
//!
//! where <span class="math-inline">\eta</span> is the learning rate parameter.
//!
//! ### 2. Elastic Weight Consolidation (EWC++)
//! To prevent catastrophic forgetting during sequential trajectory optimizations, parameter updates are regularized
//! by penalizing changes to critical parameter dimensions derived from the diagonal Fisher Information Matrix <span class="math-inline">F</span>. This formulation is based on the Elastic Weight Consolidation regularization method introduced in ["Overcoming catastrophic forgetting in neural networks" by James Kirkpatrick et al. (Google DeepMind)](https://arxiv.org/abs/1612.00796). Helix uses this to regularize parameter updates on the SONA query projection matrices, preventing catastrophic forgetting of previously learned query alignments:
//!
//! ```math
//! \mathcal{L}(\theta) = \mathcal{L}_T(\theta) + \sum_{i} \frac{\lambda}{2} F_i (\theta_i - \theta_{T-1,i}^{*})^2
//! ```
//!
//! where <span class="math-inline">\theta_i</span> represents active weights, <span class="math-inline">\theta_{T-1,i}^{*}</span> represents optimized weights from previous turns,
//! and <span class="math-inline">\lambda</span> controls regularization strength.
//!
//! ### 3. TurboQuant: Data-Oblivious Vector Quantization
//! To compress vector indices in the [turbovec](https://github.com/RyanCodrai/turbovec) vector search library (created by Ryan Codrai) by up to 16x while preserving similarity rankings, dimensions are quantized
//! using the TurboQuant algorithm first proposed in ["TurboQuant: Online Vector Quantization with Near-optimal Distortion Rate" by Amir Zandieh et al. (Google Research, ICLR 2026)](https://openreview.net/forum?id=V2pWwM6xK4). Unlike traditional Product Quantization (PQ) which requires a k-means training phase to build a static codebook, TurboQuant is training-free, enabling immediate vector insertion and dynamic indexing. The mathematical transformation proceeds in three steps:
//!
//! 1. **Randomized Rotation**: The input vector <span class="math-inline">x \in \mathbb{R}^d</span> is rotated using a randomized Hadamard matrix <span class="math-inline">H_D = H \cdot D</span> (where <span class="math-inline">D</span> is a diagonal matrix with independent random signs <span class="math-inline">\pm 1</span>) to distribute the coordinate variance uniformly:
//!
//! ```math
//! \tilde{x} = \frac{1}{\sqrt{d}} H D x
//! ```
//!
//! 2. **Scalar Quantization**: The rotated vector components <span class="math-inline">\tilde{x}_i</span> are quantized to a <span class="math-inline">b</span>-bit width (e.g., <span class="math-inline">b=4</span> bits) using uniform scalar quantization.
//!
//! 3. **Quantized Johnson-Lindenstrauss (QJL) Residual Correction**: The residual error <span class="math-inline">e = \tilde{x} - Q(\tilde{x})</span> is projected onto a randomized 1-bit subspace to ensure unbiased inner product estimation:
//!
//! ```math
//! \langle x, y \rangle \approx \langle Q(\tilde{x}), Q(\tilde{y}) \rangle + \frac{\pi}{2 \sqrt{d}} \langle \text{sgn}(A e_x), \text{sgn}(A e_y) \rangle
//! ```
//!
//! ### 4. Hybrid Search via Reciprocal Rank Fusion (RRF)
//! Lexical and semantic search scores are unified using Reciprocal Rank Fusion, avoiding scoring scale mismatches
//! and prioritizing documents ranked highly by both models. This technique is adapted from the paper
//! ["Reciprocal Rank Fusion out-performs Condorcet and individual algorithms" by Gordon Cormack et al. (University of Waterloo)](https://dl.acm.org/doi/10.1145/1571941.1572114):
//!
//! ```math
//! RRF(d) = \sum_{m \in M} \frac{1}{k + r_m(d)}
//! ```
//!
//! where <span class="math-inline">M = \{\text{Vector Search}, \text{Lexical FTS5}\}</span>, <span class="math-inline">r_m(d)</span> is the 1-indexed rank of document <span class="math-inline">d</span> within
//! system <span class="math-inline">m</span>, and <span class="math-inline">k</span> is a smoothing constant (typically <span class="math-inline">k=60</span>).
//!
//! ---
//!
//! ## Core Subsystems
//!
//! *   **`core`**: The execution engine, responsible for coordinating tool iterations, metric collection, and agent healing.
//! *   **`memory`**: A quantized semantic vector search index (`turbovec`) utilizing offline ONNX embeddings (`fastembed`) and SQLite metadata.
//! *   **`model`**: Language model interfaces, API provider abstractions, and schema-guided validation.
//! *   **`tools`**: Built-in system tools (file I/O, web browsing), sandboxed execution backends (Docker, Local, and metered WebAssembly/WASI sandboxing via `wasmi`), and Model Context Protocol (MCP) client capabilities.
//! *   **`config`**: Credentials, model templates, custom configuration pathways, and setup wizard logic.
//! *   **`cli`**: Prompt input-loop, terminal-adaptive layout renderers, and interactive REPL command routing.
//! *   **`error`**: Unified error hierarchy and healer classification metrics.
//!
//! ---
//!
//! ## Installation & Setup
//! Helix is fully supported across macOS, Linux, and Windows:
//!
//! 1. **Compile and Run from Source**:
//!    ```bash
//!    cargo build --release
//!    cargo run --release
//!    ```
//! 2. **Script / Container Installation**:
//!    For ready-to-use curl installers (macOS & Linux), PowerShell setups (Windows), or containerized deployments (Docker), please refer to the comprehensive [Installation Guide](https://github.com/poornachandra24/Helix/blob/main/docs/installation.md) or the repository `README.md`.
//!
//! 3. **Config & Data Directories**:
//!    *   **Linux**: Config is located at `~/.config/helix/config.toml`, database and custom skills under `~/.local/share/helix/`.
//!    *   **macOS**: Config is located at `~/Library/Preferences/com.helix.helix/config.toml`, database and custom skills under `~/Library/Application Support/com.helix.helix/`.
//!    *   **Windows**: Config is located at `%APPDATA%\helix\helix\config\config.toml`, database and custom skills under `%APPDATA%\helix\helix\data\`.
//!
//! ---
//!
//! ## Model Context Protocol (MCP) Support
//!
//! Helix natively supports the Model Context Protocol (MCP) via the stdio transport layer. External tools
//! can be dynamically spawned and registered at boot time by specifying server configurations in an
//! `mcp_config.json` file. MCP tools are wrapped and registered in the unified tools module, enabling
//! seamless interaction between the language model and third-party context servers.
//!
//! ## Open Source Credits & Dependencies
//!
//! Helix is built on top of many incredible open-source libraries and crates from the Rust ecosystem:
//!
//! *   **Semantic Indexing**: `fastembed` for local ONNX text embeddings, and [turbovec](https://github.com/RyanCodrai/turbovec) by Ryan Codrai for fast, memory-mapped vector search.
//! *   **Self-Optimizing Neural Architecture (SONA)**: `ruvector-sona` for runtime-adaptive context query alignment.
//! *   **Model Context Protocol (MCP)**: `rmcp` client capabilities for spawning context and tool servers.
//! *   **WASM Sandboxing**: `wasmi` and `wasmi_wasi` for running secure, isolated WebAssembly client code.
//! *   **Terminal UI Orchestration**: `crossterm` for cross-platform raw console support, `termimad` for Markdown rendering, `dialoguer` for interactive setups, and `comfy-table` for output alignment.
//! *   **Async Runtime**: `tokio` and `reqwest` powering asynchronous network operations and API dispatch.
//!
//! ---
//!
//! ## Technical Roadmap
//!
//! The following capabilities are under active research and development:
//!
//! ### 1. OTLP Tracing & Observability (helix-telemetry)
//! Integrate native OpenTelemetry (`opentelemetry` & `tracing-opentelemetry`) to export spans for model latencies, vector DB search queries, and sandbox executions to external collectors (e.g. Jaeger, Honeycomb).
//!
//! ### 2. WASM Sandbox Resource Profiling (helix-profiler)
//! Expose WASM guest execution resource statistics (fuel/gas consumed via `wasmi` and sandbox directory disk footprint) directly into the telemetry subscriber.
//!
//! ### 3. TUI Performance Overlay (helix-tui)
//! Add a lightweight, interactive terminal visualization showing real-time token budgets, memory retrieval quality scores, and tool execution cycles.
//!
//! ### 4. Multimodal Input Support (helix-multimodal)
//! Refactor the inner message structure from a plain `String` to content-part enums, allowing users to attach screenshots or file buffers, and update the API drivers (Ollama/OpenAI) to support vision models.
//!
//! ### 5. WASM/WASI Plugin SDK (helix-sdk)
//! Develop a structured SDK and manifest format allowing developers to write custom tools in Rust, C, or Go, compile them to WebAssembly, and dynamically register them in the sandbox.
//!
//! ### 6. Dynamic SLM-Based Context Compaction (helix-compaction)
//! Implement smart context compression utilizing a local Small Language Model (SLM) to summarize old chat turns instead of simple turn omission.
//!
//! ### 7. Provider-Agnostic Heterogeneous Multi-Agent Orchestration (helix-orchestrate)
//! Spawning concurrent, isolated child agents inside separate WASI sandboxes, utilizing smaller, specialized models or large models across heterogeneous providers (OpenAI, Anthropic, Ollama, Groq) to collaborate on sub-tasks. The orchestration engine dynamically handles task routing and communication, abstracting away model selection and provider configurations so the user can focus purely on task outcomes regardless of token consumption.

#![allow(clippy::missing_safety_doc)]

pub mod cli;
pub mod config;
pub mod core;
pub mod error;

#[cfg(any(target_os = "linux", target_os = "macos"))]
extern crate blas_src;

pub mod memory;
pub mod model;
pub mod tools;

// Compatibility shims for GLIBC 2.38 C23 string parsing functions to run
// precompiled ONNX Runtime on older GLIBC versions (e.g. Ubuntu 22.04 with GLIBC 2.35)
unsafe extern "C" {
    fn strtoll(
        nptr: *const std::ffi::c_char,
        endptr: *mut *mut std::ffi::c_char,
        base: std::ffi::c_int,
    ) -> std::ffi::c_longlong;
    fn strtoull(
        nptr: *const std::ffi::c_char,
        endptr: *mut *mut std::ffi::c_char,
        base: std::ffi::c_int,
    ) -> std::ffi::c_ulonglong;
    fn strtol(
        nptr: *const std::ffi::c_char,
        endptr: *mut *mut std::ffi::c_char,
        base: std::ffi::c_int,
    ) -> std::ffi::c_long;
}

#[doc(hidden)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_strtoll(
    nptr: *const std::ffi::c_char,
    endptr: *mut *mut std::ffi::c_char,
    base: std::ffi::c_int,
) -> std::ffi::c_longlong {
    unsafe { strtoll(nptr, endptr, base) }
}

#[doc(hidden)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_strtoull(
    nptr: *const std::ffi::c_char,
    endptr: *mut *mut std::ffi::c_char,
    base: std::ffi::c_int,
) -> std::ffi::c_ulonglong {
    unsafe { strtoull(nptr, endptr, base) }
}

#[doc(hidden)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn __isoc23_strtol(
    nptr: *const std::ffi::c_char,
    endptr: *mut *mut std::ffi::c_char,
    base: std::ffi::c_int,
) -> std::ffi::c_long {
    unsafe { strtol(nptr, endptr, base) }
}
