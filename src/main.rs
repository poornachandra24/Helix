mod config;
mod context;
mod builtins;
mod engine;
mod model;
mod persistence;
mod tools;
mod skills;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use std::io::{self, Write};
use console::style;

#[derive(Parser, Debug)]
#[command(author, version, about = "Interactive Agent CLI", long_about = None)]
struct Args {
    #[command(subcommand)]
    cmd: Option<Command>,

    /// Increase verbosity (-v for info, -vv for debug)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start an interactive Chat REPL (Default)
    Chat,
    /// Run a single prompt and exit
    Run { goal: String },
    /// Configure models and providers interactively
    Config,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let log_level = match args.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(format!("harness_cli={}", log_level)))
        .with_target(false)
        .init();

    let mut app_config = config::load_config()?;

    match &args.cmd {
        Some(Command::Config) => {
            println!("Current Provider: {}", app_config.active_provider);
            println!("Current Model: {}", app_config.active_model);
            let _ = config::interactive_setup(Some(app_config))?;
        }
        Some(Command::Run { goal }) => {
            run_single(app_config, goal.to_string(), args.verbose).await?;
        }
        Some(Command::Chat) | None => {
            // Interactive Chat REPL (Session Management)
            println!("{}", style("========================================================").dim());
            println!("💬 {}", style("Harness Interactive Session Started").bold().cyan());
            println!("🤖 Provider: {}", style(&app_config.active_provider).green());
            println!("🧠 Model:    {}", style(&app_config.active_model).green());
            println!("Commands: {} to switch models, {} to quit", style("'/config'").yellow(), style("'exit'").yellow());
            println!("{}", style("========================================================").dim());

            let session = persistence::Session::new(None)?;
            let mut registry = tools::ToolRegistry::new();
            registry.register(Box::new(builtins::BashTool));
            
            let data_dir = config::get_data_dir()?;
            let skill_reg = skills::SkillRegistry::new(data_dir.join("skills"))?;
            let skills_prompt = skill_reg.load_skills_prompt().unwrap_or_default();
            let base_system_prompt = "You are an autonomous AI agent with access to tools. To complete the user's goal, you MUST use the tools provided. Once you have the final result, provide a conversational answer.";
            let system_prompt = format!("{}{}", base_system_prompt, skills_prompt);

            let model_adapter = model::OllamaAdapter::new(app_config.clone());
            let mut engine = engine::Engine::new(model_adapter, context::ContextManager::new(), registry, session);

            loop {
                print!("\n{} ", style(">").bold().blue());
                io::stdout().flush()?;
                let mut input = String::new();
                io::stdin().read_line(&mut input)?;
                let trimmed = input.trim();

                if trimmed == "exit" || trimmed == "quit" {
                    break;
                }
                if trimmed == "/config" {
                    let new_config = config::interactive_setup(Some(app_config.clone()))?;
                    app_config = new_config.clone();
                    engine.model.config = new_config;
                    println!("{}", style(format!("✅ Seamlessly switched to {} ({})", app_config.active_provider, app_config.active_model)).green());
                    continue;
                }
                if trimmed.is_empty() {
                    continue;
                }

                tokio::select! {
                    res = engine.run_turn(&system_prompt, trimmed) => {
                        match res {
                            Ok(final_result) => println!("\n🤖 {}", style(final_result).green()),
                            Err(e) => println!("\n❌ {}", style(format!("Error: {}", e)).red()),
                        }
                    }
                    _ = tokio::signal::ctrl_c() => {
                        println!("\n{}", style("[⛔ Request Cancelled by User]").red().bold());
                        // The engine future is dropped, gracefully terminating any pending HTTP requests
                    }
                }
            }
        }
    }

    Ok(())
}

async fn run_single(app_config: config::AppConfig, goal: String, verbosity: u8) -> Result<()> {
    let session = persistence::Session::new(None)?;
    let mut registry = tools::ToolRegistry::new();
    registry.register(Box::new(builtins::BashTool));

    let data_dir = config::get_data_dir()?;
    let skill_reg = skills::SkillRegistry::new(data_dir.join("skills"))?;
    let skills_prompt = skill_reg.load_skills_prompt().unwrap_or_default();
    
    let base_system_prompt = "You are an autonomous AI agent with access to tools. To complete the user's goal, you MUST use the tools provided. Once you have the final result, provide a conversational answer.";
    let system_prompt = format!("{}{}", base_system_prompt, skills_prompt);

    let model = model::OllamaAdapter::new(app_config);
    let mut engine = engine::Engine::new(model, context::ContextManager::new(), registry, session);
    
    println!("🚀 Starting Agent... (verbosity: {})\n", verbosity);
    let final_result = engine.run_turn(&system_prompt, &goal).await?;
    
    println!("\n✅ Final Result: {}", final_result);
    Ok(())
}
