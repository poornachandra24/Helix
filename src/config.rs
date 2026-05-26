use anyhow::{Context, Result};
use dialoguer::{theme::ColorfulTheme, Input, Password, Select};
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
        write!(f, "{} → {} ({}){}", self.name, self.base_url, self.api_format, key_hint)
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
        if self.active_provider != "auto" {
            if let Some(p) = self.providers.iter().find(|p| p.name == self.active_provider) {
                println!("📡 Probing active provider: '{}' ({})", p.name, p.base_url);
                if check_provider_health(client, p).await {
                    return p.clone();
                } else {
                    println!("⚠️  [Auto-Routing] Active provider '{}' ({}) is offline. Failover activated...", p.name, p.base_url);
                }
            }
        }

        // 2. Either active_provider is "auto" or it is offline. Probe all configured providers.
        let mut candidates = Vec::new();
        for p in &self.providers {
            if self.active_provider != "auto" && p.name == self.active_provider {
                continue;
            }
            println!("📡 Probing candidate provider: '{}' ({})", p.name, p.base_url);
            if check_provider_health(client, p).await {
                candidates.push(p.clone());
            }
        }

        if candidates.is_empty() {
            // Absolutely everything is offline, fall back to active_provider config if possible
            if let Some(p) = self.providers.iter().find(|p| p.name == self.active_provider) {
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
        println!("🤖 [Auto-Routing] Routed requests to: '{}' ({})", chosen.name, chosen.base_url);
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
    let url = match provider.api_format {
        ApiFormat::OpenAiCompatible => format!("{}/models", provider.base_url.trim_end_matches('/')),
        ApiFormat::OllamaNative     => format!("{}/api/tags", provider.base_url.trim_end_matches('/')),
    };
    let mut req = client.get(&url).timeout(std::time::Duration::from_millis(800));
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
    let proj_dirs = ProjectDirs::from("com", "harness", "harness-cli")
        .context("Could not determine config directory")?;
    Ok(proj_dirs.config_dir().to_path_buf())
}

pub fn get_state_dir() -> Result<PathBuf> {
    let proj_dirs = ProjectDirs::from("com", "harness", "harness-cli")
        .context("Could not determine state directory")?;
    Ok(proj_dirs.state_dir().unwrap_or_else(|| proj_dirs.data_dir()).to_path_buf())
}

pub fn get_data_dir() -> Result<PathBuf> {
    let proj_dirs = ProjectDirs::from("com", "harness", "harness-cli")
        .context("Could not determine data directory")?;
    Ok(proj_dirs.data_dir().to_path_buf())
}

pub fn load_config() -> Result<AppConfig> {
    let config_file = get_config_dir()?.join("config.toml");
    if config_file.exists() {
        let content = fs::read_to_string(&config_file)?;
        if let Ok(config) = toml::from_str(&content) {
            return Ok(config);
        }
    }
    println!("Welcome to Harness CLI! Let's set up your primary provider.");
    interactive_setup(None)
}

/// Provider templates: (display name, base_url, api_key_required, default_model, format)
const PROVIDER_TEMPLATES: &[(&str, &str, bool, &str, ApiFormat)] = &[
    ("Ollama (Local)",              "http://localhost:11434", false, "qwen3:4b",           ApiFormat::OllamaNative),
    ("Ollama Cloud",                "https://ollama.com",     true,  "gpt-oss:120b-cloud", ApiFormat::OllamaNative),
    ("Groq",                        "https://api.groq.com/openai/v1", true, "llama3-8b-8192", ApiFormat::OpenAiCompatible),
    ("OpenAI",                      "https://api.openai.com/v1", true, "gpt-4o",          ApiFormat::OpenAiCompatible),
    ("vLLM (Local)",                "http://localhost:8000/v1", false, "meta-llama/Llama-3-8b", ApiFormat::OpenAiCompatible),
    ("Gemini",                      "https://generativelanguage.googleapis.com/v1beta/openai", true, "gemini-1.5-flash", ApiFormat::OpenAiCompatible),
    ("Anthropic (via LiteLLM proxy)", "http://localhost:4000/v1", false, "claude-3-haiku-20240307", ApiFormat::OpenAiCompatible),
    ("Custom (OpenAI-compatible)",  "", false, "", ApiFormat::OpenAiCompatible),
    ("Custom (Ollama native)",      "", false, "", ApiFormat::OllamaNative),
];

pub fn interactive_setup(existing: Option<AppConfig>) -> Result<AppConfig> {
    let selections: Vec<&str> = PROVIDER_TEMPLATES.iter().map(|p| p.0).collect();

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Choose a Provider Template")
        .default(0)
        .items(&selections)
        .interact()?;

    let (name, default_url, needs_key, default_model, format) = &PROVIDER_TEMPLATES[selection];
    let name = name.to_string();

    let base_url: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Base URL")
        .default(default_url.to_string())
        .interact_text()?;

    let api_key = if *needs_key {
        let key: String = Password::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("API Key for {}", name))
            .interact()?;
        Some(key)
    } else {
        let key: String = Password::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("API Key for {} (Enter to skip)", name))
            .allow_empty_password(true)
            .interact()?;
        if key.trim().is_empty() { None } else { Some(key) }
    };

    let model_name: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Model Name")
        .default(default_model.to_string())
        .interact_text()?;

    let provider = Provider {
        name: name.clone(),
        base_url,
        api_key,
        api_format: format.clone(),
    };

    let mut config = existing.unwrap_or_else(|| AppConfig {
        providers: vec![],
        active_provider: String::new(),
        active_model: String::new(),
        context_window_override: None,
        response_headroom: None,
        sandbox_mode: SandboxMode::Local,
    });

    config.providers.retain(|p| p.name != provider.name);
    config.providers.push(provider.clone());
    config.active_provider = provider.name;
    config.active_model = model_name;

    config.save()?;
    println!("\n✅ Configuration updated and saved!");
    Ok(config)
}

/// Switch to a pre-configured provider by name (no password re-entry).
pub fn switch_provider(config: &mut AppConfig, provider_name: &str, model: Option<&str>) -> Result<()> {
    if provider_name.eq_ignore_ascii_case("auto") {
        config.active_provider = "auto".to_string();
        if let Some(m) = model {
            config.active_model = m.to_string();
        }
        config.save()?;
        return Ok(());
    }
    let found = config.providers.iter().find(|p| p.name.eq_ignore_ascii_case(provider_name));
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
            config.providers.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
        ),
    }
}
