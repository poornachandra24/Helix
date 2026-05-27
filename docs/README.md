# Helix Documentation

Welcome to the Helix developer documentation. This directory contains detailed architectural explanations, data flow diagrams, and sequence diagrams for the core subsystems of the Helix autonomous agent.

## Subsystems

1.  **[Local Semantic Memory Architecture](memory_architecture.md)**
    *   Details the dual-store setup combining SQLite (metadata) and a 4-bit Lloyd-Max quantized SIMD vector index (`turbovec`).
    *   Includes data flow, initialization, retrieval, and turn-committing flowcharts and sequence diagrams.
    *   Contains memory footprint profiles (baseline ~227 MiB RSS), thread pool resource allocation analysis, and verification commands.

2.  **[Self-Evolution Loop and Security Gates](self_evolution.md)**
    *   Exposes how Helix analyzes its execution logs, runs LLM patch proposals, enforces sandboxed security gates, and benchmarks performance regressions before asking for human approval.
    *   Contains the complete evolutionary state transition sequence diagrams.

3.  **[Model Context Protocol (MCP) Client Subsystem](mcp_integration.md)**
    *   Explains how external tool servers are dynamically loaded at runtime via standard stdin/stdout JSON-RPC pipes.
    *   Includes process life-cycle sequence diagrams and configuration blueprints.

4.  **[Terminal UI Design System](ux_design.md)**
    *   Covers dynamic terminal-width layout, box math formulas, color token definitions, and word-wrap implementation.
    *   Documents the two-tone conversation box scheme (indigo `You` / amber `Helix`), post-response footnote format, SONA quality footnote, and agent loop counter.
    *   Explains why top borders are split into separate `print!` calls to prevent ANSI color bleed.
