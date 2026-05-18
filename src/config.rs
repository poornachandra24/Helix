use anyhow::{Context, Result};
use dialoguer::{theme::ColorfulTheme, Input, Password, Select};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Provider {
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    pub providers: Vec<Provider>,
    pub active_provider: String,
    pub active_model: String,
}

impl AppConfig {
    pub fn get_active_provider(&self) -> Result<Provider> {
        self.providers
            .iter()
            .find(|p| p.name == self.active_provider)
            .cloned()
            .context("Active provider not found in config")
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

pub fn interactive_setup(existing: Option<AppConfig>) -> Result<AppConfig> {
    let templates = vec![
        ("Ollama (Local)", "http://localhost:11434/v1", false, "qwen3.5:2b"),
        ("Groq", "https://api.groq.com/openai/v1", true, "llama3-8b-8192"),
        ("OpenAI", "https://api.openai.com/v1", true, "gpt-4o"),
        ("vLLM (Local)", "http://localhost:8000/v1", false, "meta-llama/Llama-3-8b"),
        ("Gemini", "https://generativelanguage.googleapis.com/v1beta/openai", true, "gemini-1.5-flash"),
        ("Anthropic (via LiteLLM proxy)", "http://localhost:4000/v1", false, "claude-3-haiku-20240307"),
        ("Custom", "", false, ""),
    ];

    let selections: Vec<&str> = templates.iter().map(|p| p.0).collect();
    
    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Choose a Provider Template (They all use the standard OpenAI format!)")
        .default(0)
        .items(&selections)
        .interact()?;

    let selected = templates[selection];
    let name = selected.0.to_string();
    
    let base_url: String = if selected.1.is_empty() {
        Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Enter the Base URL")
            .interact_text()?
    } else {
        Input::with_theme(&ColorfulTheme::default())
            .with_prompt("Base URL")
            .default(selected.1.to_string())
            .interact_text()?
    };

    let api_key = if selected.2 {
        let key: String = Password::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("API Key for {}", name))
            .interact()?;
        Some(key)
    } else {
        let key: String = Password::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("API Key for {} (Press Enter to skip)", name))
            .allow_empty_password(true)
            .interact()?;
        if key.trim().is_empty() { None } else { Some(key) }
    };

    let model_name: String = Input::with_theme(&ColorfulTheme::default())
        .with_prompt("Model Name")
        .default(selected.3.to_string())
        .interact_text()?;

    let provider = Provider {
        name: name.clone(),
        base_url,
        api_key,
    };

    let mut config = existing.unwrap_or_else(|| AppConfig {
        providers: vec![],
        active_provider: String::new(),
        active_model: String::new(),
    });

    config.providers.retain(|p| p.name != provider.name);
    config.providers.push(provider.clone());
    config.active_provider = provider.name;
    config.active_model = model_name;

    config.save()?;
    println!("\n✅ Configuration updated and saved!");
    Ok(config)
}
