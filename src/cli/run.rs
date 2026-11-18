#![allow(clippy::collapsible_if)]

use anyhow::Result;
use console::style;
use std::io::{self, Write};
use tokio::sync::mpsc;

use crate::config;
use crate::core::engine;
use crate::core::persistence;
use crate::memory;
use crate::memory::skills;
use crate::tools::sandbox;

use super::helpers::{
    build_context, build_model, build_tool_registry, init_mcp_tools, load_sona_state,
    save_sona_state, set_auto_approve,
};

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub goal: String,
    pub yes: bool,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub no_memory: bool,
    pub no_reflection: bool,
    pub max_iterations: usize,
    pub thinking: Option<String>,
    pub context_window: Option<usize>,
    pub response_headroom: Option<usize>,
    pub output_trajectory: Option<String>,
}

pub async fn run_single(app_config: &config::AppConfig, opts: RunOptions) -> Result<()> {
    let mut effective_config = app_config.clone();

    if opts.yes {
        set_auto_approve(true);
    }

    if let Some(ref m) = opts.model {
        effective_config.active_model = m.clone();
    }

    if let Some(ref p) = opts.provider {
        effective_config.active_provider = p.clone();
    }

    if let Some(ref t) = opts.thinking {
        effective_config.thinking_level = Some(t.clone());
    }

    if let Some(cw) = opts.context_window {
        effective_config.context_window_override = Some(cw);
    }

    if let Some(rh) = opts.response_headroom {
        effective_config.response_headroom = Some(rh);
    }

    if opts.api_key.is_some() || opts.api_base.is_some() {
        if let Some(active) = effective_config
            .providers
            .iter_mut()
            .find(|p| p.name == effective_config.active_provider)
        {
            if let Some(ref key) = opts.api_key {
                active.api_key = Some(key.clone());
            }
            if let Some(ref base) = opts.api_base {
                active.base_url = base.clone();
            }
        } else {
            effective_config.providers.push(config::Provider {
                name: effective_config.active_provider.clone(),
                base_url: opts
                    .api_base
                    .clone()
                    .unwrap_or_else(|| "https://api.openai.com/v1".to_string()),
                api_key: opts.api_key.clone(),
                api_format: config::ApiFormat::OpenAiCompatible,
            });
        }
    }

    let data_dir = config::get_data_dir()?;
    let _skill_reg = skills::SkillRegistry::new(data_dir.join("skills"))?;
    let sandbox = sandbox::SharedSandbox::new(effective_config.sandbox_mode);
    let mut tools = build_tool_registry(
        sandbox.clone(),
        data_dir.join("skills"),
        effective_config.clone(),
    );

    // Initialize MCP tools without automatically registering them into the active tool registry
    let (_mcp_registry, mcp_tools) = init_mcp_tools().await?;

    // Load available skills
    let mut all_skills = std::collections::HashMap::new();
    if let Ok(entries) = std::fs::read_dir(data_dir.join("skills")) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("");
            if path.is_file() && matches!(ext, "txt" | "md") {
                if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        all_skills.insert(name.to_string(), content);
                    }
                }
            }
        }
    }

    let mut all_mcp_map = std::collections::HashMap::new();
    for t in mcp_tools {
        all_mcp_map.insert(
            t.name.clone(),
            std::sync::Arc::new(t) as std::sync::Arc<dyn crate::tools::Tool>,
        );
    }

    let dynamic_state =
        std::sync::Arc::new(std::sync::Mutex::new(crate::tools::DynamicRegistryState {
            all_mcp_tools: all_mcp_map,
            all_skills,
            active_tools: std::collections::HashSet::new(),
            active_skills: std::collections::HashSet::new(),
            changed: false,
        }));

    // Register dynamic loader tools into tools
    tools.register(
        crate::tools::builtins::ListAvailableToolsAndSkillsTool::new(dynamic_state.clone()),
    );
    tools.register(crate::tools::builtins::LoadToolOrSkillTool::new(
        dynamic_state.clone(),
    ));
    tools.register(crate::tools::builtins::UnloadToolOrSkillTool::new(
        dynamic_state.clone(),
    ));

    // Store core tools copy for Engine
    let mut core_tools = std::collections::HashMap::new();
    for (name, tool) in &tools.tools {
        core_tools.insert(name.clone(), tool.clone());
    }

    let disable_memory = opts.no_memory
        || std::env::var("HELIX_NO_MEMORY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    let disable_reflection = opts.no_reflection
        || std::env::var("HELIX_NO_REFLECTION")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    let effective_max_iterations = if opts.max_iterations != 30 {
        opts.max_iterations
    } else if let Ok(val) = std::env::var("HELIX_MAX_ITERATIONS") {
        val.parse::<usize>().unwrap_or(opts.max_iterations)
    } else {
        opts.max_iterations
    };

    let memory_engine = if !disable_memory {
        let memory_dir = data_dir.join("memory");
        memory::HelixMemoryEngine::new(&memory_dir).ok()
    } else {
        None
    };

    let base_system = "You are an autonomous AI software engineering agent with access to tools. \
        Investigate the repository, locate relevant code and tests, use 'str_replace' to make precise, surgical changes, and run tests via 'bash' to verify your solution. \
        When done, provide a clear summary of your fix and conclude without calling further tools.";

    let system_prompt = base_system.to_string();

    let lookup_client = crate::model::registry::build_lookup_client();
    let context = build_context(
        &effective_config,
        base_system,
        &tools.descriptors(),
        &lookup_client,
    )
    .await;
    let model = build_model(&effective_config);
    let session = persistence::Session::new(None)?;

    let mut engine = engine::Engine::new(model, context, tools, session, core_tools, dynamic_state)
        .with_headless(true)
        .with_reflection(!disable_reflection)
        .with_max_iterations(effective_max_iterations);

    if let Some(mem) = memory_engine {
        let sona_config = ruvector_sona::SonaConfig {
            hidden_dim: 384,
            embedding_dim: 384,
            ..Default::default()
        };
        let sona_engine = ruvector_sona::SonaEngine::with_config(sona_config);
        load_sona_state(&data_dir, &sona_engine);
        engine = engine.with_memory(mem).with_sona(sona_engine);
    }

    println!("🚀 Running: {}\n", style(&opts.goal).bold());

    // Streaming for non-interactive run too
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let printer = tokio::spawn(async move {
        while let Some(token) = rx.recv().await {
            if token == "\x02" || token == "\x03" {
                // Ignore spinner control symbols
            } else if token.starts_with("\x1b[S") {
                // Ignore spinner status text
            } else if token.starts_with("\x1b[T") {
                let msg = token.strip_prefix("\x1b[T").unwrap_or("");
                println!("{}", msg);
                io::stdout().flush().ok();
            } else {
                print!("{}", token);
                io::stdout().flush().ok();
            }
        }
    });

    let actual_mode = if effective_config.sandbox_mode == sandbox::SandboxMode::Worktree {
        if sandbox::requires_worktree(&opts.goal) {
            sandbox::SandboxMode::Worktree
        } else {
            sandbox::SandboxMode::Local
        }
    } else {
        effective_config.sandbox_mode
    };
    sandbox.set_mode(actual_mode);

    let run_res = tokio::select! {
        res = engine.run_turn(&system_prompt, &opts.goal, Some(tx)) => {
            printer.await.ok();
            match res {
                Ok(out) => {
                    println!("\n\n{}", style("✔ Done.").green().bold());
                    let _ = sandbox.finish_turn(true).await;
                    Ok(out)
                }
                Err(e) => {
                    println!("\n{}", style(format!("✘ Error: {}", e)).red());
                    let _ = sandbox.finish_turn(false).await;
                    Err(e)
                }
            }
        }
        _ = tokio::signal::ctrl_c() => {
            printer.abort();
            println!("\n{}", style("[⛔ Cancelled]").red().bold());
            let _ = sandbox.finish_turn(false).await;
            anyhow::bail!("Execution cancelled by user")
        }
    };

    if let Some(ref traj_path) = opts.output_trajectory {
        let traj_json = serde_json::json!({
            "goal": opts.goal,
            "model": effective_config.active_model,
            "provider": effective_config.active_provider,
            "success": run_res.is_ok(),
            "messages_count": engine.global_messages.len(),
            "history": engine.global_messages
        });
        if let Ok(content) = serde_json::to_string_pretty(&traj_json) {
            let _ = std::fs::write(traj_path, content);
            println!("Trajectory exported to: {}", traj_path);
        }
    }

    if let Some(ref sona) = engine.sona {
        save_sona_state(&data_dir, sona);
    }

    if let Some(telemetry) = crate::core::TelemetryManager::global() {
        telemetry.flush().await;
    }

    let _ = run_res?;
    Ok(())
}
