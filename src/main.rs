use anyhow::Result;
use clap::{Parser, Subcommand};

use helix::{cli, config};

#[derive(Parser, Debug)]
#[command(author, version, about = "Helix — autonomous AI agent CLI", long_about = None)]
struct Args {
    #[command(subcommand)]
    cmd: Option<Command>,

    /// Verbosity: -v = info, -vv = debug
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Uninstall Helix CLI and clean up configuration
    #[arg(long)]
    uninstall: bool,

    /// Resume a past session by ID
    #[arg(long, short)]
    resume: Option<String>,

    /// Enable Logfire OpenTelemetry tracing
    #[arg(long, alias = "logfire")]
    trace: bool,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Interactive chat REPL (default)
    Chat {
        /// Resume a past session by ID
        #[arg(long, short)]
        resume: Option<String>,
    },
    /// Resume a past session by ID
    Resume {
        /// The session ID to resume
        session_id: String,
    },
    /// Run a single goal and exit
    Run {
        /// The goal or instruction to execute
        goal: String,

        /// Automatically approve tool execution without interactive confirmation prompts
        #[arg(short = 'y', long = "yes", alias = "non-interactive")]
        yes: bool,

        /// Model to use for this execution
        #[arg(short, long)]
        model: Option<String>,

        /// Provider name or endpoint to use
        #[arg(short, long)]
        provider: Option<String>,

        /// API key override
        #[arg(long)]
        api_key: Option<String>,

        /// Base URL override
        #[arg(long)]
        api_base: Option<String>,

        /// Disable local ONNX semantic memory and embeddings (lean benchmark mode)
        #[arg(long)]
        no_memory: bool,

        /// Disable secondary LLM self-reflection calls to minimize harness tax
        #[arg(long)]
        no_reflection: bool,

        /// Maximum agent tool loop iterations (default: 30, 0 for unlimited)
        #[arg(long, default_value_t = 30)]
        max_iterations: usize,

        /// Reasoning effort / thinking level (off, low, medium, high, or budget in tokens)
        #[arg(long = "thinking", alias = "reasoning-effort")]
        thinking: Option<String>,

        /// Context window size override in tokens
        #[arg(long)]
        context_window: Option<usize>,

        /// Response headroom / max output token reservation
        #[arg(long = "response-headroom", alias = "max-tokens")]
        response_headroom: Option<usize>,

        /// Image file path or URL to attach to the initial prompt (repeatable)
        #[arg(long = "image", value_name = "PATH")]
        images: Vec<String>,

        /// Path to export execution trajectory JSON (for Harbor / evaluation leaderboards)
        #[arg(long)]
        output_trajectory: Option<String>,

        /// Enable Logfire OpenTelemetry tracing for this run
        #[arg(long, alias = "logfire")]
        trace: bool,
    },
    /// Configure models and providers
    Config,
    /// Uninstall Helix CLI and clean up configuration files
    Uninstall,
}

fn perform_uninstall() -> Result<()> {
    println!("This will delete the Helix configuration, databases, and the executable.");
    print!("Are you sure you want to uninstall Helix? (y/N): ");
    use std::io::{BufRead, Write};
    std::io::stdout().flush()?;
    let mut line = String::new();
    let stdin = std::io::stdin();
    stdin.lock().read_line(&mut line)?;
    if line.trim().eq_ignore_ascii_case("y") {
        // 1. Delete config dir (helix)
        if let Some(config_dir) = config::get_config_dir().ok().filter(|p| p.exists()) {
            println!("Removing config directory: {:?}", config_dir);
            let _ = std::fs::remove_dir_all(&config_dir);
        }

        // 2. Delete data dir (helix)
        if let Some(data_dir) = config::get_data_dir().ok().filter(|p| p.exists()) {
            println!("Removing data directory: {:?}", data_dir);
            let _ = std::fs::remove_dir_all(&data_dir);
        }

        // 3. Delete state dir (helix)
        if let Some(state_dir) = config::get_state_dir().ok().filter(|p| p.exists()) {
            println!("Removing state directory: {:?}", state_dir);
            let _ = std::fs::remove_dir_all(&state_dir);
        }

        // 4. Delete the executable
        if let Ok(exe_path) = std::env::current_exe() {
            println!("Removing executable: {:?}", exe_path);
            let _ = std::fs::remove_file(&exe_path);
        }
        println!("✓ Helix has been successfully uninstalled.");
    } else {
        println!("Uninstall cancelled.");
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Check for uninstall flag or subcommand first before loading config
    let is_uninstall = args.uninstall || matches!(args.cmd, Some(Command::Uninstall));
    if is_uninstall {
        perform_uninstall()?;
        return Ok(());
    }

    let mut app_config = config::load_config()?;

    // Check if --trace was set on CLI
    let trace_cli = args.trace || matches!(&args.cmd, Some(Command::Run { trace: true, .. }));
    if trace_cli {
        app_config.enable_telemetry = true;
    }

    let log_level = match args.verbose {
        0 => "off",
        1 => "warn",
        2 => "info",
        _ => "debug",
    };

    let _telemetry = helix::core::TelemetryManager::init(
        log_level,
        app_config.enable_telemetry,
        app_config.logfire_token.clone(),
    )?;

    let resume_id = args.resume.clone().or_else(|| match &args.cmd {
        Some(Command::Chat { resume }) => resume.clone(),
        Some(Command::Resume { session_id }) => Some(session_id.clone()),
        _ => None,
    });

    let res: Result<()> = match &args.cmd {
        Some(Command::Config) => {
            println!(
                "Current: {} / {}",
                app_config.active_provider, app_config.active_model
            );
            match config::interactive_setup(Some(app_config)) {
                Ok(new_config) => {
                    println!(
                        "Now using: {} / {}",
                        new_config.active_provider, new_config.active_model
                    );
                }
                Err(_) => {
                    println!("\nConfiguration cancelled.");
                }
            }
            Ok(())
        }

        Some(Command::Run {
            goal,
            yes,
            model,
            provider,
            api_key,
            api_base,
            no_memory,
            no_reflection,
            max_iterations,
            thinking,
            context_window,
            response_headroom,
            images,
            output_trajectory,
            trace: _,
        }) => {
            let opts = cli::run::RunOptions {
                goal: goal.clone(),
                yes: *yes,
                model: model.clone(),
                provider: provider.clone(),
                api_key: api_key.clone(),
                api_base: api_base.clone(),
                no_memory: *no_memory,
                no_reflection: *no_reflection,
                max_iterations: *max_iterations,
                thinking: thinking.clone(),
                context_window: *context_window,
                response_headroom: *response_headroom,
                images: images.clone(),
                output_trajectory: output_trajectory.clone(),
            };
            cli::run::run_single(&app_config, opts).await
        }

        Some(Command::Chat { .. }) | Some(Command::Resume { .. }) | None => {
            cli::repl::run_repl(app_config, resume_id).await
        }

        Some(Command::Uninstall) => {
            // Already handled above
            Ok(())
        }
    };

    if let Some(telemetry) = helix::core::TelemetryManager::global() {
        telemetry.shutdown().await;
    }

    res
}
