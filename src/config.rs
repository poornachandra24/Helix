//! # System Configuration Loader & Setup Wizard
//!
//! This module handles provider API templates (Ollama, OpenAI, Gemini, etc.),
//! global execution settings (sandbox modes, token headroom), and the CLI configuration wizard.

use anyhow::{Context, Result};

use dialoguer::{Confirm, Input, Password, Select, theme::ColorfulTheme};
use directories::ProjectDirs;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::path::PathBuf;

use crate::tools::sandbox::SandboxMode;

/// Determines which HTTP path and payload schema the adapter uses for a provider.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApiFormat {
    /// Standard OpenAI-compatible: POST /v1/chat/completions
    #[default]
    OpenAiCompatible,
    /// Ollama native format: POST /api/chat
    OllamaNative,
}

impl fmt::Display for ApiFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiFormat::OpenAiCompatible => write!(f, "OpenAI-compatible"),
            ApiFormat::OllamaNative => write!(f, "Ollama native"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Provider {
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_format: ApiFormat,
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let key_hint = match &self.api_key {
            Some(k) if !k.is_empty() => format!(" [key: {}***]", &k[..k.len().min(4)]),
            _ => " [no key]".to_string(),
        };
        write!(
            f,
            "{} → {} ({}){}",
            self.name, self.base_url, self.api_format, key_hint
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub providers: Vec<Provider>,
    pub active_provider: String,
    pub active_model: String,
    /// Override the auto-detected context window (in tokens).
    pub context_window_override: Option<usize>,
    /// Tokens reserved for the model's generated response. Default: 2048.
    pub response_headroom: Option<usize>,
    /// Sandbox mode for tool execution.
    #[serde(default)]
    pub sandbox_mode: SandboxMode,
    /// Optional user-defined thinking level for reasoning models.
    #[serde(default)]
    pub thinking_level: Option<String>,
    /// Enable OpenTelemetry tracing export to Logfire. Default: false.
    #[serde(default)]
    pub enable_telemetry: bool,
    /// Saved Logfire API write token. Reused across sessions until edited.
    #[serde(default)]
    pub logfire_token: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            providers: vec![],
            active_provider: "auto".to_string(),
            active_model: "gpt-4o".to_string(),
            context_window_override: None,
            response_headroom: Some(2048),
            sandbox_mode: SandboxMode::Local,
            thinking_level: None,
            enable_telemetry: false,
            logfire_token: None,
        }
    }
}

impl AppConfig {
    #[allow(dead_code)]
    pub fn get_active_provider(&self) -> Result<Provider> {
        self.providers
            .iter()
            .find(|p| p.name == self.active_provider)
            .cloned()
            .context("Active provider not found in config")
    }

    pub async fn resolve_best_provider(&self, client: &Client) -> Provider {
        // 1. If active_provider is not "auto", try to get it.
        if self.active_provider != "auto"
            && let Some(p) = self
                .providers
                .iter()
                .find(|p| p.name == self.active_provider)
        {
            tracing::info!("Probing active provider '{}' ({})", p.name, p.base_url);
            if check_provider_health(client, p).await {
                return p.clone();
            } else {
                tracing::warn!(
                    "Active provider '{}' is offline. Proceeding anyway...",
                    p.name
                );
                return p.clone();
            }
        }

        // 2. We are in "auto" mode. Probe all configured providers to find the best online candidate.
        let mut candidates = Vec::new();
        for p in &self.providers {
            tracing::info!("Probing candidate '{}' ({})", p.name, p.base_url);
            if check_provider_health(client, p).await {
                candidates.push(p.clone());
            }
        }

        if candidates.is_empty() {
            // Absolutely everything is offline, fall back to active_provider config if possible
            if let Some(p) = self
                .providers
                .iter()
                .find(|p| p.name == self.active_provider)
            {
                return p.clone();
            }
            if !self.providers.is_empty() {
                return self.providers[0].clone();
            }
            // Absolute fallback template
            return Provider {
                name: "Ollama (Local)".to_string(),
                base_url: "http://localhost:11434".to_string(),
                api_key: None,
                api_format: ApiFormat::OllamaNative,
            };
        }

        // 3. Sort candidates: local (localhost/127.0.0.1) first, local LAN next, cloud last
        candidates.sort_by_key(|p| {
            let url = p.base_url.to_lowercase();
            if url.contains("localhost") || url.contains("127.0.0.1") || url.contains("::1") {
                0
            } else if url.contains("192.168.") || url.contains("10.") || url.contains("172.16.") {
                1
            } else {
                2
            }
        });

        let chosen = candidates[0].clone();
        tracing::info!("Routed requests to '{}' ({})", chosen.name, chosen.base_url);
        chosen
    }

