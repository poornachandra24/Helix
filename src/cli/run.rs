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
    build_context, build_model, build_system_prompt, build_tool_registry, init_mcp_tools,
    load_sona_state, save_sona_state,
};

pub async fn run_single(app_config: &config::AppConfig, goal: &str) -> Result<()> {
    let data_dir = config::get_data_dir()?;
    let skill_reg = skills::SkillRegistry::new(data_dir.join("skills"))?;
    let sandbox = sandbox::SharedSandbox::new(app_config.sandbox_mode);
    let mut tools = build_tool_registry(sandbox, data_dir.join("skills"));
    let _mcp_registry = init_mcp_tools(&mut tools).await?;
    let memory_dir = data_dir.join("memory");
    let memory_engine = memory::HelixMemoryEngine::new(&memory_dir)?;

    let base_system = "You are an autonomous AI agent with access to tools. \
        Use the tools to accomplish the goal. \
        When done, provide a clear final answer. \
        CRITICAL FORMATTING RULES FOR TERMINAL ENVIRONMENT:\n\
        1. DO NOT USE MARKDOWN TABLES (e.g. '| Header | Header |') because they get severely mangled and wrapped when displayed inside the terminal's narrow fixed-width box layout (typically 80-110 characters). Instead, represent tabular data using nested bullet points, bold key-value listings, or record blocks (e.g. '◆ Record 1:\n  * Key: Value').\n\
        2. Keep horizontal lines and separators short. DO NOT output long horizontal line dashes like '------------------------------' or ASCII art. Keep horizontal dividers short, e.g., '---'.\n\
        3. Prioritize concise, structured lists and paragraphs so the text reads beautifully on a terminal.";
    let system_prompt = build_system_prompt(base_system, &skill_reg);
    let lookup_client = crate::model::registry::build_lookup_client();
    let context = build_context(
        app_config,
        &system_prompt,
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

    let mut engine = engine::Engine::new(model, context, tools, session)
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
