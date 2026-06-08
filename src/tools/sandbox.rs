use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::process::Command;

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SandboxMode {
    #[default]
    Local,
    Docker,
    Wasm,
    SSH,
}

#[derive(Debug, Clone)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[async_trait]
pub trait SandboxBackend: Send + Sync {
    async fn execute_command(&self, cmd: &str) -> Result<CommandResult>;
    async fn read_file(&self, path: &str) -> Result<String>;
    async fn write_file(&self, path: &str, content: &str) -> Result<()>;
    async fn list_dir(&self, path: &str, max_depth: usize) -> Result<String>;
}

// ──────────────────────────────────────────────
// Local Host Sandbox
// ──────────────────────────────────────────────

pub struct LocalSandbox;

#[async_trait]
impl SandboxBackend for LocalSandbox {
    async fn execute_command(&self, cmd: &str) -> Result<CommandResult> {
        let output = if cfg!(target_os = "windows") {
            Command::new("cmd").arg("/c").arg(cmd).output().await?
        } else {
            Command::new("sh").arg("-c").arg(cmd).output().await?
        };
        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        Ok(CommandResult {
            exit_code,
            stdout,
            stderr,
        })
    }

    async fn read_file(&self, path: &str) -> Result<String> {
        let current_dir = std::env::current_dir()?;
        let real_path = resolve_and_validate_path(&current_dir, path)?;
        let content = tokio::fs::read_to_string(&real_path)
            .await
            .map_err(|e| anyhow::anyhow!("Cannot read file '{}': {}", path, e))?;
        Ok(content)
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let real_path = resolve_and_validate_path(&current_dir, path)?;
        if let Some(parent) = real_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(real_path, content).await?;
        Ok(())
    }

    async fn list_dir(&self, path: &str, max_depth: usize) -> Result<String> {
        let current_dir = std::env::current_dir()?;
        let real_path = resolve_and_validate_path(&current_dir, path)?;
        let mut out = String::new();
        list_dir_recursive(&real_path, 0, max_depth, &mut out)?;
        Ok(out)
    }
}

// ──────────────────────────────────────────────
// Docker Container Sandbox (Stateful & Persistent)
// ──────────────────────────────────────────────

pub struct DockerSandbox {
    container_workspace: String,
}

impl Default for DockerSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl DockerSandbox {
    pub fn new() -> Self {
        Self {
            container_workspace: "/workspace".to_string(),
        }
    }

    fn get_container_name(&self) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let current_dir = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
        let mut hasher = DefaultHasher::new();
        current_dir.hash(&mut hasher);
        format!("helix-sandbox-{}", hasher.finish())
    }

    async fn ensure_container_running(&self, container_name: &str) -> Result<()> {
        let inspect_output = std::process::Command::new("docker")
            .arg("inspect")
            .arg("--format")
            .arg("{{.State.Running}}")
            .arg(container_name)
            .output();

        let is_running = match inspect_output {
            Ok(output) => {
                let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
                s == "true"
            }
            Err(_) => false,
        };

        if !is_running {
            // Container not running or doesn't exist. Clean up just in case.
            let _ = std::process::Command::new("docker")
                .arg("rm")
                .arg("-f")
                .arg(container_name)
                .output();

            let current_dir =
                std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
            let current_dir_str = current_dir.to_string_lossy().into_owned();

            let uid = std::process::Command::new("id")
                .arg("-u")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|_| "1000".to_string());
            let gid = std::process::Command::new("id")
                .arg("-g")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_else(|_| "1000".to_string());

            let mut start_cmd = std::process::Command::new("docker");
            start_cmd
                .arg("run")
                .arg("-d")
                .arg("--name")
                .arg(container_name)
                .arg("--rm")
                .arg("--memory")
                .arg("512m")
                .arg("--cpus")
                .arg("1.0");

            start_cmd
                .arg("-v")
                .arg(format!("{}:/workspace", current_dir_str));

            if let Some(base_dirs) = directories::BaseDirs::new() {
                let cargo_home = base_dirs.home_dir().join(".cargo");
                let registry = cargo_home.join("registry");
                let git = cargo_home.join("git");
                if registry.exists() {
                    start_cmd.arg("-v").arg(format!(
                        "{}:/usr/local/cargo/registry",
                        registry.to_string_lossy()
                    ));
                }
                if git.exists() {
                    start_cmd
                        .arg("-v")
                        .arg(format!("{}:/usr/local/cargo/git", git.to_string_lossy()));
                }
            }

            if !cfg!(target_os = "windows") {
                start_cmd.arg("--user").arg(format!("{}:{}", uid, gid));
            }

            start_cmd
                .arg("rust:latest")
                .arg("tail")
                .arg("-f")
                .arg("/dev/null");

            let output = start_cmd.output();
            match output {
                Ok(out) if out.status.success() => {
                    // Give container a moment to initialize
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                Ok(out) => {
                    let err_msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    anyhow::bail!("Failed to start stateful Docker container: {}", err_msg);
                }
                Err(e) => {
                    anyhow::bail!("Failed to execute docker run command: {}", e);
                }
            }
        }
        Ok(())
    }

    fn translate_path(&self, path: &str) -> Result<PathBuf> {
        let current_dir = std::env::current_dir()?;
        let target = Path::new(path);

        if target.is_absolute() && target.starts_with(&current_dir) {
            return resolve_and_validate_path(&current_dir, path);
        }

        let path_str = path.replace('\\', "/");
        let path_to_validate = if path_str.starts_with(&self.container_workspace) {
            path_str
                .strip_prefix(&self.container_workspace)
                .unwrap_or(&path_str)
                .to_string()
        } else if path_str.starts_with('/') {
            path_str.strip_prefix('/').unwrap_or(&path_str).to_string()
        } else {
            path.to_string()
        };

        let path_to_validate = path_to_validate.trim_start_matches('/').to_string();
        resolve_and_validate_path(&current_dir, &path_to_validate)
    }
}