    /// Response headroom with fallback default.
    pub fn effective_headroom(&self) -> usize {
        self.response_headroom.unwrap_or(2048)
    }

    pub fn save(&self) -> Result<()> {
        let config_dir = get_config_dir()?;
        fs::create_dir_all(&config_dir)?;
        let config_file = config_dir.join("config.toml");
        let toml_str = toml::to_string(self)?;
        fs::write(&config_file, toml_str)?;
        Ok(())
    }
}

pub async fn check_provider_health(client: &Client, provider: &Provider) -> bool {
    let base = provider.base_url.trim_end_matches('/');
    let url = match provider.api_format {
        ApiFormat::OpenAiCompatible => {
            if base.ends_with("/models") {
                base.to_string()
            } else {
                format!("{}/models", base)
            }
        }
        ApiFormat::OllamaNative => {
            if base.ends_with("/api/tags") {
                base.to_string()
            } else if base.ends_with("/api") {
                format!("{}/tags", base)
            } else {
                format!("{}/api/tags", base)
            }
        }
    };
    let mut req = client
        .get(&url)
        .timeout(std::time::Duration::from_millis(5000));
    if let Some(key) = &provider.api_key {
        req = req.bearer_auth(key);
    }
    match req.send().await {
        Ok(resp) => {
            resp.status().is_success() || resp.status() == reqwest::StatusCode::UNAUTHORIZED
        }
        Err(_) => false,
    }
}

pub fn get_config_dir() -> Result<PathBuf> {
    if let Ok(val) = std::env::var("HELIX_HOME") {
        let val = val.trim();
        if !val.is_empty() {
            return Ok(PathBuf::from(val).join("config"));
        }
    }
    if let Some(proj_dirs) = ProjectDirs::from("com", "helix", "helix") {
        let dir = proj_dirs.config_dir().to_path_buf();
        if dir.exists() || fs::create_dir_all(&dir).is_ok() {
            return Ok(dir);
        }
    }
    let temp = std::env::temp_dir().join("helix_config");
    let _ = fs::create_dir_all(&temp);
    Ok(temp)
}

pub fn get_state_dir() -> Result<PathBuf> {
    if let Ok(val) = std::env::var("HELIX_HOME") {
        let val = val.trim();
        if !val.is_empty() {
            return Ok(PathBuf::from(val).join("state"));
        }
    }
    if let Some(proj_dirs) = ProjectDirs::from("com", "helix", "helix") {
        let dir = proj_dirs
            .state_dir()
            .unwrap_or_else(|| proj_dirs.data_dir())
            .to_path_buf();
        if dir.exists() || fs::create_dir_all(&dir).is_ok() {
            return Ok(dir);
        }
    }
    let temp = std::env::temp_dir().join("helix_state");
    let _ = fs::create_dir_all(&temp);
    Ok(temp)
}

pub fn get_data_dir() -> Result<PathBuf> {
    if let Ok(val) = std::env::var("HELIX_HOME") {
        let val = val.trim();
        if !val.is_empty() {
            return Ok(PathBuf::from(val).join("data"));
        }
    }
    if let Some(proj_dirs) = ProjectDirs::from("com", "helix", "helix") {
        let dir = proj_dirs.data_dir().to_path_buf();
        if dir.exists() || fs::create_dir_all(&dir).is_ok() {
            return Ok(dir);
        }
    }
    let temp = std::env::temp_dir().join("helix_data");
    let _ = fs::create_dir_all(&temp);
    Ok(temp)
}

