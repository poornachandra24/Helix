use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use helix::{config, cli};

#[derive(Parser, Debug)]
#[command(author, version, about = "Helix — autonomous AI agent CLI", long_about = None)]
struct Args {
    #[command(subcommand)]
    cmd: Option<Command>,

    /// Verbosity: -v = info, -vv = debug
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Interactive chat REPL (default)
    Chat,
    /// Run a single goal and exit
    Run {
        /// The goal or instruction to execute
        goal: String,
    },
    /// Configure models and providers
    Config,
    /// Run the benchmark suite against the current model
    Benchmark {
        /// Save results as the new baseline
        #[arg(long)]
        update_baseline: bool,
    },
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
        .with_env_filter(EnvFilter::new(format!("helix={}", log_level)))
        .with_target(false)
        .init();

    let app_config = config::load_config()?;

    match &args.cmd {
        Some(Command::Config) => {
            println!("Current: {} / {}", app_config.active_provider, app_config.active_model);
            let new_config = config::interactive_setup(Some(app_config))?;
            println!("✅ Now using: {} / {}", new_config.active_provider, new_config.active_model);
        }

        Some(Command::Run { goal }) => {
            cli::run::run_single(&app_config, goal).await?;
        }

        Some(Command::Benchmark { update_baseline }) => {
            cli::bench::run_benchmark(&app_config, *update_baseline).await?;
        }

        Some(Command::Chat) | None => {
            cli::repl::run_repl(app_config).await?;
        }
    }

    Ok(())
}
