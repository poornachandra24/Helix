use anyhow::Result;
use console::style;
use std::io::{self, Write};
use tokio::sync::mpsc;

use crate::config;
use crate::core::context;
use crate::core::engine;
use crate::evolution;
use crate::memory;
use crate::core::metrics;
use crate::core::persistence;
use crate::tools::sandbox;
use crate::memory::skills;

use super::helpers::{
    build_context, build_model, build_system_prompt, build_tool_registry,
    count_omitted_turns, init_mcp_tools, print_banner,
};
use super::bench::run_benchmark;

const HELP_TEXT: &str = "\
Commands:
  /config                — reconfigure provider / model
  /providers             — list saved providers
  /use <name> [model]    — switch to a saved provider instantly
  /sessions              — list recent sessions
  /resume <id>           — load a past session into context
  /clear                 — reset context (keep session file)
  /status                — show active model, workspace, and token budget
  /benchmark             — run benchmark suite
  /save-baseline         — save current benchmark as new baseline
  /evolve                — analyze logs and propose self-evolution patch
  /evolve --auto-approve — analyze, check, test, and automatically apply if safe
  /evolve --dry-run      — analyze metrics but skip proposing edits
  /approve               — apply the pending evolution diff
  /reject [reason]       — discard the pending evolution diff
  /memory [query]        — search/list workspace memories (use --clear to wipe)
  /help                  — show this message
  /exit | /quit          — exit REPL (also accepts exit | quit)";

