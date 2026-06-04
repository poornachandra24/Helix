# Helix Deployment Guide

This guide details how to deploy and configure Helix across different runtime environments: Local, Docker containers, and WebAssembly (Wasm) guests.

---

## 1. Local Execution & Development

Running Helix natively on the host machine provides maximum speed and full access to system resources.

### Prerequisites
*   Rust 2024 edition (rustc 1.82+)
*   Docker (optional, for sandboxed mode)
*   Nvidia/AMD BLAS runtimes (optional, for fast vector operations)

### Building the Binary
```bash
cargo build --release
```
The compiled executable is created at `target/release/helix`.

### Session & Configuration Files
Helix configures itself in your home directory under the following locations:
*   **Config File**: `~/.config/helix/config.toml` (contains API keys, active provider, model selections, and thinking levels).
*   **Vector Database**: `~/.config/helix/memory/` (SQLite DB and embedding indices).
*   **Active Skills**: `~/.config/helix/skills/` (Markdown extension skills).

---

## 2. Stateful Docker Sandbox Environment

For secure, isolated code execution, configure Helix to run in **Docker Sandbox Mode**.

### 2.1 Activation
In the REPL, switch sandbox modes:
```
[Ctx: 1.0%] > /config
(Select Sandbox Mode -> Docker)
```
Or edit your `~/.config/helix/config.toml`:
```toml
sandbox_mode = "docker"
```

### 2.2 Daemon Container Setup
When sandboxing is set to `Docker`, Helix launches a background container (`rust:latest`) on startup:
*   **Workspace Mapping**: Mapped from the host's current directory to `/workspace` inside the container.
*   **Cache Mounting**: If present, local cargo cache directories (`~/.cargo/registry` and `~/.cargo/git`) are mounted into the container to prevent downloading dependencies repeatedly.
*   **User Ownership**: Maps container operations to the host's exact UID/GID to avoid writing root-owned files to the host.

### 2.3 Resource Boundaries
The daemon container enforces strict hardware resource allocations:
```bash
docker run -d --rm \
  --memory 512m \
  --cpus 1.0 \
  -v $CWD:/workspace \
  rust:latest tail -f /dev/null
```

---

## 3. WebAssembly (Wasm) Execution

For ultra-lightweight, near-instant sandboxed execution without Docker overhead, Helix supports a **Wasm Sandbox Mode**.

### 3.1 Execution Engine (`wasmi`)
Helix uses the `wasmi` interpreter to execute compiled WebAssembly modules locally:
1.  **Jail Directory**: Restricts Wasm operations to a localized `wasm_jail/` folder.
2.  **No Host Imports**: Disables raw host access imports, allowing execution strictly in a memory-isolated Guest VM.

### 3.2 Running a Module
Use the `wasm_execute` tool by placing your compiled WebAssembly file (`.wasm`) in the workspace:
*   Helix loads the binary bytes into `wasmi::Module`.
*   Looks for an entrypoint function (`_start`, `main`, or exported routines).
*   Runs the code to completion and returns the execution status.
