# Installation & Setup Guide

This guide details how to install, configure, and run the Helix CLI as an end-user, or set up a local workspace as a developer.

---

## 👥 1. End-User Installation

Choose the installation method that best fits your operating system or deployment style.

### Option A: macOS & Linux (Shell Script)
This is the recommended approach for macOS and Linux systems. The script automatically detects your CPU architecture (Intel or Apple Silicon), downloads the correct precompiled binary, extracts it to `~/.local/bin`, and launches the setup wizard:
```bash
curl -fsSL https://raw.githubusercontent.com/poornachandra24/Helix/main/install.sh | sh
```
> [!NOTE]
> The precompiled Linux binary requires **GLIBC 2.35 or newer** (found in Ubuntu 22.04+, Debian 12+, Fedora 36+, etc.). If your system runs an older version of glibc, please use the **Docker** installation method below.

### Option B: Windows (PowerShell Script)
For native Windows environments, run this command in PowerShell (Administrator privileges are not required):
```powershell
irm https://raw.githubusercontent.com/poornachandra24/Helix/main/install.ps1 | iex
```
This downloads the native Windows MSVC binary, extracts it to `$HOME\.helix\bin\`, and appends the directory to your user `PATH` environment variable.

### Option C: Docker (Containerized Execution)
To run Helix inside a secure container without installing binaries directly on your host machine:

**macOS & Linux:**
```bash
docker run -it --rm \
  --user "$(id -u):$(id -g)" \
  -v "$(pwd)":/workspace \
  -e HELIX_HOME=/workspace/.helix \
  ghcr.io/poornachandra24/helix:latest
```

**Windows (PowerShell):**
```powershell
docker run -it --rm `
  -v "${PWD}:/workspace" `
  -e HELIX_HOME=/workspace/.helix `
  ghcr.io/poornachandra24/helix:latest
```

---

## 🛠️ 2. Developer Workspace Setup (Building from Source)

If you want to contribute to Helix, develop custom plugins, or build directly from the latest source code, follow these steps.

### 📋 Prerequisites
Before compiling, ensure you have the Rust toolchain installed.

*   **Rust (edition 2024)**: Install via [rustup](https://rustup.rs/):
    ```bash
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    ```

*   **Linux Dependencies (OpenBLAS & gfortran)**:
    On Linux systems, similarity search operations are accelerated via static linking to OpenBLAS. Install the following libraries:
    ```bash
    # Ubuntu / Debian
    sudo apt-get update && sudo apt-get install -y libopenblas-dev gfortran
    
    # Fedora / RHEL
    sudo dnf install -y openblas-devel gcc-gfortran
    ```

*   **macOS Dependencies**:
    Helix utilizes Apple's native **Accelerate** framework for matrix math acceleration. No extra libraries are required.

### 🔨 Compiling and Running
1.  **Clone the Repository**:
    ```bash
    git clone https://github.com/poornachandra24/Helix.git
    cd Helix
    ```

2.  **Build in Release Mode**:
    ```bash
    cargo build --release
    ```
    This outputs the compiled binary to `target/release/helix`.

3.  **Run the REPL**:
    ```bash
    cargo run --release
    ```

4.  **Run the Test Suite**:
    Verify your environment is set up correctly:
    ```bash
    cargo test --all-targets --all-features
    ```

---

## ⚙️ 3. Interactive Configuration

Regardless of how you installed the binary, running `helix` for the first time without a configuration file launches the interactive setup wizard:

```text
   ██╗  ██╗███████╗██╗     ██╗██╗  ██╗
   ██║  ██║██╔════╝██║     ██║╚██╗██╔╝
   ███████║█████╗  ██║     ██║ ╚███╔╝ 
   ██╔══██║██╔══╝  ██║     ██║ ██╔██╗ 
   ██║  ██║███████╗███████╗██║██╔╝ ██╗
   ╚═╝  ╚═╝╚══════╝╚══════╝╚═╝╚═╝  ╚═╝

  Helix Setup Wizard
  ─────────────────────────────────────────
  Let's configure your primary provider and model.

? Choose a Provider Template
❯ Ollama (Local)
  OpenRouter
  Groq
  OpenAI
  Gemini
  DeepSeek
  Custom (OpenAI-compatible)

? Base URL: http://localhost:11434
? API Key for Ollama (Local) (Enter to skip):
? Model Name: qwen3:4b

Configuration updated and saved!
```

You can re-trigger this setup wizard at any time from inside the terminal using:
```bash
helix config
```

---

## 📂 4. Configuration Directories

Helix maintains configuration and indexing states under standard user directories:

| OS | Configuration File | Vector Database & Skills |
| --- | --- | --- |
| **Linux** | `~/.config/helix/config.toml` | `~/.local/share/helix/` |
| **macOS** | `~/Library/Preferences/com.helix.helix/config.toml` | `~/Library/Application Support/com.helix.helix/` |
| **Windows** | `%APPDATA%\helix\helix\config\config.toml` | `%APPDATA%\helix\helix\data\` |

### Customizing active instructions (Skills)
Store guidelines, style guides, or API specifications in markdown or plain-text files under the `skills/` subdirectory within the data directories listed above. These are automatically loaded at boot time.