/// Normalizes any Ollama Cloud providers to the official OpenAI-compatible endpoint (https://ollama.com/v1).
pub fn normalize_ollama_cloud_providers(config: &mut AppConfig) -> bool {
    let mut modified = false;
    for p in &mut config.providers {
        let is_ollama_cloud =
            p.name.eq_ignore_ascii_case("Ollama Cloud") || p.base_url.contains("ollama.com");
        if is_ollama_cloud {
            if p.api_format != ApiFormat::OpenAiCompatible {
                p.api_format = ApiFormat::OpenAiCompatible;
                modified = true;
            }
            let base = p.base_url.trim_end_matches('/');
            if base == "https://ollama.com"
                || base == "http://ollama.com"
                || base == "https://ollama.com/api"
                || base == "http://ollama.com/api"
                || base.ends_with("/api")
                || !base.ends_with("/v1")
            {
                p.base_url = "https://ollama.com/v1".to_string();
                modified = true;
            }
            if p.api_key.is_none()
                && let Ok(key) = std::env::var("OLLAMA_API_KEY")
                && !key.trim().is_empty()
            {
                p.api_key = Some(key.trim().to_string());
                modified = true;
            }
        }
    }
    modified
}

pub fn load_config() -> Result<AppConfig> {
    let config_file = get_config_dir()?.join("config.toml");
    if config_file.exists() {
        let content = fs::read_to_string(&config_file)?;
        if let Ok(mut config) = toml::from_str::<AppConfig>(&content) {
            if normalize_ollama_cloud_providers(&mut config) {
                let _ = config.save();
            }
            return Ok(config);
        }
    }

    use std::io::IsTerminal;
    let is_terminal = std::io::stdin().is_terminal();

    // Check for environment-based configuration overrides (vital for Harbor/CI/headless benchmarks)
    let env_openai = std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let env_openrouter = std::env::var("OPENROUTER_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let env_anthropic = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let env_ollama_cloud = std::env::var("OLLAMA_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let env_model = std::env::var("HELIX_MODEL")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let env_base_url = std::env::var("HELIX_API_BASE")
        .or_else(|_| std::env::var("OPENAI_BASE_URL"))
        .ok()
        .filter(|s| !s.trim().is_empty());
    let is_non_interactive = !is_terminal
        || std::env::var("NON_INTERACTIVE").is_ok()
        || std::env::var("HELIX_YES").is_ok()
        || std::env::var("CI").is_ok();

    if is_non_interactive
        || env_openai.is_some()
        || env_openrouter.is_some()
        || env_anthropic.is_some()
        || env_ollama_cloud.is_some()
        || env_base_url.is_some()
    {
        let (provider_name, base_url, api_key, default_model, format) =
            if let Some(base) = env_base_url {
                let key = env_openai
                    .or(env_openrouter)
                    .or(env_anthropic)
                    .or(env_ollama_cloud);
                (
                    "Custom",
                    base,
                    key,
                    env_model.unwrap_or_else(|| "gpt-4o".to_string()),
                    ApiFormat::OpenAiCompatible,
                )
            } else if let Some(key) = env_ollama_cloud {
                (
                    "Ollama Cloud",
                    "https://ollama.com/v1".to_string(),
                    Some(key),
                    env_model.unwrap_or_else(|| "gemma4:31b".to_string()),
                    ApiFormat::OpenAiCompatible,
                )
            } else if let Some(key) = env_openrouter {
                (
                    "OpenRouter",
                    "https://openrouter.ai/api/v1".to_string(),
                    Some(key),
                    env_model.unwrap_or_else(|| "anthropic/claude-3.7-sonnet".to_string()),
                    ApiFormat::OpenAiCompatible,
                )
            } else if let Some(key) = env_openai {
                (
                    "OpenAI",
                    "https://api.openai.com/v1".to_string(),
                    Some(key),
                    env_model.unwrap_or_else(|| "gpt-4o".to_string()),
                    ApiFormat::OpenAiCompatible,
                )
            } else if let Some(key) = env_anthropic {
                (
                    "OpenRouter",
                    "https://openrouter.ai/api/v1".to_string(),
                    Some(key),
                    env_model.unwrap_or_else(|| "anthropic/claude-3.7-sonnet".to_string()),
                    ApiFormat::OpenAiCompatible,
                )
            } else {
                (
                    "Ollama (Local)",
                    "http://localhost:11434".to_string(),
                    None,
                    env_model.unwrap_or_else(|| "llama3.2".to_string()),
                    ApiFormat::OllamaNative,
                )
            };

        let auto_config = AppConfig {
            providers: vec![Provider {
                name: provider_name.to_string(),
                base_url,
                api_key,
                api_format: format,
            }],
            active_provider: provider_name.to_string(),
            active_model: default_model,
            context_window_override: None,
            response_headroom: Some(2048),
            sandbox_mode: SandboxMode::Local,
            thinking_level: None,
            enable_telemetry: false,
            logfire_token: None,
        };
        return Ok(auto_config);
    }

    use owo_colors::OwoColorize;
    println!();
    println!("{}", "   ██╗  ██╗███████╗██╗     ██╗██╗  ██╗".cyan().bold());
    println!("{}", "   ██║  ██║██╔════╝██║     ██║╚██╗██╔╝".cyan().bold());
    println!("{}", "   ███████║█████╗  ██║     ██║ ╚███╔╝ ".cyan().bold());
    println!("{}", "   ██╔══██║██╔══╝  ██║     ██║ ██╔██╗ ".cyan().bold());
    println!("{}", "   ██║  ██║███████╗███████╗██║██╔╝ ██╗".cyan().bold());
    println!("{}", "   ╚═╝  ╚═╝╚══════╝╚══════╝╚═╝╚═╝  ╚═╝".cyan().bold());
    println!();
    println!("  {}", "⚙️  Helix Setup Wizard".bold().cyan());
    println!("  {}", "─────────────────────────────────────────".dimmed());
    println!("  Let's configure your primary provider and model.");
    println!();

    interactive_setup(None)
}

/// Provider templates: (display name, base_url, api_key_required, default_model, format)
const PROVIDER_TEMPLATES: &[(&str, &str, bool, &str, ApiFormat)] = &[
    (
        "Ollama (Local)",
        "http://localhost:11434",
        false,
        "llama3.2",
        ApiFormat::OllamaNative,
    ),
    (
        "Ollama Cloud",
        "https://ollama.com/v1",
        true,
        "gemma4:31b",
        ApiFormat::OpenAiCompatible,
    ),
    (
        "OpenRouter",
        "https://openrouter.ai/api/v1",
        true,
        "anthropic/claude-3.5-sonnet",
        ApiFormat::OpenAiCompatible,
    ),
    (
        "DeepSeek",
        "https://api.deepseek.com/v1",
        true,
        "deepseek-chat",
        ApiFormat::OpenAiCompatible,
    ),
    (
        "OpenAI",
        "https://api.openai.com/v1",
        true,
        "gpt-4o",
        ApiFormat::OpenAiCompatible,
    ),
    (
        "Groq",
        "https://api.groq.com/openai/v1",
        true,
        "llama3-8b-8192",
        ApiFormat::OpenAiCompatible,
    ),
    (
        "Gemini",
        "https://generativelanguage.googleapis.com/v1beta/openai",
        true,
        "gemini-2.5-flash",
        ApiFormat::OpenAiCompatible,
    ),
    (
        "vLLM (Local)",
        "http://localhost:8000/v1",
        false,
        "meta-llama/Llama-3-8b",
        ApiFormat::OpenAiCompatible,
    ),
    (
        "Anthropic (via LiteLLM proxy)",
        "http://localhost:4000/v1",
        false,
        "claude-3-haiku-20240307",
        ApiFormat::OpenAiCompatible,
    ),
    (
        "Custom (OpenAI-compatible)",
        "",
        false,
        "",
        ApiFormat::OpenAiCompatible,
    ),
    (
        "Custom (Ollama native)",
        "",
        false,
        "",
        ApiFormat::OllamaNative,
    ),
];

fn validate_provider_connectivity(provider: &Provider) {
    let provider = provider.clone();
    let _ = std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(_) => return,
        };
        rt.block_on(async {
            let client = Client::new();
            if !check_provider_health(&client, &provider).await {
                println!();
                println!("  ⚠️  {}", console::style("Connection/Auth Warning:").yellow().bold());
                println!("     Could not establish a successful connection to: {}", provider.base_url);
                if provider.name == "Ollama Cloud" {
                    println!("     For Ollama Cloud, please verify you have:");
                    println!("       1. Created an account on https://ollama.com");
                    println!("       2. Generated an API key from your profile settings");
                    println!("       3. Entered this key during the setup prompt or set it in your config.toml");
                } else if provider.name == "Ollama (Local)" {
                    println!("     Please verify that your local Ollama instance is running.");
                    println!("     If you are running Helix inside a Docker container, 'localhost' points to the container.");
                    println!("     To connect to your host machine's Ollama, use:");
                    println!("       - Linux: http://172.17.0.1:11434");
                    println!("       - macOS/Windows: http://host.docker.internal:11434");
                }
                println!();
            }
        });
    }).join();
}

