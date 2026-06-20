//! # Helix
//!
//! Helix is a high-performance, self-optimizing autonomous AI agent harness and library built natively in Rust.
//! Designed as an interactive workspace companion, Helix provides fine-grained control over agent execution
//! loops, context window budget alignment, session persistence, and secure command execution.
//!
//! ## Core Subsystems
//!
//! *   **`core`**: The execution engine, responsible for coordinating tool iterations, metric collection, and agent healing.
//! *   **`memory`**: A quantized semantic vector search index (`turbovec`) utilizing offline ONNX embeddings (`fastembed`) and SQLite metadata.
//! *   **`model`**: Language model interfaces, API provider abstractions, and schema-guided validation.
//! *   **`tools`**: Built-in system tools (file I/O, web browsing), sandboxed execution backends (Docker, Local), and Model Context Protocol (MCP) client capabilities.
//! *   **`config`**: Credentials, model templates, custom configuration pathways, and setup wizard logic.
//! *   **`cli`**: Prompt input-loop, terminal-adaptive layout renderers, and interactive REPL command routing.
//! *   **`error`**: Unified error hierarchy and healer classification metrics.
//!
//! ## Self-Optimizing Neural Architecture (SONA) Integration
//!
//! Helix integrates the SONA engine to dynamically optimize context retrieval queries over time:
//!
//! *   **Two-Tier LoRA**: Applies real-time, low-rank adaptations. A fast MicroLoRA layer handles instant, turn-by-turn adjustments based on immediate feedback, while a deeper BaseLoRA layer consolidates long-term semantic patterns.
//! *   **EWC++ (Elastic Weight Consolidation)**: Protects critical weights from catastrophic forgetting as the agent learns from new codebase interactions.
//! *   **ReasoningBank**: Classifies query trajectories (using K-means++ clustering) to map incoming user prompts to historical centroids that previously yielded high-quality tool execution.
//!
//! ### Credits and Official Resources:
//! *   **Developers**: Developed by the RuVector Team (<team@ruvnet.dev>).
//! *   **Repository**: Coded as part of the [RuVector Ecosystem](https://github.com/ruvnet/ruvector).
//! *   **Crate**: Distributed via [ruvector-sona](https://crates.io/crates/ruvector-sona).
//!
//! ## Model Context Protocol (MCP) Support
//!
//! Helix natively supports the Model Context Protocol (MCP) via the stdio transport layer. External tools
//! can be dynamically spawned and registered at boot time by specifying server configurations in an
//! `mcp_config.json` file. MCP tools are wrapped and registered in the unified tools module, enabling
//! seamless interaction between the language model and third-party context servers.

//!
//! ## Technical Roadmap
//!
//! The following capabilities are under active research and development:
//!
//! ### 1. Streamable Terminal Voice-to-Voice (helix-voice)
//! Direct capture of local microphone input streaming text prompts, paired with lightweight local TTS model
//! integration to synthesize and stream spoken responses directly back to host audio outputs.
//!
//! ### 2. Multimodal Visual Validation (helix-vision)
//! Adding image and visual context processing into the evaluation loop, allowing the agent to audit UI designs,
//! inspect screenshots, and read engineering schematics.
//!
//! ### 3. Hierarchical Parallel Sub-Agents (helix-orchestrator)
//! Spawning concurrent, isolated child sub-agents to solve independent sub-tasks in parallel (e.g. running a test
//! suite in one sandbox while scraping docs in another), coordinated by a central parent agent.
//!
//! ### 4. Dynamic MCP Self-Configuration & Hot-Reloading (helix-reload)
//! Enabling the agent to programmatically configure external MCP servers in `mcp_config.json` mid-session,
//! matched with an atomic transaction scheduler that kills old subprocesses and registers new tool capabilities
//! safely at turn boundaries without interrupting active execution.
//!
//! ### 5. Session Post-Mortem Reflective Memory (helix-reflect)
//! Generating a concise, high-signal Markdown summary ("lessons learned ledger") at the end of each session,
//! detailing goals, verified working commands, and resolved blockers. These summaries are semantically indexed
//! in the vector database to build cross-session intelligence and prevent repetitive mistakes.

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