pub async fn run_repl(mut app_config: config::AppConfig) -> Result<()> {
    print_banner(&app_config);

    let session = persistence::Session::new(None)?;
    println!("📝 Session: {}", style(&session.id).dim());
    let sandbox = sandbox::SharedSandbox::new(app_config.sandbox_mode);
    let mut tools = build_tool_registry(sandbox.clone());
    let _mcp_registry = init_mcp_tools(&mut tools).await?;
    let data_dir = config::get_data_dir()?;
    let skill_reg = skills::SkillRegistry::new(data_dir.join("skills"))?;
    let memory_dir = data_dir.join("memory");
    let memory_engine = memory::HelixMemoryEngine::new(&memory_dir)?;

    let base_system = "You are an autonomous AI agent with access to tools. \
        Use the tools provided to complete the user's goal. \
        When you have a final answer, respond conversationally without calling any tools.";

    let system_prompt = build_system_prompt(base_system, &skill_reg);

    // Shared HTTP client for model info lookups (separate from the inference client)
    let lookup_client = model_registry_build_lookup_client();

    // Dynamically resolve context window from provider APIs / OpenRouter catalogue
    let context = build_context(&app_config, &system_prompt, &tools.descriptors(), &lookup_client).await;
    let model   = build_model(&app_config);

    let mut engine = engine::Engine::new(model, context, tools, session)
        .with_memory(memory_engine);
    // Wire up metrics using the session ID
    let metrics_collector = metrics::MetricsCollector::new(&engine.session.id);
    engine = engine.with_metrics(metrics_collector);

    // Maintain global evolution state for the session
    let mut evolution_state = evolution::EvolutionState::new();

    loop {
        print!("\n{} ", style(">").bold().blue());
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();

        match trimmed {
            "exit" | "quit" | "/exit" | "/quit" => break,
            "/help"         => { println!("{}", HELP_TEXT); continue; }
            "/clear"        => {
                engine.global_messages.clear();
                println!("{}", style("✅ Context cleared.").green());
                continue;
            }
            "/status" => {
                println!("\n{}", style(" Helix Session Status").bold().underlined());
                println!("  Active Provider : {}", style(&app_config.active_provider).cyan().bold());
                println!("  Active Model    : {}", style(&app_config.active_model).cyan().bold());
                if let Ok(wd) = std::env::current_dir() {
                    println!("  Workspace Path  : {}", style(wd.display()).cyan());
                }

                let budget = &engine.context.budget;
                let msg_tokens: usize = engine.global_messages.iter().map(context::TokenEstimator::estimate_message).sum();
                let system_tokens = budget.system_prompt_tokens;
                let tool_tokens = budget.tool_descriptor_tokens;
                let headroom = budget.response_headroom;
                let total_used = msg_tokens + system_tokens + tool_tokens;
                let total_capacity = budget.model_window;
                let remaining = total_capacity.saturating_sub(total_used).saturating_sub(headroom);

                println!("\n{}", style(" Context Window Budget").bold());
                println!("  Total Model Window : {} tokens", style(total_capacity).yellow());
                println!("  System Prompt Cost : {} tokens", style(system_tokens).dim());
                println!("  Tool Definitions   : {} tokens", style(tool_tokens).dim());
                println!("  Active Chat History: {} tokens ({} messages)", style(msg_tokens).cyan(), engine.global_messages.len());
                println!("  Response Headroom  : {} tokens", style(headroom).dim());
                println!("  Remaining Headroom : {} tokens", if remaining > 1000 { style(remaining).green().bold() } else { style(remaining).red().bold() });

                let omitted_turns = count_omitted_turns(&engine.global_messages);
                let compaction_events = if let Some(ref m) = engine.metrics {
                    m.turns().iter().filter(|t| t.compaction_fired).count()
                } else {
                    0
                };

                println!("\n{}", style(" Context Management Effectiveness").bold());
                println!("  Compaction Events  : {} triggered", style(compaction_events).yellow());
                println!("  Omitted History    : {} intermediate turns discarded", style(omitted_turns).red());

                let pct = (total_used as f64 / total_capacity as f64) * 100.0;
                let width = 25;
                let filled = ((pct / 100.0) * width as f64).round() as usize;
                let filled = filled.min(width);
                let empty = width - filled;
                let bar_color = if pct > 85.0 {
                    style("█".repeat(filled)).red()
                } else if pct > 70.0 {
                    style("█".repeat(filled)).yellow()
                } else {
                    style("█".repeat(filled)).green()
                };
                let bar = format!("{}{}", bar_color, style("░".repeat(empty)).dim());
                println!("  Utilization        : [{}] {:.1}% used", bar, pct);

                continue;
            }
            "/clear_memory" | "/clear_memories" => {
                println!("{}", style("💡 Hint: /clear_memory is deprecated. Use `/memory --clear` instead.").yellow());
                if let Some(ref mut memory_engine) = engine.memory {
                    if let Ok(workspace_dir) = std::env::current_dir() {
                        let workspace_str = workspace_dir.to_string_lossy().to_string();
                        match memory_engine.clear_workspace(&workspace_str) {
                            Ok(()) => println!("{}", style("✅ Workspace memory cleared successfully.").green()),
                            Err(e) => println!("❌ Error: {}", e),
                        }
                    }
                }
                continue;
            }
            _ => {}
        }

        // Unified /memory command
        if trimmed.starts_with("/memory") || trimmed.starts_with("/memories") {
            let is_deprecated = trimmed.starts_with("/memories");
            let raw_arg = if is_deprecated {
                trimmed.strip_prefix("/memories").unwrap().trim()
            } else {
                trimmed.strip_prefix("/memory").unwrap().trim()
            };

            if is_deprecated {
                println!("{}", style("💡 Hint: /memories is deprecated. Use `/memory [query]` instead.").yellow());
            }

            if let Some(ref mut memory_engine) = engine.memory {
                if let Ok(workspace_dir) = std::env::current_dir() {
                    let workspace_str = workspace_dir.to_string_lossy().to_string();
                    if raw_arg == "--clear" {
                        match memory_engine.clear_workspace(&workspace_str) {
                            Ok(()) => println!("{}", style("✅ Workspace memory cleared successfully.").green()),
                            Err(e) => println!("❌ Error: {}", e),
                        }
                    } else if raw_arg.is_empty() {
                        println!("\n{}", style("Recent memories for this workspace:").bold());
                        let query_res = (|| -> Result<()> {
                            let mut stmt = memory_engine.db.prepare(
                                "SELECT text, created_at FROM memory_metadata WHERE workspace_path = ? ORDER BY id DESC LIMIT 5"
                            )?;
                            let mut rows = stmt.query([&workspace_str])?;
                            let mut count = 0;
                            while let Some(row) = rows.next()? {
                                let text: String = row.get::<_, String>(0)?;
                                let created_at: String = row.get::<_, String>(1)?;
                                println!("  {} [{}]", style(text.replace('\n', " ↵ ")).cyan(), style(created_at).dim());
                                count += 1;
                            }
                            if count == 0 {
                                println!("  No memories recorded yet.");
                            }
                            Ok(())
                        })();
                        if let Err(e) = query_res {
                            println!("❌ Error: {}", e);
                        }
                    } else {
                        println!("\nSearching memories for '{}'...", raw_arg);
                        match memory_engine.search(raw_arg, &workspace_str, 5) {
                            Ok(matches) => {
                                if matches.is_empty() {
                                    println!("  No matches found.");
                                } else {
                                    for m in matches {
                                        let file_info = m.file_path.as_ref().map(|p| format!(" (file: {})", p)).unwrap_or_default();
                                        println!("  {}{} [similarity: {:.2}]", style(m.text.replace('\n', " ↵ ")).cyan(), file_info, m.score);
                                    }
                                }
                            }
                            Err(e) => println!("❌ Error searching: {}", e),
                        }
                    }
                }
            }
            continue;
        }

        // ── /config ───────────────────────────────────────────
        if trimmed == "/config" {
            let new_config = config::interactive_setup(Some(app_config.clone()))?;
            app_config = new_config.clone();
            sandbox.set_mode(app_config.sandbox_mode);
            let new_system = build_system_prompt(base_system, &skill_reg);
            let new_context = build_context(&new_config, &new_system, &engine.tools.descriptors(), &lookup_client).await;
            engine.update_model(build_model(&new_config), new_context);
            println!("{}", style(format!("✅ Switched to {} / {}", app_config.active_provider, app_config.active_model)).green());
            continue;
        }

        // ── /providers ────────────────────────────────────────
        if trimmed == "/providers" {
            println!("\n{}", style("Saved providers:").bold());
            for p in &app_config.providers {
                let active = if p.name == app_config.active_provider { " ◀ active" } else { "" };
                println!("  {}{}", style(p).cyan(), style(active).green().bold());
            }
            continue;
        }

        // ── /use <name> [model] ───────────────────────────────
        if let Some(rest) = trimmed.strip_prefix("/use ") {
            let mut parts = rest.splitn(2, ' ');
            let name  = parts.next().unwrap_or("").trim();
            let model_override = parts.next().map(str::trim);
            match config::switch_provider(&mut app_config, name, model_override) {
                Ok(()) => {
                    sandbox.set_mode(app_config.sandbox_mode);
                    let new_context = build_context(&app_config, &system_prompt, &engine.tools.descriptors(), &lookup_client).await;
                    engine.update_model(build_model(&app_config), new_context);
                    println!("{}", style(format!("✅ Using {} / {}", app_config.active_provider, app_config.active_model)).green());
                }
                Err(e) => println!("{}", style(format!("❌ {}", e)).red()),
            }
            continue;
        }

        // ── /sessions ─────────────────────────────────────────
        if trimmed == "/sessions" {
            match persistence::list_sessions() {
                Ok(sessions) if sessions.is_empty() => println!("No saved sessions."),
                Ok(sessions) => {
                    println!("\n{}", style("Recent sessions (newest first):").bold());
                    for s in sessions.iter().take(15) {
                        println!("  {} — {} events — {}", 
                            style(&s.id).cyan(),
                            s.event_count,
                            s.modified_at.format("%Y-%m-%d %H:%M")
                        );
                    }
                    println!("  Use {} to load one.", style("/resume <id>").yellow());
                }
                Err(e) => println!("{}", style(format!("❌ {}", e)).red()),
            }
            continue;
        }

        // ── /resume <id> ──────────────────────────────────────
        if let Some(id) = trimmed.strip_prefix("/resume ") {
            let id = id.trim();
            match persistence::list_sessions() {
                Ok(sessions) => {
                    if let Some(meta) = sessions.into_iter().find(|s| s.id == id) {
                        match persistence::Session::load_messages(&meta.path) {
                            Ok(msgs) => {
                                engine.global_messages = msgs;
                                println!("{}", style(format!("✅ Resumed '{}' ({} messages).", id, engine.global_messages.len())).green());
                            }
                            Err(e) => println!("{}", style(format!("❌ {}", e)).red()),
                        }
                    } else {
                        println!("{}", style(format!("❌ Session '{}' not found. Run /sessions to list.", id)).red());
                    }
                }
                Err(e) => println!("{}", style(format!("❌ {}", e)).red()),
            }
            continue;
        }

        // ── Benchmarks & Evolution ─────────────────────────────
        if trimmed == "/benchmark" {
            let _ = run_benchmark(&app_config, false).await;
            continue;
        }
        if trimmed == "/baseline" || trimmed == "/save-baseline" {
            if trimmed == "/baseline" {
                println!("{}", style("💡 Hint: /baseline is deprecated. Use `/save-baseline` instead.").yellow());
            }
            let _ = run_benchmark(&app_config, true).await;
            continue;
        }
        if trimmed.starts_with("/evolve") {
            let dry_run = trimmed.contains("--dry-run");
            let auto_approve = trimmed.contains("--auto-approve") || trimmed.contains("--auto");
            evolution::handle_evolve(&app_config, &mut evolution_state, dry_run, auto_approve).await;
            continue;
        }
        if trimmed == "/approve" {
            evolution::handle_approve(&app_config, &mut evolution_state).await;
            continue;
        }
        if let Some(reason) = trimmed.strip_prefix("/reject") {
            evolution::handle_reject(&mut evolution_state, reason.trim());
            continue;
        }

        if trimmed.is_empty() { continue; }

        // ── Normal turn with streaming output ─────────────────
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        // Spawn printer task — handles streaming tokens and interactive spinner
        let printer = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(80));
            let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut frame_idx = 0;
            let mut show_spinner = false;
            let mut received_any = false;
            let mut spinner_suffix = "thinking...".to_string();
            let thinking_style = console::Style::new().color256(244); // Dim grey
            let spinner_style = console::Style::new().color256(51).bold(); // Cyber Cyan

            loop {
                tokio::select! {
                    token_opt = rx.recv() => {
                        match token_opt {
                            Some(token) => {
                                if token == "\x02" {
                                    show_spinner = true;
                                    received_any = false;
                                } else if token == "\x03" {
                                    if show_spinner {
                                        print!("\r\x1B[K");
                                        io::stdout().flush().ok();
                                    }
                                    show_spinner = false;
                                } else if token.starts_with("\x1b[S") {
                                    spinner_suffix = token.strip_prefix("\x1b[S").unwrap_or("thinking...").to_string();
                                    if show_spinner && !received_any {
                                        let frame = spinner_frames[frame_idx % spinner_frames.len()];
                                        print!("\r{} {} ", spinner_style.apply_to(frame), thinking_style.apply_to(&spinner_suffix));
                                        io::stdout().flush().ok();
                                    }
                                } else if token.starts_with("\x1b[T") {
                                    if show_spinner {
                                        print!("\r\x1B[K");
                                        io::stdout().flush().ok();
                                    }
                                    let msg = token.strip_prefix("\x1b[T").unwrap_or("");
                                    println!("{}", msg);
                                    io::stdout().flush().ok();
                                } else {
                                    if show_spinner {
                                        print!("\r\x1B[K");
                                        io::stdout().flush().ok();
                                        show_spinner = false;
                                    }
                                    print!("{}", token);
                                    io::stdout().flush().ok();
                                    received_any = true;
                                }
                            }
                            None => {
                                break;
                            }
                        }
                    }
                    _ = interval.tick() => {
                        if show_spinner && !received_any {
                            let frame = spinner_frames[frame_idx % spinner_frames.len()];
                            print!("\r{} {} ", spinner_style.apply_to(frame), thinking_style.apply_to(&spinner_suffix));
                            io::stdout().flush().ok();
                            frame_idx += 1;
                        }
                    }
                }
            }
            if show_spinner {
                print!("\r\x1B[K");
                io::stdout().flush().ok();
            }
        });

        println!(); // newline before streaming output

        tokio::select! {
            res = engine.run_turn(&system_prompt, trimmed, Some(tx)) => {
                printer.await.ok(); // flush remaining tokens
                match res {
                    Ok(final_text) => {
                        if final_text.is_empty() {
                            println!("{}", style("(empty response)").dim());
                        } else {
                            println!(); // blank line after response
                        }
                    }
                    Err(e) => println!("\n{}", style(format!("❌ Error: {}", e)).red()),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                printer.abort();
                println!("\n{}", style("[⛔ Request cancelled]").red().bold());
            }
        }

        // Flush metrics to disk after every turn so /evolve can see them immediately
        if let Some(m) = &engine.metrics {
            if let Ok(state_dir) = config::get_state_dir() {
                let _ = m.flush_to_disk(&state_dir.join("sessions"));
            }
        }
    }

    // Ensure metrics are flushed to disk before exiting
    if let Some(m) = &engine.metrics {
        let sessions_dir = config::get_state_dir()?.join("sessions");
        if let Err(e) = m.flush_to_disk(&sessions_dir) {
            tracing::warn!("Failed to flush metrics: {}", e);
        }
    }

    println!("{}", style("Goodbye!").cyan());
    Ok(())
}

fn model_registry_build_lookup_client() -> reqwest::Client {
    crate::model::registry::build_lookup_client()
}
