# Changelog

All notable changes to the Helix project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.5.0](https://github.com/poornachandra24/Helix/compare/v0.4.0...v0.5.0) (2026-11-18)


### Features

* add harness leaderboard compatibility, reasoning_content preservation, and model limits CLI flags ([#30](https://github.com/poornachandra24/Helix/issues/30)) ([8d927ed](https://github.com/poornachandra24/Helix/commit/8d927ed75d6547d4109703d6e4e539286cbc6940))
* Logfire OpenTelemetry, fine-tuning trajectory schema, CI/CD hardening, and CVE remediation ([#26](https://github.com/poornachandra24/Helix/issues/26)) ([6161e7b](https://github.com/poornachandra24/Helix/commit/6161e7b39daa9cb1f127a6b7718c56f527c52279))


### Bug Fixes

* **ruvector-sona:** ensure non-zero latency in trajectory builder test on high-speed macOS runners ([#29](https://github.com/poornachandra24/Helix/issues/29)) ([460fb83](https://github.com/poornachandra24/Helix/commit/460fb83405da24d655fdf84077cb19653b3bf008))

## [Unreleased]

### Added
*   **OpenTelemetry & Logfire Subsystem**: Native tracing pipeline backed by Pydantic Logfire with non-blocking Tokio unbounded channels and synchronized shutdown/flush hooks.
*   **Agent Trajectory Fine-Tuning Schema**: Telemetry spans now capture comprehensive training metadata (`dataset.is_training_signal`, `dataset.quality_tier`, `dataset.tools_used`, `dataset.has_healer_retries`, and observation outputs) to extract clean SFT and DPO datasets for small language models (1B–8B).
*   **In-Memory Embedding Cache**: Integrated an in-memory vector cache in `HelixMemoryEngine` for `fastembed-rs` ONNX embeddings, cutting turn startup latency by ~1.5 seconds.
*   **Multi-Agent Workflow Coordination Tools**: Added consensus verification tools and subagent execution primitives.
*   **Interactive `/telemetry` Commands**: Added REPL commands `/telemetry on`, `/telemetry off`, `/telemetry token <token>`, and `/telemetry status` with persistent storage in `config.toml`.

### Changed
*   **OTel Call-Site Noise Filtering**: Applied targeted `EnvFilter` directly to `otel_layer` (`helix=info,ruvector_sona=info,tokenizers=off,hyper=off,reqwest=off,h2=off,mio=off,want=off,fastembed=warn`), slashing trace volume by >95% and raising signal purity to >85%.

### Security & Dependencies
*   **Vulnerability Remediation (32 Advisories Cleared)**:
    *   Upgraded `aws-lc-sys` to `0.47.0` (resolves RUSTSEC-2026-0045, RUSTSEC-2026-0046, RUSTSEC-2026-0047, RUSTSEC-2026-0048, RUSTSEC-2026-0044).
    *   Upgraded `quinn-proto` to `0.11.19` & `quinn-udp` to `0.5.16` (resolves RUSTSEC-2026-0341, RUSTSEC-2026-0342, RUSTSEC-2026-0338, RUSTSEC-2026-0340, RUSTSEC-2026-0339, RUSTSEC-2026-0524, RUSTSEC-2026-0523, RUSTSEC-2026-0538).
    *   Upgraded `rustls` to `0.23.46` & `rustls-webpki` to `0.103.17` (resolves RUSTSEC-2026-0349, RUSTSEC-2026-0348, RUSTSEC-2026-0346, RUSTSEC-2026-0285, RUSTSEC-2026-0382).
    *   Upgraded `webpki-roots` and `webpki-root-certs` to `1.0.12` (resolves RUSTSEC-2026-0381, RUSTSEC-2026-0383).
    *   Upgraded `h2` to `0.4.20` (resolves RUSTSEC-2026-0258).
    *   Upgraded `crossbeam-epoch` to `0.9.21` (resolves RUSTSEC-2026-0204).
    *   Upgraded `rkyv` to `0.8.18` (resolves RUSTSEC-2026-0235, RUSTSEC-2026-0234, RUSTSEC-2026-0233).
    *   Upgraded `rusqlite` to `0.41.0` and `libsqlite3-sys` to `0.39.1` (resolves RUSTSEC-2026-0459).
    *   Upgraded `rmcp` to `3.10.0` (resolves RUSTSEC-2026-0462).
    *   Removed unused `ruvllm` dependency and its vulnerable `zip 1.1.4` transitive crate (resolves RUSTSEC-2026-0429, RUSTSEC-2026-0369, RUSTSEC-2026-0368, RUSTSEC-2026-0367).
    *   Configured `.cargo/audit.toml` to track and document `cap-primitives 0.26.1` trailing-slash advisory RUSTSEC-2026-0482 alongside RUSTSEC-2024-0445.

---

## [0.3.0](https://github.com/poornachandra24/Helix/compare/v0.2.0...v0.3.0) (2026-06-21)


### Features

* dynamic MCP hot-reloading, post-mortem reflection, and BM25 hybrid search ([#9](https://github.com/poornachandra24/Helix/issues/9)) ([d719d87](https://github.com/poornachandra24/Helix/commit/d719d8769333ec4a83dff730373b33512782d992))

## [0.2.0] - 2026-06-04

### Added
*   **Stateful Sandboxed Execution**: Transitioned `DockerSandbox` from single-use container spawns to a persistent daemon container, retaining environment states, directory paths, and cargo dependencies.
*   **Workspace Traversal Protection**: Implemented a secure path validator (`resolve_and_validate_path`) that normalizes paths, resolves parent-directory escapes (`..`), canonicalizes symlinks, and validates paths strictly within the workspace.
*   **Thinking Level Control**: Added the `/thinking` command to configure and hot-reload reasoning token budgets. Integrates `"reasoning_effort"` for OpenAI endpoints and `"thinking_budget"` options for Gemini/Ollama endpoints.
*   **Premium Confirmation Cards**: Enhanced authorization prompts with a Comfy-Table card layout that highlights agent tool requests, action descriptions, and payload details.
*   **Async Prompt Interruptibility**: Refactored the REPL prompt loop to use asynchronous non-blocking stdin reads. Users can now press `Ctrl+C` at the prompt to exit immediately, or during generation to cancel execution.
*   **Docker Limits**: Configured static CPU (`1.0`) and Memory (`512m`) limits for the sandbox container to prevent host resource exhaustion.
*   **Wasm Sandbox Mode**: Integrated the `wasmi` interpreter backend to run compiled `.wasm` modules inside an isolated, permissionless virtual guest VM.

### Changed
*   **Context Budgeting**: Replaced character-count heuristics with exact `tiktoken-rs` token counting.
*   **Scraper DOM Extraction**: Upgraded HTML tag stripping to a structure-aware DOM parser, discarding script/style blocks and decoding XML entities.
*   **Response UX Streaming**: Replaced laggy text output with real-time word-wrapped streaming lines, hot-swapping into the final `termimad` markdown response card upon completion.

### Fixed
*   **Ctrl+C Stdin Hangs**: Fixed an issue where pressing `Ctrl+C` at the input prompt printed `^C` without exiting or clearing the line due to blocking stdin reads.
*   **Test Isolation**: Aligned integration and sandbox test suites to write temporary files inside the current workspace `.tmp` directory rather than host `/tmp` to satisfy security checks.

---

## [0.1.0] - 2026-05-17
*   Initial Rust port of the Helix autonomous agent harness.
*   Basic REPL session logs, MCP integration skeleton, and Local sandbox backend.
