use anyhow::Result;
use console::style;
use std::io::{self, Write};
use tokio::sync::mpsc;

use crate::config;
use crate::core::engine;
use crate::memory;
use crate::core::persistence;
use crate::tools::sandbox;
use crate::memory::skills;

use super::helpers::{
    build_context, build_model, build_system_prompt, build_tool_registry,
    init_mcp_tools,
};

pub async fn run_single(app_config: &config::AppConfig, goal: &str) -> Result<()> {
    let sandbox = sandbox::SharedSandbox::new(app_config.sandbox_mode);
    let mut tools = build_tool_registry(sandbox);
    let _mcp_registry = init_mcp_tools(&mut tools).await?;
    let data_dir = config::get_data_dir()?;
    let skill_reg = skills::SkillRegistry::new(data_dir.join("skills"))?;
    let memory_dir = data_dir.join("memory");
    let memory_engine = memory::HelixMemoryEngine::new(&memory_dir)?;

    let base_system = "You are an autonomous AI agent with access to tools. \
        Use the tools to accomplish the goal. \
        When done, provide a clear final answer.";
    let system_prompt = build_system_prompt(base_system, &skill_reg);
    let lookup_client = crate::model::registry::build_lookup_client();
    let context = build_context(app_config, &system_prompt, &tools.descriptors(), &lookup_client).await;
    let model = build_model(app_config);
    let session = persistence::Session::new(None)?;

    let mut engine = engine::Engine::new(model, context, tools, session)
        .with_memory(memory_engine);

    println!("🚀 Running: {}\n", style(goal).bold());

    // Streaming for non-interactive run too
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let printer = tokio::spawn(async move {
        while let Some(token) = rx.recv().await {
            print!("{}", token);
            io::stdout().flush().ok();
        }
    });

    tokio::select! {
        res = engine.run_turn(&system_prompt, goal, Some(tx)) => {
            printer.await.ok();
            match res {
                Ok(_) => println!("\n\n{}", style("✅ Done.").green().bold()),
                Err(e) => println!("\n{}", style(format!("❌ Error: {}", e)).red()),
            }
        }
        _ = tokio::signal::ctrl_c() => {
            printer.abort();
            println!("\n{}", style("[⛔ Cancelled]").red().bold());
        }
    }

    Ok(())
}
