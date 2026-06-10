use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

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
}

#[derive(Subcommand, Debug)]
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
    },
    /// Configure models and providers
    Config,
    /// Uninstall Helix CLI and clean up configuration files
    Uninstall,
}

fn perform_uninstall() -> Result<()> {
    println!("This will delete the Helix configuration, databases, and the executable.");
    print!("Are you sure you want to uninstall Helix? (y/N): ");
    use std::io::{Write, BufRead};
    std::io::stdout().flush()?;
    let mut line = String::new();
    let stdin = std::io::stdin();
    stdin.lock().read_line(&mut line)?;
    if line.trim().eq_ignore_ascii_case("y") {
        // 1. Delete config dir (harness-cli)
        if let Ok(config_dir) = config::get_config_dir() {
            if config_dir.exists() {
                println!("Removing config directory: {:?}", config_dir);
                let _ = std::fs::remove_dir_all(&config_dir);
            }
        }

        // Also clean up ~/.config/helix if it exists
        let home_dir = std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())
            .map(std::path::PathBuf::from);

        if let Some(home) = home_dir {
            let legacy_config = home.join(".config").join("helix");
            if legacy_config.exists() {
                println!("Removing legacy config directory: {:?}", legacy_config);
                let _ = std::fs::remove_dir_all(&legacy_config);
            }
        }

        // 2. Delete data dir (harness-cli)
        if let Ok(data_dir) = config::get_data_dir() {
            if data_dir.exists() {
                println!("Removing data directory: {:?}", data_dir);
                let _ = std::fs::remove_dir_all(&data_dir);
            }
        }

        // 3. Delete state dir (harness-cli)
        if let Ok(state_dir) = config::get_state_dir() {
            if state_dir.exists() {
                println!("Removing state directory: {:?}", state_dir);
                let _ = std::fs::remove_dir_all(&state_dir);
            }
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

    let resume_id = args.resume.clone().or_else(|| {
        match &args.cmd {
            Some(Command::Chat { resume }) => resume.clone(),
            Some(Command::Resume { session_id }) => Some(session_id.clone()),
            _ => None,
        }
    });

    match &args.cmd {
        Some(Command::Config) => {
            println!(
                "Current: {} / {}",
                app_config.active_provider, app_config.active_model
            );
            match config::interactive_setup(Some(app_config)) {
                Ok(new_config) => {
                    println!(
                        "✅ Now using: {} / {}",
                        new_config.active_provider, new_config.active_model
                    );
                }
                Err(_) => {
                    println!("\nConfiguration cancelled.");
                }
            }
        }

        Some(Command::Run { goal }) => {
            cli::run::run_single(&app_config, goal).await?;
        }

        Some(Command::Chat { .. }) | Some(Command::Resume { .. }) | None => {
            cli::repl::run_repl(app_config, resume_id).await?;
        }

        Some(Command::Uninstall) => {
            // Already handled above
        }
    }

    Ok(())
}