#[async_trait]
impl SandboxBackend for DockerSandbox {
    async fn execute_command(&self, cmd: &str) -> Result<CommandResult> {
        let container_name = self.get_container_name();
        self.ensure_container_running(&container_name).await?;

        // Construct wrapper to persist environment variables and directory state
        let wrapper = format!(
            "if [ -f /workspace/.helix_sandbox_cwd ]; then cd \"$(cat /workspace/.helix_sandbox_cwd)\" || cd /workspace; else cd /workspace; fi; \
             if [ -f /workspace/.helix_sandbox_env ]; then . /workspace/.helix_sandbox_env; fi; \
             {}; \
             status=$?; \
             pwd > /workspace/.helix_sandbox_cwd; \
             export -p > /workspace/.helix_sandbox_env; \
             echo -n \"HELIX_DIR:\"; cat /workspace/.helix_sandbox_cwd; \
             exit $status",
            cmd
        );

        let mut exec_cmd = Command::new("docker");
        exec_cmd
            .arg("exec")
            .arg(&container_name)
            .arg("sh")
            .arg("-c")
            .arg(&wrapper);

        let output = exec_cmd.output().await?;
        let exit_code = output.status.code().unwrap_or(-1);
        let stdout_raw = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        // Extract the HELIX_DIR line and clean up stdout
        let mut stdout_clean = String::new();
        for line in stdout_raw.lines() {
            if !line.starts_with("HELIX_DIR:") {
                stdout_clean.push_str(line);
                stdout_clean.push('\n');
            }
        }
        if !stdout_raw.ends_with('\n') && stdout_clean.ends_with('\n') {
            stdout_clean.pop();
        }

        Ok(CommandResult {
            exit_code,
            stdout: stdout_clean,
            stderr,
        })
    }

    async fn read_file(&self, path: &str) -> Result<String> {
        let real_path = self.translate_path(path)?;
        let content = tokio::fs::read_to_string(real_path)
            .await
            .map_err(|e| anyhow::anyhow!("Cannot read file '{}' in sandbox: {}", path, e))?;
        Ok(content)
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<()> {
        let real_path = self.translate_path(path)?;
        if let Some(parent) = real_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(real_path, content).await?;
        Ok(())
    }

    async fn list_dir(&self, path: &str, max_depth: usize) -> Result<String> {
        let real_path = self.translate_path(path)?;
        let mut out = String::new();
        list_dir_recursive(&real_path, 0, max_depth, &mut out)?;
        Ok(out)
    }
}

// ──────────────────────────────────────────────
// WASM Container Sandbox (Secure Guest VM)
// ──────────────────────────────────────────────

pub struct WasmSandbox {
    jail_dir: PathBuf,
}

impl Default for WasmSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl WasmSandbox {
    pub fn new() -> Self {
        let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let jail_dir = current_dir.join("wasm_jail");
        let _ = std::fs::create_dir_all(&jail_dir);
        Self { jail_dir }
    }

