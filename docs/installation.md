# 📥 Installation & Setup Guide

This guide details how to install, configure, and run Helix CLI across different environments (macOS, Linux, Windows, and Docker).

---

## 1. Quick Install Options

Choose the installation method that best fits your environment:

### macOS & Linux (Shell Script)
This is the recommended approach for macOS and Linux users. It detects your CPU architecture (Intel or Apple Silicon), downloads the correct precompiled binary, extracts it, installs it to `~/.local/bin`, and launches the setup wizard:
```bash
curl -fsSL https://raw.githubusercontent.com/poornachandra24/Helix/main/install.sh | sh
```

> [!NOTE]
> **Compatibility**: The precompiled Linux binary requires **GLIBC 2.35 or newer** (found in Ubuntu 22.04+, Debian 12+, and other recent distributions). If your system runs an older version of glibc, please use the **Docker** installation method below.

### Windows (PowerShell Script)
For native Windows users, run this command in PowerShell (Administrator access is not required):
```powershell
irm https://raw.githubusercontent.com/poornachandra24/Helix/main/install.ps1 | iex
```
This downloads the native Windows MSVC binary, extracts it to `$HOME\.helix\bin\`, and appends the directory to your user `PATH` environment variable.

### Rust Cargo (For Developers)
If you have a local Rust toolchain installed, you can build and install directly from Crates.io:
```bash
# From crates.io (once published)
cargo install helix

# From the git repository directly
cargo install --git https://github.com/poornachandra24/helix.git
```

### Docker (For Containerized Run)
If you prefer not to install any binaries on your host machine, you can pull the official Helix image. The following commands map your current working directory to the container workspace and persist configurations:

**macOS & Linux:**
```bash
docker run -it --rm \
  --user "$(id -u):$(id -g)" \
  -v "$(pwd)":/workspace \
  -e HELIX_HOME=/workspace/.helix \
  ghcr.io/poornachandra24/helix
```

**Windows (PowerShell):**
```powershell
docker run -it --rm `
  -v "${PWD}:/workspace" `
  -e HELIX_HOME=/workspace/.helix `
  ghcr.io/poornachandra24/helix
```

---

## 2. Interactive Setup Wizard

Regardless of how you installed the binary, running `helix` for the first time without a config file will start an interactive setup wizard:

```text
Welcome to Harness CLI! Let's set up your primary provider.

? Choose a Provider Template
❯ Ollama (Local)
  Ollama Cloud
  Groq
  OpenAI
  Gemini
  Custom (OpenAI-compatible)

? Base URL: http://localhost:11434
? API Key for Ollama (Local) (Enter to skip):
? Model Name: qwen3:4b

✅ Configuration updated and saved!
```

You can re-trigger this setup wizard at any time using:
```bash
helix config
```

---

## 3. Manual Configuration

Helix configures itself in your home directory under the following locations:
*   **Config File**: `~/.config/helix/config.toml` (or `%USERPROFILE%\.config\helix\config.toml` on Windows)
*   **Vector Database**: `~/.config/helix/memory/`
*   **Active Skills**: `~/.config/helix/skills/`

### Configuration Blueprint (`config.toml`)
If you prefer to write your configuration file manually, create or edit the file with the following format:

```toml
active_provider = "anthropic"
active_model = "claude-3-5-sonnet"
sandbox_mode = "docker"

[providers.anthropic]
api_key = "sk-ant-..."
base_url = "https://api.anthropic.com/v1"
api_format = "openai_compatible"

[providers.ollama]
base_url = "http://localhost:11434"
api_format = "ollama_native"
```

---

## 4. Troubleshooting & Verification

To verify that your installation is running correctly and diagnose connection issues, run:
```bash
helix --version
```
To check provider statuses and verify active configurations from inside the REPL, run the `/status` command:
```text
[Ctx: 0.0%] > /status
```
