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
    save_sona_state,
};

pub async fn run_single(app_config: &config::AppConfig, goal: &str) -> Result<()> {
    let data_dir = config::get_data_dir()?;
    let _skill_reg = skills::SkillRegistry::new(data_dir.join("skills"))?;
    let sandbox = sandbox::SharedSandbox::new(app_config.sandbox_mode);
    let mut tools = build_tool_registry(sandbox, data_dir.join("skills"));

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

    let memory_dir = data_dir.join("memory");
    let memory_engine = memory::HelixMemoryEngine::new(&memory_dir)?;

    let base_system = "You are an autonomous AI agent with access to tools. \
        Use the tools to accomplish the goal. \
        When done, provide a clear final answer. \
        CRITICAL FORMATTING RULES FOR TERMINAL ENVIRONMENT:\n\
        1. DO NOT USE MARKDOWN TABLES (e.g. '| Header | Header |') because they get severely mangled and wrapped when displayed inside the terminal's narrow fixed-width box layout (typically 80-110 characters). Instead, represent tabular data using nested bullet points, bold key-value listings, or record blocks (e.g. '◆ Record 1:\n  * Key: Value').\n\
        2. Keep horizontal lines and separators short. DO NOT output long horizontal line dashes like '------------------------------' or ASCII art. Keep horizontal dividers short, e.g., '---'.\n\
        3. Prioritize concise, structured lists and paragraphs so the text reads beautifully on a terminal.\n\
        4. When you need up-to-date facts, current news, search results, or information beyond your training cutoff (e.g. recent events, sports scores, or future events like the 2026 World Cup), you MUST use the 'web_search' tool FIRST. Do not guess, make up, or hallucinate details. Use 'web_search' to find search results, and then use 'web_fetch' on specific result URLs if you need to read the full page content.";

    let system_prompt = base_system.to_string();

    let lookup_client = crate::model::registry::build_lookup_client();
    let context = build_context(
        app_config,
        base_system,
        &tools.descriptors(),
        &lookup_client,
    )
    .await;
    let model = build_model(app_config);
    let session = persistence::Session::new(None)?;

    let sona_config = ruvector_sona::SonaConfig {
        hidden_dim: 384,
        embedding_dim: 384,
        ..Default::default()
    };
    let sona_engine = ruvector_sona::SonaEngine::with_config(sona_config);
    load_sona_state(&data_dir, &sona_engine);

    let mut engine = engine::Engine::new(model, context, tools, session, core_tools, dynamic_state)
        .with_memory(memory_engine)
        .with_sona(sona_engine);

    println!("🚀 Running: {}\n", style(goal).bold());

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

    tokio::select! {
        res = engine.run_turn(&system_prompt, goal, Some(tx)) => {
            printer.await.ok();
            match res {
                Ok(_) => println!("\n\n{}", style("✔ Done.").green().bold()),
                Err(e) => println!("\n{}", style(format!("✘ Error: {}", e)).red()),
            }
        }
        _ = tokio::signal::ctrl_c() => {
            printer.abort();
            println!("\n{}", style("[⛔ Cancelled]").red().bold());
        }
    }

    if let Some(ref sona) = engine.sona {
        save_sona_state(&data_dir, sona);
    }

    Ok(())
}