    fn translate_path(&self, path: &str) -> Result<PathBuf> {
        let path_str = path.replace('\\', "/");
        let path_to_validate = if path_str.starts_with('/') {
            path_str.trim_start_matches('/').to_string()
        } else {
            path.to_string()
        };
        resolve_and_validate_path(&self.jail_dir, &path_to_validate)
    }
}

#[async_trait]
impl SandboxBackend for WasmSandbox {
    async fn execute_command(&self, cmd: &str) -> Result<CommandResult> {

        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            anyhow::bail!("No WASM program specified");
        }

        let wasm_file = parts[0];
        let args = parts[1..]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<String>>();

        let real_wasm_path = self.translate_path(wasm_file)?;
        if !real_wasm_path.exists() {
            anyhow::bail!("WASM file '{}' not found in sandbox jail", wasm_file);
        }

        let wasm_bytes = tokio::fs::read(&real_wasm_path).await?;
        let jail_dir_clone = self.jail_dir.clone();

        let result = tokio::task::spawn_blocking(move || -> Result<CommandResult> {
            let mut config = wasmi::Config::default();
            config.consume_fuel(true);
            let engine = wasmi::Engine::new(&config);
            let module = wasmi::Module::new(&engine, &mut &wasm_bytes[..])
                .map_err(|e| anyhow::anyhow!("Failed to compile WASM module: {}", e))?;

            let stdout_pipe = wasi_common::pipe::WritePipe::new_in_memory();
            let stderr_pipe = wasi_common::pipe::WritePipe::new_in_memory();

            let dir = wasmi_wasi::Dir::open_ambient_dir(&jail_dir_clone, wasmi_wasi::ambient_authority())
                .map_err(|e| anyhow::anyhow!("Failed to open jail directory for WASM: {}", e))?;

            let mut wasi_builder = wasmi_wasi::WasiCtxBuilder::new()
                .stdout(Box::new(stdout_pipe.clone()))
                .stderr(Box::new(stderr_pipe.clone()))
                .preopened_dir(dir, ".")?;

            for arg in args {
                wasi_builder = wasi_builder.arg(&arg)?;
            }

            let wasi_ctx = wasi_builder.build();
            let mut store = wasmi::Store::new(&engine, wasi_ctx);
            store.add_fuel(50_000_000).unwrap();

            let mut linker = wasmi::Linker::<wasmi_wasi::WasiCtx>::new(&engine);
            wasmi_wasi::add_to_linker(&mut linker, |ctx| ctx)?;

            let instance = linker.instantiate(&mut store, &module)?.start(&mut store)?;

            let run_result = if let Ok(func) = instance.get_typed_func::<(), ()>(&store, "_start") {
                func.call(&mut store, ())
            } else if let Ok(func) = instance.get_typed_func::<(), ()>(&store, "main") {
                func.call(&mut store, ())
            } else {
                let mut executed = false;
                let mut last_res = Ok(());
                for export in module.exports() {
                    if !matches!(export.ty(), wasmi::ExternType::Func(_)) {
                        continue;
                    }
                    if let Ok(func) = instance.get_typed_func::<(), ()>(&store, export.name()) {
                        last_res = func.call(&mut store, ());
                        executed = true;
                        break;
                    }
                }
                if !executed {
                    anyhow::bail!("No executable parameterless function found in WASM module");
                }
                last_res
            };

            drop(store);
            let stdout_bytes = stdout_pipe.try_into_inner()
                .map(|p| p.into_inner())
                .unwrap_or_default();
            let stderr_bytes = stderr_pipe.try_into_inner()
                .map(|p| p.into_inner())
                .unwrap_or_default();

            let stdout_str = String::from_utf8_lossy(&stdout_bytes).into_owned();
            let stderr_str = String::from_utf8_lossy(&stderr_bytes).into_owned();

            match run_result {
                Ok(_) => {
                    Ok(CommandResult {
                        exit_code: 0,
                        stdout: stdout_str,
                        stderr: stderr_str,
                    })
                }
                Err(e) => {
                    let err_str = e.to_string();
                    let exit_code = if err_str.contains("out of fuel") {
                        -2
                    } else {
                        -1
                    };
                    Ok(CommandResult {
                        exit_code,
                        stdout: stdout_str,
                        stderr: format!("WASM Execution Error: {}\n{}", err_str, stderr_str),
                    })
                }
            }
        }).await?;

        result
    }


