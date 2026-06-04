# Changelog

All notable changes to the Helix project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

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
