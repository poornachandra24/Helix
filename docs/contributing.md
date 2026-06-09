# Versioning, Shipping, and Contribution Guide

This document outlines the release versioning strategies, publication channels, installation flows, and contribution guidelines for the Helix CLI ecosystem.

---

## 1. Semantic Versioning Policy

Helix strictly adheres to [Semantic Versioning 2.0.0](https://semver.org/). Version numbers follow the `MAJOR.MINOR.PATCH` format:

*   **MAJOR** version increments denote incompatible API changes:
    *   Breaking changes to the `Tool` trait or the core `SandboxBackend` interfaces.
    *   Breaking modifications to the SQLite database schema without automatic backward-compatible migrations.
    *   Major refactoring of the configuration layout (`config.toml`) that renders older configurations invalid.
*   **MINOR** version increments add backward-compatible functionality:
    *   Adding new commands (e.g., `/thinking`) or new options to `/config`.
    *   Introducing new built-in tools or sandbox backends.
    *   Extending the metrics engine or the SONA embedding projection capability.
*   **PATCH** version increments introduce backward-compatible bug fixes:
    *   Resolving terminal display bugs, edge cases in token estimation, or path validation errors.
    *   Security patches or dependency updates.

---

## 2. Shipping & Publication Pipeline

Helix is distributed through three primary channels:

### 2.1 Crates.io (Source and Library)
Helix is published as a cargo binary and library package:
```bash
cargo publish
```
This allows developers to build and install it locally using standard Cargo commands.

### 2.2 Precompiled Static Binaries (GitHub Releases)
For users without a local Rust toolchain, a CI/CD workflow (GitHub Actions) builds static binaries on every tag release:
*   **Linux**: `x86_64-unknown-linux-gnu` (dynamically linked against OpenBLAS).
*   **macOS**: Universal binaries supporting both Apple Silicon (`aarch64-apple-darwin`) and Intel (`x86_64-apple-darwin`).
*   **Windows**: Statically linked MSVC binaries (`x86_64-pc-windows-msvc`).

### 2.3 Pre-built Docker Images
Docker images are built and pushed to the GitHub Container Registry (`ghcr.io/poornachandra24/helix`):
*   Includes a pre-configured environment with Rust, Node.js, and Python runtimes so the agent can execute code immediately without installing dependencies on the host.

---

## 3. Getting Started for New Users

### Installation
Users can install Helix using one of the following standard methods:

**macOS & Linux (Shell Script)**
Downloads the correct precompiled binary, extracts it, installs it to `~/.local/bin`, and offers to configure/launch Helix:
```bash
curl -fsSL https://raw.githubusercontent.com/poornachandra24/Helix/main/install.sh | sh
```

**Windows (PowerShell)**
Downloads the native MSVC executable, installs it to `~/.helix/bin`, updates the user `PATH` environment variable, and offers to configure/launch Helix:
```powershell
irm https://raw.githubusercontent.com/poornachandra24/Helix/main/install.ps1 | iex
```

**Via Cargo (Rust developers)**
```bash
cargo install helix
```

### Initial Configuration
On first execution (or by running `helix config`), Helix launches an interactive command-line setup wizard to configure your preferred LLM provider, API base URL, API keys, and model selections.

If you prefer to configure it manually, you can edit the generated file at `~/.config/helix/config.toml` (or `%USERPROFILE%\.config\helix\config.toml` on Windows):

```toml
active_provider = "anthropic"
active_model = "claude-3-5-sonnet"
sandbox_mode = "docker"

[providers.anthropic]
api_key = "sk-ant-..."
```

To run the interactive CLI chat session, invoke:
```bash
helix
```

---

## 4. Contributing Guide for Developers

We welcome contributions from the community! Follow these steps to set up, test, and submit changes.

### 4.1 Development Setup
1.  Clone the repository:
    ```bash
    git clone https://github.com/poornachandra24/helix.git
    cd helix
    ```
2.  Install dependencies: Ensure Rust 1.82+ is installed on the host.

### 4.2 Code Style & Quality Checklists
All pull requests must pass the following sanity checks:

*   **Formatting**: Format code according to rustfmt guidelines:
    ```bash
    cargo fmt --all -- --check
    ```
*   **Linting**: Ensure code conforms to Clippy compiler lints (no warnings allowed):
    ```bash
    cargo clippy --all-targets -- -D warnings
    ```
*   **Testing**: Ensure the entire test suite passes successfully:
    ```bash
    cargo test --all-targets
    ```

### 4.3 Pull Request Lifecycle
1.  **Fork & Branch**: Create a descriptive branch name (e.g., `feature/async-stdin` or `fix/path-traversal`).
2.  **Changelog**: Add a brief entry in `CHANGELOG.md` under the `[Unreleased]` section.
3.  **Submit**: Open a Pull Request targeting the `main` branch. Ensure the automated CI checks (format, clippy, unit tests) pass before requesting a review.