    async fn read_file(&self, path: &str) -> Result<String> {
        let real_path = self.translate_path(path)?;
        let content = tokio::fs::read_to_string(real_path).await?;
        Ok(content)
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<()> {
        let real_path = self.translate_path(path)?;
        if let Some(parent) = real_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(real_path, content).await?;
        Ok(())
    }

    async fn list_dir(&self, path: &str, max_depth: usize) -> Result<String> {
        let real_path = self.translate_path(path)?;
        let mut out = String::new();
        list_dir_recursive(&real_path, 0, max_depth, &mut out)?;
        Ok(out)
    }
}

// ──────────────────────────────────────────────
// Shared Dynamic Sandbox Dispatcher
// ──────────────────────────────────────────────

#[derive(Clone)]
pub struct SharedSandbox {
    mode: Arc<std::sync::RwLock<SandboxMode>>,
}

impl SharedSandbox {
    pub fn new(mode: SandboxMode) -> Self {
        Self {
            mode: Arc::new(std::sync::RwLock::new(mode)),
        }
    }

    pub fn set_mode(&self, mode: SandboxMode) {
        if let Ok(mut w) = self.mode.write() {
            *w = mode;
        }
    }

    pub fn get_mode(&self) -> SandboxMode {
        self.mode.read().map(|r| *r).unwrap_or(SandboxMode::Local)
    }
}

#[async_trait]
impl SandboxBackend for SharedSandbox {
    async fn execute_command(&self, cmd: &str) -> Result<CommandResult> {
        let mode = self.get_mode();
        match mode {
            SandboxMode::Local => LocalSandbox.execute_command(cmd).await,
            SandboxMode::Docker => DockerSandbox::new().execute_command(cmd).await,
            SandboxMode::Wasm => WasmSandbox::new().execute_command(cmd).await,
            SandboxMode::SSH => anyhow::bail!("SSH sandbox mode is not implemented yet"),
        }
    }

    async fn read_file(&self, path: &str) -> Result<String> {
        let mode = self.get_mode();
        match mode {
            SandboxMode::Local => LocalSandbox.read_file(path).await,
            SandboxMode::Docker => DockerSandbox::new().read_file(path).await,
            SandboxMode::Wasm => WasmSandbox::new().read_file(path).await,
            SandboxMode::SSH => anyhow::bail!("SSH sandbox mode is not implemented yet"),
        }
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<()> {
        let mode = self.get_mode();
        match mode {
            SandboxMode::Local => LocalSandbox.write_file(path, content).await,
            SandboxMode::Docker => DockerSandbox::new().write_file(path, content).await,
            SandboxMode::Wasm => WasmSandbox::new().write_file(path, content).await,
            SandboxMode::SSH => anyhow::bail!("SSH sandbox mode is not implemented yet"),
        }
    }

    async fn list_dir(&self, path: &str, max_depth: usize) -> Result<String> {
        let mode = self.get_mode();
        match mode {
            SandboxMode::Local => LocalSandbox.list_dir(path, max_depth).await,
            SandboxMode::Docker => DockerSandbox::new().list_dir(path, max_depth).await,
            SandboxMode::Wasm => WasmSandbox::new().list_dir(path, max_depth).await,
            SandboxMode::SSH => anyhow::bail!("SSH sandbox mode is not implemented yet"),
        }
    }
}

pub fn resolve_and_validate_path(base_dir: &Path, user_path: &str) -> Result<PathBuf> {
    let raw_path = base_dir.join(user_path);
    // Normalize path by resolving components
    let mut normalized = PathBuf::new();
    for component in raw_path.components() {
        match component {
            std::path::Component::Prefix(..) => {
                normalized.push(component);
            }
            std::path::Component::RootDir => {
                normalized.push(component);
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    anyhow::bail!("Path traversal attempt detected: escaping root");
                }
            }
            std::path::Component::Normal(c) => {
                normalized.push(c);
            }
        }
    }

    // Canonicalize base_dir so symlinks are correctly evaluated
    let canonical_base = base_dir.canonicalize().map_err(|e| {
        anyhow::anyhow!(
            "Failed to canonicalize base directory '{}': {}",
            base_dir.display(),
            e
        )
    })?;

    #[cfg(target_os = "windows")]
    let canonical_base = {
        let base_str = canonical_base.to_string_lossy();
        if let Some(stripped) = base_str.strip_prefix(r#"\\?\"#) {
            PathBuf::from(stripped)
        } else {
            canonical_base
        }
    };

