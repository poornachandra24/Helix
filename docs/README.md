# Helix Documentation

Welcome to the Helix developer documentation. This directory contains detailed architectural explanations, data flow diagrams, and sequence diagrams for the core subsystems of the Helix autonomous agent.

## Subsystems

1.  **[Local Semantic Memory Architecture](memory_architecture.md)**
    *   Details the dual-store setup combining SQLite (metadata) and a 4-bit Lloyd-Max quantized SIMD vector index (`turbovec`).
    *   Includes data flow, initialization, retrieval, and turn-committing flowcharts and sequence diagrams.
    *   Contains memory footprint profiles (baseline ~227 MiB RSS), thread pool resource allocation analysis, and verification commands.

2.  **[Model Context Protocol (MCP) Client Subsystem](mcp_integration.md)**
    *   Explains how external tool servers are dynamically loaded at runtime via standard stdin/stdout JSON-RPC pipes.
    *   Includes process life-cycle sequence diagrams and configuration blueprints.

3.  **[Terminal UI Design System](ux_design.md)**
    *   Covers dynamic terminal-width layout, box math formulas, color token definitions, and word-wrap implementation.
    *   Documents the two-tone conversation box scheme (blue `You` / yellow `Helix`), post-response footnote format, SONA quality footnote, and agent loop counter.
    *   Explains why top borders are split into separate `print!` calls to prevent ANSI color bleed.

4.  **[Skill Registry Subsystem](skills.md)**
    *   Details the scanning and loading of modular `.md` and `.txt` instruction files from the user configuration folder.
    *   Explains how domain-specific behaviors are dynamically injected into the system prompt context.

## Guides & Deployment

5.  **[Core Architecture Overview](architecture.md)**
    *   An in-depth map of orchestration loops, self-healing parser mechanisms, exact context budgeting, and container sandboxing models.

6.  **[Plugin & Extension Development](plugin_development.md)**
    *   A guide on building custom native Rust tools, configuring external MCP servers, and writing natural-language Markdown skills.

7.  **[Deployment Guide](edge_deployment.md)**
    *   Instructions for native host setups, stateful container sandboxes with resource limits, and permissionless guest VM WASM runs.

8.  **[Versioning & Contribution Guide](contributing.md)**
    *   Guidelines on semantic versioning, distribution pipelines, user installation commands, and developer pull request lifecycle checklists.