pub fn interactive_setup(existing: Option<AppConfig>) -> Result<AppConfig> {
    let mut config = existing.unwrap_or_else(|| AppConfig {
        providers: vec![],
        active_provider: String::new(),
        active_model: String::new(),
        context_window_override: None,
        response_headroom: None,
        sandbox_mode: SandboxMode::Local,
        thinking_level: None,
        enable_telemetry: false,
        logfire_token: None,
    });

    loop {
        let main_menu = vec![
            format!(
                "Edit active provider: {} ({})",
                config.active_provider, config.active_model
            ),
            "Add / Configure a new provider from template".to_string(),
            format!(
                "Change sandbox execution mode (currently: {:?})",
                config.sandbox_mode
            ),
            format!(
                "Change thinking level (currently: {})",
                config.thinking_level.as_deref().unwrap_or("not set")
            ),
            format!(
                "Change context window override (currently: {})",
                config
                    .context_window_override
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "none".to_string())
            ),
            format!(
                "Change response headroom (currently: {})",
                config
                    .response_headroom
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "none".to_string())
            ),
            format!(
                "Configure Logfire telemetry (currently: {}, token: {})",
                if config.enable_telemetry {
                    "enabled"
                } else {
                    "disabled"
                },
                if config.logfire_token.is_some() {
                    "saved"
                } else {
                    "none"
                }
            ),
            "Exit configuration".to_string(),
        ];

        let choice = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Helix CLI Configuration Menu")
            .default(0)
            .items(&main_menu)
            .interact()?;

        match choice {
            0 => {
                let active_p_name = config.active_provider.clone();
                let existing_provider = config
                    .providers
                    .iter()
                    .find(|p| p.name == active_p_name)
                    .cloned();
                if let Some(mut ep) = existing_provider {
                    let base_url: String = Input::with_theme(&ColorfulTheme::default())
                        .with_prompt("Base URL (type 'back' to cancel)")
                        .default(ep.base_url.clone())
                        .interact_text()?;
                    if base_url.trim().eq_ignore_ascii_case("back") {
                        continue;
                    }

                    let api_key = if let Some(ref current_key) = ep.api_key {
                        let confirm = Confirm::with_theme(&ColorfulTheme::default())
                            .with_prompt("Keep existing API Key?")
                            .default(true)
                            .interact()?;
                        if confirm {
                            Some(current_key.clone())
                        } else {
                            let key: String = Password::with_theme(&ColorfulTheme::default())
                                .with_prompt("New API Key (Enter to skip)")
                                .allow_empty_password(true)
                                .interact()?;
                            if key.trim().is_empty() {
                                None
                            } else {
                                Some(key)
                            }
                        }
                    } else {
                        let key: String = Password::with_theme(&ColorfulTheme::default())
                            .with_prompt("API Key (Enter to skip)")
                            .allow_empty_password(true)
                            .interact()?;
                        if key.trim().is_empty() {
                            None
                        } else {
                            Some(key)
                        }
                    };

                    let model_name: String = Input::with_theme(&ColorfulTheme::default())
                        .with_prompt("Model Name (type 'back' to cancel)")
                        .default(config.active_model.clone())
                        .interact_text()?;
                    if model_name.trim().eq_ignore_ascii_case("back") {
                        continue;
                    }

                    ep.base_url = base_url;
                    ep.api_key = api_key;
                    config.active_model = model_name;

                    config.providers.retain(|p| p.name != ep.name);
                    config.providers.push(ep.clone());
                    config.save()?;
                    println!("\nConfiguration updated and saved!");
                    validate_provider_connectivity(&ep);
                } else {
                    println!("No active provider configured yet.");
                }
            }
            1 => {
                let mut selections: Vec<String> =
                    PROVIDER_TEMPLATES.iter().map(|p| p.0.to_string()).collect();
                selections.push("< Back to main menu".to_string());

                let selection = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt("Choose a Provider Template")
                    .default(0)
                    .items(&selections)
                    .interact()?;

                if selection == PROVIDER_TEMPLATES.len() {
                    continue;
                }

                let (name, default_url, needs_key, default_model, format) =
                    &PROVIDER_TEMPLATES[selection];
                let name = name.to_string();

                let matching_provider = config.providers.iter().find(|p| p.name == name);
                let prepop_url = matching_provider
                    .map(|p| p.base_url.clone())
                    .unwrap_or_else(|| default_url.to_string());
                let prepop_model = matching_provider
                    .map(|_| config.active_model.clone())
                    .unwrap_or_else(|| default_model.to_string());

                let base_url: String = Input::with_theme(&ColorfulTheme::default())
                    .with_prompt("Base URL (type 'back' to cancel)")
                    .default(prepop_url)
                    .interact_text()?;
                if base_url.trim().eq_ignore_ascii_case("back") {
                    continue;
                }

                let env_var_name = match name.as_str() {
                    "OpenAI" => Some("OPENAI_API_KEY"),
                    "OpenRouter" => Some("OPENROUTER_API_KEY"),
                    "DeepSeek" => Some("DEEPSEEK_API_KEY"),
                    "Groq" => Some("GROQ_API_KEY"),
                    "Gemini" => Some("GEMINI_API_KEY"),
                    "Ollama Cloud" => Some("OLLAMA_API_KEY"),
                    _ => None,
                };

                let api_key = if *needs_key {
                    let mut key_val = None;
                    if let Some(env_var) = env_var_name {
                        let env_val = std::env::var(env_var).ok().filter(|v| !v.trim().is_empty());
                        if let Some(val) = env_val {
                            let confirm = Confirm::with_theme(&ColorfulTheme::default())
                                .with_prompt(format!(
                                    "Detected API key in env ({}). Use it?",
                                    env_var
                                ))
                                .default(true)
                                .interact()?;
                            if confirm {
                                key_val = Some(val);
                            }
                        }
                    }

                    if key_val.is_none() {
                        if let Some(k) = matching_provider.and_then(|p| p.api_key.as_ref()) {
                            let confirm = Confirm::with_theme(&ColorfulTheme::default())
                                .with_prompt("Reuse existing API key for this provider?")
                                .default(true)
                                .interact()?;
                            if confirm {
                                key_val = Some(k.clone());
                            }
                        }
                        if key_val.is_none() {
                            let key: String = Password::with_theme(&ColorfulTheme::default())
                                .with_prompt(format!("API Key for {}", name))
                                .interact()?;
                            key_val = Some(key);
                        }
                    }
                    key_val
                } else {
                    let key: String = Password::with_theme(&ColorfulTheme::default())
                        .with_prompt(format!("API Key for {} (Enter to skip)", name))
                        .allow_empty_password(true)
                        .interact()?;
                    if key.trim().is_empty() {
                        None
                    } else {
                        Some(key)
                    }
                };

                let model_name: String = Input::with_theme(&ColorfulTheme::default())
                    .with_prompt("Model Name (type 'back' to cancel)")
                    .default(prepop_model)
                    .interact_text()?;
                if model_name.trim().eq_ignore_ascii_case("back") {
                    continue;
                }

                let provider = Provider {
                    name: name.clone(),
                    base_url,
                    api_key,
                    api_format: format.clone(),
                };

                config.providers.retain(|p| p.name != provider.name);
                config.providers.push(provider.clone());
                config.active_provider = provider.name.clone();
                config.active_model = model_name;

                config.save()?;
                println!("\nConfiguration updated and saved!");
                validate_provider_connectivity(&provider);
            }
            2 => {
                let modes = vec![
                    "local",
                    "docker",
                    "wasm",
                    "ssh",
                    "worktree",
                    "< Back to main menu",
                ];
                let default_sel = match config.sandbox_mode {
                    SandboxMode::Local => 0,
                    SandboxMode::Docker => 1,
                    SandboxMode::Wasm => 2,
                    SandboxMode::SSH => 3,
                    SandboxMode::Worktree => 4,
                };
                let selection = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt("Select Sandbox Execution Mode")
                    .default(default_sel)
                    .items(&modes)
                    .interact()?;
                if selection == 5 {
                    continue;
                }
                config.sandbox_mode = match selection {
                    0 => SandboxMode::Local,
                    1 => SandboxMode::Docker,
                    2 => SandboxMode::Wasm,
                    3 => SandboxMode::SSH,
                    _ => SandboxMode::Worktree,
                };
                config.save()?;
                println!("\nSandbox mode updated to {:?}", config.sandbox_mode);
            }
            3 => {
                let levels = vec![
                    "off",
                    "low",
                    "medium",
                    "high",
                    "Custom (enter budget)",
                    "< Back to main menu",
                ];
                let default_sel = match config.thinking_level.as_deref().unwrap_or("off") {
                    "off" | "disabled" => 0,
                    "low" => 1,
                    "medium" => 2,
                    "high" => 3,
                    _ => 4,
                };
                let selection = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt("Select model reasoning effort / thinking budget")
                    .default(default_sel)
                    .items(&levels)
                    .interact()?;
                if selection == 5 {
                    continue;
                }

                let val = match selection {
                    0 => Some("off".to_string()),
                    1 => Some("low".to_string()),
                    2 => Some("medium".to_string()),
                    3 => Some("high".to_string()),
                    _ => {
                        let budget: String = Input::with_theme(&ColorfulTheme::default())
                            .with_prompt("Enter custom token budget (type 'back' to cancel)")
                            .interact_text()?;
                        if budget.trim().eq_ignore_ascii_case("back") {
                            continue;
                        }
                        Some(budget)
                    }
                };
                config.thinking_level = val;
                config.save()?;
                println!("\nThinking level updated to {:?}", config.thinking_level);
            }
            4 => {
                let current = config
                    .context_window_override
                    .map(|c| c.to_string())
                    .unwrap_or_default();
                let val_str: String = Input::with_theme(&ColorfulTheme::default())
                    .with_prompt(
                        "Enter context window override in tokens (Enter to clear, type 'back' to cancel)",
                    )
                    .default(current)
                    .allow_empty(true)
                    .interact_text()?;
                if val_str.trim().eq_ignore_ascii_case("back") {
                    continue;
                }

                let val = if val_str.trim().is_empty() {
                    None
                } else {
                    val_str.trim().parse::<usize>().ok()
                };
                config.context_window_override = val;
                config.save()?;
                println!("\nContext window override updated!");
            }
            5 => {
                let current = config
                    .response_headroom
                    .map(|c| c.to_string())
                    .unwrap_or_default();
                let val_str: String = Input::with_theme(&ColorfulTheme::default())
                    .with_prompt(
                        "Enter response headroom in tokens (Enter to clear, type 'back' to cancel)",
                    )
                    .default(current)
                    .allow_empty(true)
                    .interact_text()?;
                if val_str.trim().eq_ignore_ascii_case("back") {
                    continue;
                }

                let val = if val_str.trim().is_empty() {
                    None
                } else {
                    val_str.trim().parse::<usize>().ok()
                };
                config.response_headroom = val;
                config.save()?;
                println!("\nResponse headroom updated!");
            }
            6 => {
                let tele_menu = vec![
                    format!(
                        "Toggle telemetry (currently: {})",
                        if config.enable_telemetry {
                            "enabled"
                        } else {
                            "disabled"
                        }
                    ),
                    format!(
                        "Set / Update Logfire token (currently: {})",
                        if let Some(ref t) = config.logfire_token {
                            let masked = if t.len() > 8 {
                                format!("{}***{}", &t[..4], &t[t.len() - 4..])
                            } else {
                                "***".to_string()
                            };
                            format!("saved [{}]", masked)
                        } else {
                            "not set".to_string()
                        }
                    ),
                    "< Back to main menu".to_string(),
                ];

                let tele_choice = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt("Logfire Observability & Tracing")
                    .default(0)
                    .items(&tele_menu)
                    .interact()?;

                match tele_choice {
                    0 => {
                        let new_state = !config.enable_telemetry;
                        if new_state && config.logfire_token.is_none() {
                            let token_input: String =
                                Password::with_theme(&ColorfulTheme::default())
                                    .with_prompt("Enter your Logfire write token (empty to cancel)")
                                    .allow_empty_password(true)
                                    .interact()?;
                            let trimmed = token_input.trim();
                            if !trimmed.is_empty() {
                                config.logfire_token = Some(trimmed.to_string());
                                config.enable_telemetry = true;
                                if let Some(mgr) = crate::core::TelemetryManager::global() {
                                    let _ = mgr.enable(trimmed);
                                }
                                config.save()?;
                                println!("\n✔ Logfire token saved. Telemetry is now enabled.");
                            } else {
                                println!("\nTelemetry remains disabled (token is required).");
                            }
                        } else {
                            config.enable_telemetry = new_state;
                            if let Some(mgr) = crate::core::TelemetryManager::global() {
                                if new_state {
                                    if let Some(ref tok) = config.logfire_token {
                                        let _ = mgr.enable(tok);
                                    }
                                } else {
                                    let _ = mgr.disable();
                                }
                            }
                            config.save()?;
                            println!(
                                "\n✔ Telemetry {}",
                                if config.enable_telemetry {
                                    "enabled"
                                } else {
                                    "disabled"
                                }
                            );
                        }
                    }
                    1 => {
                        let token_input: String = Password::with_theme(&ColorfulTheme::default())
                            .with_prompt("Enter new Logfire write token (empty to clear)")
                            .allow_empty_password(true)
                            .interact()?;
                        let trimmed = token_input.trim();
                        if trimmed.is_empty() {
                            config.logfire_token = None;
                            config.enable_telemetry = false;
                            if let Some(mgr) = crate::core::TelemetryManager::global() {
                                let _ = mgr.disable();
                            }
                            config.save()?;
                            println!("\n✔ Logfire token cleared. Telemetry disabled.");
                        } else {
                            config.logfire_token = Some(trimmed.to_string());
                            if config.enable_telemetry
                                && let Some(mgr) = crate::core::TelemetryManager::global()
                            {
                                let _ = mgr.enable(trimmed);
                            }
                            config.save()?;
                            println!("\n✔ Logfire token updated in config.");
                        }
                    }
                    _ => {}
                }
            }
            _ => {
                println!("Exiting configuration menu.");
                break;
            }
        }
    }

    Ok(config)
}