    // Canonicalize target (handling non-existing suffix components)
    let canonical_target = if normalized.exists() {
        normalized.canonicalize()?
    } else {
        // Resolve closest existing ancestor
        let mut ancestor = normalized.as_path();
        while let Some(parent) = ancestor.parent() {
            if parent.exists() {
                ancestor = parent;
                break;
            }
            ancestor = parent;
        }
        if ancestor.exists() {
            let canon_ancestor = ancestor.canonicalize()?;
            let suffix = normalized.strip_prefix(ancestor).unwrap_or(Path::new(""));
            canon_ancestor.join(suffix)
        } else {
            normalized.clone()
        }
    };

    #[cfg(target_os = "windows")]
    let canonical_target = {
        let target_str = canonical_target.to_string_lossy();
        if let Some(stripped) = target_str.strip_prefix(r#"\\?\"#) {
            PathBuf::from(stripped)
        } else {
            canonical_target
        }
    };

    if !canonical_target.starts_with(&canonical_base) {
        anyhow::bail!(
            "Access denied: path '{}' is outside the workspace root '{}'",
            user_path,
            canonical_base.display()
        );
    }

    Ok(canonical_target)
}

// Helper directory recursion
fn list_dir_recursive(dir: &Path, depth: usize, max_depth: usize, out: &mut String) -> Result<()> {
    let indent = "  ".repeat(depth);
    let entries = std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("Cannot read directory '{}': {}", dir.display(), e))?;
    let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            out.push_str(&format!("{}📁 {}/\n", indent, name));
            if depth < max_depth {
                let _ = list_dir_recursive(&path, depth + 1, max_depth, out);
            }
        } else {
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push_str(&format!("{}📄 {} ({}B)\n", indent, name, size));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_local_sandbox_command() {
        let sandbox = LocalSandbox;
        let res = sandbox.execute_command("echo 'hello world'").await.unwrap();
        assert_eq!(res.exit_code, 0);
        assert!(res.stdout.contains("hello world"));
    }

    #[tokio::test]
    async fn test_local_sandbox_file_ops() {
        let sandbox = LocalSandbox;
        let current_dir = std::env::current_dir().unwrap();
        let dir = tempfile::Builder::new().tempdir_in(&current_dir).unwrap();
        let file_path = dir.path().join("test.txt");
        let path_str = file_path.to_str().unwrap();

        sandbox
            .write_file(path_str, "sandbox test content")
            .await
            .unwrap();
        let content = sandbox.read_file(path_str).await.unwrap();
        assert_eq!(content, "sandbox test content");

        let listing = sandbox
            .list_dir(dir.path().to_str().unwrap(), 1)
            .await
            .unwrap();
        assert!(listing.contains("test.txt"));
    }

    #[test]
    fn test_docker_sandbox_path_translation() {
        let sandbox = DockerSandbox::new();
        let current_dir = std::env::current_dir().unwrap();

        // Relative path
        let rel = sandbox.translate_path("src/lib.rs").unwrap();
        assert_eq!(rel, current_dir.join("src/lib.rs"));

        // Absolute path within current workspace
        let abs_in = current_dir.join("src/lib.rs");
        let abs_in_str = abs_in.to_str().unwrap();
        let res_in = sandbox.translate_path(abs_in_str).unwrap();
        assert_eq!(res_in, abs_in);

        // Absolute path inside container workspace
        let abs_container = "/workspace/src/lib.rs";
        let res_container = sandbox.translate_path(abs_container).unwrap();
        assert_eq!(res_container, current_dir.join("src/lib.rs"));

        // Absolute path outside current workspace (should be jailed)
        let abs_out = "/etc/passwd";
        let res_out = sandbox.translate_path(abs_out).unwrap();
        assert_eq!(res_out, current_dir.join("etc/passwd"));
    }

    #[tokio::test]
    async fn test_wasm_sandbox_execution() {
        let sandbox = WasmSandbox::new();
        let _ = std::fs::create_dir_all(&sandbox.jail_dir);

        let wasm_bytes = [
            0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, 0x01, 0x04, 0x01, 0x60, 0x00, 0x00,
            0x03, 0x02, 0x01, 0x00, 0x07, 0x08, 0x01, 0x04, 0x6d, 0x61, 0x69, 0x6e, 0x00, 0x00,
            0x0a, 0x04, 0x01, 0x02, 0x00, 0x0b,
        ];

        let wasm_file_path = sandbox.jail_dir.join("test_module.wasm");
        tokio::fs::write(&wasm_file_path, wasm_bytes).await.unwrap();

        let res = sandbox.execute_command("test_module.wasm").await.unwrap();
        assert_eq!(res.exit_code, 0);

        let _ = tokio::fs::remove_file(wasm_file_path).await;
    }
}
