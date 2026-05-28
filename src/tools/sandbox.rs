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
        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .await?;
        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        Ok(CommandResult { exit_code, stdout, stderr })
    }

    async fn read_file(&self, path: &str) -> Result<String> {
        let content = tokio::fs::read_to_string(path).await
            .map_err(|e| anyhow::anyhow!("Cannot read file '{}': {}", path, e))?;
        Ok(content)
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<()> {
        if let Some(parent) = Path::new(path).parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, content).await?;
        Ok(())
    }

    async fn list_dir(&self, path: &str, max_depth: usize) -> Result<String> {
        let mut out = String::new();
        list_dir_recursive(Path::new(path), 0, max_depth, &mut out)?;
        Ok(out)
    }
}

// ──────────────────────────────────────────────
// Docker Container Sandbox
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

    fn translate_path(&self, path: &str) -> Result<PathBuf> {
        let current_dir = std::env::current_dir()?;
        let target = Path::new(path);
        if target.is_absolute() {
            if target.starts_with(&current_dir) {
                Ok(target.to_path_buf())
            } else {
                let relative = target.strip_prefix("/").unwrap_or(target);
                Ok(current_dir.join(relative))
            }
        } else {
            Ok(current_dir.join(target))
        }
    }
}

#[async_trait]
impl SandboxBackend for DockerSandbox {
    async fn execute_command(&self, cmd: &str) -> Result<CommandResult> {
        let current_dir = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
        let current_dir_str = current_dir.to_string_lossy().into_owned();

        let uid = std::process::Command::new("id").arg("-u").output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "1000".to_string());
        let gid = std::process::Command::new("id").arg("-g").output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "1000".to_string());

        let mut docker_cmd = Command::new("docker");
        docker_cmd.arg("run").arg("--rm");
        
        docker_cmd.arg("-v").arg(format!("{}:{}", current_dir_str, self.container_workspace));

        if let Some(base_dirs) = directories::BaseDirs::new() {
            let cargo_home = base_dirs.home_dir().join(".cargo");
            let registry = cargo_home.join("registry");
            let git = cargo_home.join("git");
            if registry.exists() {
                docker_cmd.arg("-v").arg(format!("{}:/usr/local/cargo/registry", registry.to_string_lossy()));
            }
            if git.exists() {
                docker_cmd.arg("-v").arg(format!("{}:/usr/local/cargo/git", git.to_string_lossy()));
            }
        }

        docker_cmd
            .arg("-w")
            .arg(&self.container_workspace)
            .arg("--user")
            .arg(format!("{}:{}", uid, gid))
            .arg("rust:latest")
            .arg("sh")
            .arg("-c")
            .arg(cmd);

        let output = docker_cmd.output().await?;
        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        Ok(CommandResult { exit_code, stdout, stderr })
    }

    async fn read_file(&self, path: &str) -> Result<String> {
        let real_path = self.translate_path(path)?;
        let content = tokio::fs::read_to_string(real_path).await
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
            SandboxMode::SSH => anyhow::bail!("SSH sandbox mode is not implemented yet"),
        }
    }

    async fn read_file(&self, path: &str) -> Result<String> {
        let mode = self.get_mode();
        match mode {
            SandboxMode::Local => LocalSandbox.read_file(path).await,
            SandboxMode::Docker => DockerSandbox::new().read_file(path).await,
            SandboxMode::SSH => anyhow::bail!("SSH sandbox mode is not implemented yet"),
        }
    }

    async fn write_file(&self, path: &str, content: &str) -> Result<()> {
        let mode = self.get_mode();
        match mode {
            SandboxMode::Local => LocalSandbox.write_file(path, content).await,
            SandboxMode::Docker => DockerSandbox::new().write_file(path, content).await,
            SandboxMode::SSH => anyhow::bail!("SSH sandbox mode is not implemented yet"),
        }
    }

    async fn list_dir(&self, path: &str, max_depth: usize) -> Result<String> {
        let mode = self.get_mode();
        match mode {
            SandboxMode::Local => LocalSandbox.list_dir(path, max_depth).await,
            SandboxMode::Docker => DockerSandbox::new().list_dir(path, max_depth).await,
            SandboxMode::SSH => anyhow::bail!("SSH sandbox mode is not implemented yet"),
        }
    }
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
    use tempfile::tempdir;

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
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        let path_str = file_path.to_str().unwrap();

        sandbox.write_file(path_str, "sandbox test content").await.unwrap();
        let content = sandbox.read_file(path_str).await.unwrap();
        assert_eq!(content, "sandbox test content");

        let listing = sandbox.list_dir(dir.path().to_str().unwrap(), 1).await.unwrap();
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

        // Absolute path outside current workspace (should be jailed)
        let abs_out = "/etc/passwd";
        let res_out = sandbox.translate_path(abs_out).unwrap();
        assert_eq!(res_out, current_dir.join("etc/passwd"));
    }
}