/// Switch to a pre-configured provider by name (no password re-entry).
pub fn switch_provider(
    config: &mut AppConfig,
    provider_name: &str,
    model: Option<&str>,
) -> Result<()> {
    if provider_name.eq_ignore_ascii_case("auto") {
        config.active_provider = "auto".to_string();
        if let Some(m) = model {
            config.active_model = m.to_string();
        }
        config.save()?;
        return Ok(());
    }
    let found = config
        .providers
        .iter()
        .find(|p| p.name.eq_ignore_ascii_case(provider_name));
    match found {
        Some(p) => {
            config.active_provider = p.name.clone();
            if let Some(m) = model {
                config.active_model = m.to_string();
            }
            config.save()?;
            Ok(())
        }
        None => anyhow::bail!(
            "Provider '{}' not found. Available: {}",
            provider_name,
            config
                .providers
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_telemetry_config_defaults() {
        let toml_str = r#"
            active_provider = "Ollama"
            active_model = "llama3.2"
            providers = []
        "#;
        let config: AppConfig = toml::from_str(toml_str).expect("deserialize config");
        assert!(
            !config.enable_telemetry,
            "Telemetry should be false by default"
        );
        assert_eq!(
            config.logfire_token, None,
            "Logfire token should be None by default"
        );
    }

    #[test]
    fn test_telemetry_config_persistence_roundtrip() {
        let config = AppConfig {
            providers: vec![],
            active_provider: "Ollama".to_string(),
            active_model: "llama3.2".to_string(),
            context_window_override: None,
            response_headroom: Some(2048),
            sandbox_mode: SandboxMode::Local,
            thinking_level: None,
            enable_telemetry: true,
            logfire_token: Some("logfire_test_token_12345".to_string()),
        };

        let serialized = toml::to_string(&config).expect("serialize config");
        let deserialized: AppConfig = toml::from_str(&serialized).expect("deserialize config");

        assert!(deserialized.enable_telemetry);
        assert_eq!(
            deserialized.logfire_token.as_deref(),
            Some("logfire_test_token_12345")
        );
    }
}
