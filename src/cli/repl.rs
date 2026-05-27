use anyhow::Result;
use console::style;
use unicode_width::UnicodeWidthStr;
use owo_colors::OwoColorize;
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
    count_omitted_turns, init_mcp_tools, load_sona_state, print_banner,
    print_status_card, save_sona_state, wrap_text,
};
use super::bench::run_benchmark;

fn print_help() {
    use comfy_table::{Table, Cell, ColumnConstraint, Width};
    use comfy_table::presets::NOTHING;
    use owo_colors::OwoColorize;

    println!("\n  {}", "HELIX CLI SYSTEM COMMANDS".cyan().bold());
    println!("  {}", "─".repeat(50).dimmed());

    let mut table = Table::new();
    table.load_preset(NOTHING);
    table.set_width(80);
    table.set_constraints(vec![
        ColumnConstraint::Absolute(Width::Fixed(26)),
        ColumnConstraint::Absolute(Width::Fixed(50)),
    ]);

    // Section 1
    table.add_row(vec![
        Cell::new("General Chat & Context:").fg(comfy_table::Color::DarkGrey).add_attribute(comfy_table::Attribute::Bold),
        Cell::new(""),
    ]);

    let general = vec![
        ("/help", "show this command guide"),
        ("/status", "show active model, context budget, SONA & evolution stats"),
        ("/clear", "reset current chat history context"),
        ("/config", "reconfigure active provider / model"),
        ("/providers", "list configured API providers"),
        ("/use <name> [model]", "hot-switch provider/model in the current session"),
        ("/sessions", "list previous chat sessions"),
        ("/resume <id>", "load a past session into the active context"),
        ("/memory [query]", "search/manage semantic memory (use --clear to wipe)"),
        ("/exit | /quit", "exit the REPL session"),
    ];
    for (cmd, desc) in general {
        table.add_row(vec![
            Cell::new(format!("  {}", cmd)).fg(comfy_table::Color::Cyan),
            Cell::new(format!("—  {}", desc)).fg(comfy_table::Color::White),
        ]);
    }

    // Section 2
    table.add_row(vec![
        Cell::new("\nSelf-Evolution System:").fg(comfy_table::Color::DarkGrey).add_attribute(comfy_table::Attribute::Bold),
        Cell::new(""),
    ]);

    let evolution = vec![
        ("/evolve", "analyze logs and propose self-evolution patch"),
        ("/evolve --auto-approve", "analyze, check, test, and automatically apply if safe"),
        ("/evolve --dry-run", "analyze metrics but skip proposing edits"),
        ("/approve", "apply the pending evolution diff"),
        ("/reject [reason]", "discard the pending code evolution patch"),
    ];
    for (cmd, desc) in evolution {
        table.add_row(vec![
            Cell::new(format!("  {}", cmd)).fg(comfy_table::Color::Cyan),
            Cell::new(format!("—  {}", desc)).fg(comfy_table::Color::White),
        ]);
    }

    // Section 3
    table.add_row(vec![
        Cell::new("\nAgent Performance Benchmarking:").fg(comfy_table::Color::DarkGrey).add_attribute(comfy_table::Attribute::Bold),
        Cell::new(""),
    ]);

    let bench = vec![
        ("/benchmark", "run benchmark suite"),
        ("/save-baseline", "save current benchmark as new baseline"),
    ];
    for (cmd, desc) in bench {
        table.add_row(vec![
            Cell::new(format!("  {}", cmd)).fg(comfy_table::Color::Cyan),
            Cell::new(format!("—  {}", desc)).fg(comfy_table::Color::White),
        ]);
    }

    println!("{table}");
}

pub async fn run_repl(mut app_config: config::AppConfig) -> Result<()> {
    // Banner is printed after engine is built so we can pass session/memory/SONA info

    let session = persistence::Session::new(None)?;
    let sandbox = sandbox::SharedSandbox::new(app_config.sandbox_mode);
    let mut tools = build_tool_registry(sandbox.clone());
    let _mcp_registry = init_mcp_tools(&mut tools).await?;
    let data_dir = config::get_data_dir()?;
    let skill_reg = skills::SkillRegistry::new(data_dir.join("skills"))?;
    let memory_dir = data_dir.join("memory");
    let memory_engine = memory::HelixMemoryEngine::new(&memory_dir)?;

    let base_system = "You are an autonomous AI agent with access to tools. \
        Use the tools provided to complete the user's goal. \
        When you have a final answer, respond conversationally without calling any tools. \
        CRITICAL FORMATTING RULES FOR TERMINAL ENVIRONMENT:\n\
        1. DO NOT USE MARKDOWN TABLES (e.g. '| Header | Header |') because they get severely mangled and wrapped when displayed inside the terminal's narrow fixed-width box layout (typically 80-110 characters). Instead, represent tabular data using nested bullet points, bold key-value listings, or record blocks (e.g. '◆ Record 1:\n  * Key: Value').\n\
        2. Keep horizontal lines and separators short. DO NOT output long horizontal line dashes like '------------------------------' or ASCII art. Keep horizontal dividers short, e.g., '---'.\n\
        3. Prioritize concise, structured lists and paragraphs so the text reads beautifully on a terminal.";

    let system_prompt = build_system_prompt(base_system, &skill_reg);

    // Shared HTTP client for model info lookups (separate from the inference client)
    let lookup_client = model_registry_build_lookup_client();

    // Dynamically resolve context window from provider APIs / OpenRouter catalogue
    let context = build_context(&app_config, &system_prompt, &tools.descriptors(), &lookup_client).await;
    let model   = build_model(&app_config);

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
    // Wire up metrics using the session ID
    let metrics_collector = metrics::MetricsCollector::new(&engine.session.id);
    engine = engine.with_metrics(metrics_collector);

    // Maintain global evolution state for the session
    let mut evolution_state = evolution::EvolutionState::new();

    // Print combined startup banner with all session details in a single card
    let memory_size = engine.memory.as_ref().map(|m| m.size()).unwrap_or(0);
    let patterns_count = engine.sona.as_ref().map(|s| s.stats().patterns_stored).unwrap_or(0);
    print_banner(&app_config, &engine.session.id, memory_size, patterns_count);

    loop {
        print!("\n{} ", style(">").bold().blue());
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();

        match trimmed {
            "exit" | "quit" | "/exit" | "/quit" => break,
            "/help"         => { print_help(); continue; }
            "/clear"        => {
                engine.global_messages.clear();
                println!("{}", style("✔ Context cleared.").green());
                continue;
            }
            "/status" => {
                let memory_size = engine.memory.as_ref().map(|m| m.size()).unwrap_or(0);
                print_status_card(
                    &engine.session.id,
                    engine.model.model_name(),
                    memory_size,
                    engine.sona.as_ref(),
                    evolution_state.pending_diff.is_some(),
                    true,
                );

                let budget = &engine.context.budget;
                let msg_tokens: usize = engine.global_messages.iter().map(context::TokenEstimator::estimate_message).sum();
                let system_tokens = budget.system_prompt_tokens;
                let tool_tokens = budget.tool_descriptor_tokens;
                let headroom = budget.response_headroom;
                let total_used = msg_tokens + system_tokens + tool_tokens;
                let total_capacity = budget.model_window;
                let remaining = total_capacity.saturating_sub(total_used).saturating_sub(headroom);

                println!("\n  {}", style("CONTEXT WINDOW BUDGET").bold().color256(111));
                println!("  {}", style("─".repeat(50)).color256(240));
                println!("  {:20}: {} tokens", style("Total Model Window").color256(244), style(total_capacity).color256(117));
                println!("  {:20}: {} tokens", style("System Prompt Cost").color256(244), style(system_tokens).color256(243));
                println!("  {:20}: {} tokens", style("Tool Definitions").color256(244), style(tool_tokens).color256(243));
                println!("  {:20}: {} tokens ({} messages)", style("Active Chat History").color256(244), style(msg_tokens).color256(117), engine.global_messages.len());
                println!("  {:20}: {} tokens", style("Response Headroom").color256(244), style(headroom).color256(243));
                println!("  {:20}: {} tokens", style("Remaining Headroom").color256(244), if remaining > 1000 { style(remaining).green().bold() } else { style(remaining).red().bold() });

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
                    style("█".repeat(filled)).color256(150)
                };
                let bar = format!("{}{}", bar_color, style("░".repeat(empty)).color256(239));
                println!("  {:20}: [{}] {:.1}% used", style("Utilization").color256(244), bar, pct);

                let omitted_turns = count_omitted_turns(&engine.global_messages);
                let compaction_events = if let Some(ref m) = engine.metrics {
                    m.turns().iter().filter(|t| t.compaction_fired).count()
                } else {
                    0
                };

                println!("\n  {}", style("CONTEXT COMPACTION SUMMARY").bold().color256(111));
                println!("  {}", style("─".repeat(50)).color256(240));
                println!("  {:20}: {} triggered", style("Compaction Events").color256(244), style(compaction_events).color256(117));
                println!("  {:20}: {} intermediate turns discarded", style("Omitted History").color256(244), style(omitted_turns).color256(203));



                if let Some(ref m) = engine.metrics {
                    let summary = m.summary();
                    println!("\n  {}", style("SELF-EVOLUTION LOGS").bold().color256(111));
                    println!("  {}", style("─".repeat(50)).color256(240));
                    println!("  {:20}: {}", style("Total Session Turns").color256(244), style(summary.turn_count).color256(117));
                    println!("  {:20}: {}", style("Total Healer Retries").color256(244), style(summary.total_healer_retries).color256(117));
                    let success_rate = (1.0 - summary.error_rate) * 100.0;
                    println!("  {:20}: {:.1}%", style("Healer Success Rate").color256(244), style(success_rate).color256(150));
                }
                println!();

                continue;
            }
            "/clear_memory" | "/clear_memories" => {
                println!("{}", style("💡 Hint: /clear_memory is deprecated. Use `/memory --clear` instead.").yellow());
                if let Some(ref mut memory_engine) = engine.memory {
                    if let Ok(workspace_dir) = std::env::current_dir() {
                        let workspace_str = workspace_dir.to_string_lossy().to_string();
                        match memory_engine.clear_workspace(&workspace_str) {
                            Ok(()) => println!("{}", style("✔ Workspace memory cleared successfully.").green()),
                            Err(e) => println!("✘ Error: {}", e),
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
                            Ok(()) => println!("{}", style("✔ Workspace memory cleared successfully.").green()),
                            Err(e) => println!("✘ Error: {}", e),
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
                            println!("✘ Error: {}", e);
                        }
                    } else {
                        println!("\nSearching memories for '{}'...", raw_arg);
                        match memory_engine.search(raw_arg, None, &workspace_str, 5) {
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
                            Err(e) => println!("✘ Error searching: {}", e),
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
            println!("{}", style(format!("✔ Switched to {} / {}", app_config.active_provider, app_config.active_model)).green());
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
                    println!("{}", style(format!("✔ Using {} / {}", app_config.active_provider, app_config.active_model)).green());
                }
                Err(e) => println!("{}", style(format!("✘ {}", e)).red()),
            }
            continue;
        }

        // ── /sessions ─────────────────────────────────────────
        if trimmed == "/sessions" {
            match persistence::list_sessions() {
                Ok(sessions) if sessions.is_empty() => println!("No saved sessions."),
                Ok(sessions) => {
                    use comfy_table::{Table, Cell, ColumnConstraint, Width};
                    use comfy_table::presets::NOTHING;
                    use owo_colors::OwoColorize;

                    println!("\n  {}", "Recent sessions (newest first):".cyan().bold());
                    println!("  {}", "─".repeat(50).dimmed());

                    let mut table = Table::new();
                    table.load_preset(NOTHING);
                    table.set_width(80);
                    table.set_constraints(vec![
                        ColumnConstraint::Absolute(Width::Fixed(40)),
                        ColumnConstraint::Absolute(Width::Fixed(15)),
                        ColumnConstraint::Absolute(Width::Fixed(25)),
                    ]);

                    table.add_row(vec![
                        Cell::new("SESSION ID").fg(comfy_table::Color::DarkGrey).add_attribute(comfy_table::Attribute::Bold),
                        Cell::new("EVENTS").fg(comfy_table::Color::DarkGrey).add_attribute(comfy_table::Attribute::Bold),
                        Cell::new("LAST ACTIVE").fg(comfy_table::Color::DarkGrey).add_attribute(comfy_table::Attribute::Bold),
                    ]);

                    for s in sessions.iter().take(15) {
                        table.add_row(vec![
                            Cell::new(&s.id).fg(comfy_table::Color::Cyan),
                            Cell::new(s.event_count.to_string()).fg(comfy_table::Color::White),
                            Cell::new(s.modified_at.format("%Y-%m-%d %H:%M").to_string()).fg(comfy_table::Color::White),
                        ]);
                    }
                    println!("{table}");
                    println!("  Use {} to load one.\n", "/resume <id>".yellow());
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
                                println!("{}", style(format!("✔ Resumed '{}' ({} messages).", id, engine.global_messages.len())).green());
                            }
                            Err(e) => println!("{}", style(format!("✘ {}", e)).red()),
                        }
                    } else {
                        println!("{}", style(format!("✘ Session '{}' not found. Run /sessions to list.", id)).red());
                    }
                }
                Err(e) => println!("{}", style(format!("✘ {}", e)).red()),
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

        let terminal_width = if let Some((_, cols)) = console::Term::stdout().size_checked() {
            cols as usize
        } else {
            80
        };
        // Clamp content_width between 50 and 110 (preventing too-wide layouts on huge screens)
        let content_width = terminal_width.saturating_sub(8).clamp(50, 110);

        // Print User Input Box — cool blue/cyan scheme (adapts to terminal theme)
        let user_border = |s: &str| s.blue().to_string();
        let user_header = |s: &str| s.blue().bold().to_string();
        let user_pipe = "│".blue().to_string();

        // Clear the user's raw input line (move cursor up 1 line and clear it)
        print!("\x1B[1A\x1B[K");
        io::stdout().flush().ok();

        // Print boxed user input
        let title = "You";
        let dashes_count = content_width.saturating_sub(title.len());
        print!("  ");
        print!("{}", user_border("╭── "));
        print!("{}", user_header(title));
        println!("{}", user_border(&format!(" {}╮", "─".repeat(dashes_count))));
        let wrapped_user = wrap_text(trimmed, content_width);
        for line in wrapped_user {
            let expanded_line = line.replace('\t', "    ");
            let padded = format!("  {:width$}  ", expanded_line, width = content_width);
            println!("  {}{}{}", user_pipe, padded, user_pipe);
        }
        println!("  {}", user_border(&format!("╰{}╯", "─".repeat(content_width + 4))));

        // Spawn printer task — handles streaming tokens and interactive spinner
        let printer = tokio::spawn(async move {
            // Assistant Response — warm amber/gold scheme (visually distinct from user box)
            let border_color_fn = |s: &str| s.yellow().to_string();
            let header_color_fn = |s: &str| s.yellow().bold().to_string();
            let pipe = "│".yellow().to_string();

            let mut interval = tokio::time::interval(std::time::Duration::from_millis(80));
            let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut frame_idx = 0;
            let mut show_spinner = true;
            let mut spinner_suffix = "thinking...".to_string();
            let thinking_style_fn = |s: &str| s.dimmed().to_string();
            let spinner_style_fn = |s: &str| s.cyan().bold().to_string();

            let mut telemetry_msgs = Vec::new();
            let start_time = std::time::Instant::now();
            let mut full_response = String::new();

            loop {
                tokio::select! {
                    token_opt = rx.recv() => {
                        match token_opt {
                            Some(token) => {
                                if token == "\x02" {
                                    show_spinner = true;
                                } else if token == "\x03" {
                                    if show_spinner {
                                        print!("\r\x1B[K");
                                        io::stdout().flush().ok();
                                    }
                                    show_spinner = false;
                                } else if token.starts_with("\x1b[S") {
                                    spinner_suffix = token.strip_prefix("\x1b[S").unwrap_or("thinking...").to_string();
                                } else if token.starts_with("\x1b[T") {
                                    if show_spinner {
                                        print!("\r\x1B[K");
                                        io::stdout().flush().ok();
                                    }
                                    let msg = token.strip_prefix("\x1b[T").unwrap_or("");
                                    telemetry_msgs.push(msg.to_string());
                                } else {
                                    show_spinner = false;
                                    full_response.push_str(&token);
                                }
                            }
                            None => {
                                break;
                            }
                        }
                    }
                    _ = interval.tick() => {
                        if show_spinner {
                            let frame = spinner_frames[frame_idx % spinner_frames.len()];
                            print!("\r  {} {} \x1B[K", spinner_style_fn(frame), thinking_style_fn(&spinner_suffix));
                            io::stdout().flush().ok();
                            frame_idx += 1;
                        }
                    }
                }
            }

            // Clear any remaining spinner line
            print!("\r\x1B[K");
            io::stdout().flush().ok();

            // Print the response box if we received any response tokens
            if !full_response.is_empty() {
                let title = "Helix";
                let dashes_count = content_width.saturating_sub(title.len());
                print!("  ");
                print!("{}", border_color_fn("╭── "));
                print!("{}", header_color_fn(title));
                println!("{}", border_color_fn(&format!(" {}╮", "─".repeat(dashes_count))));

                // Render with termimad
                let skin = termimad::MadSkin::default();
                let fmt_text = skin.text(&full_response, Some(content_width));
                let rendered = fmt_text.to_string();

                for line in rendered.lines() {
                    let expanded_line = line.replace('\t', "    ");
                    let clean_line = console::strip_ansi_codes(&expanded_line);
                    let width = clean_line.width();
                    let pad = content_width.saturating_sub(width);
                    println!("  {}  {}{}  {}", pipe, expanded_line, " ".repeat(pad), pipe);
                }

                // Close the box with a timer
                let elapsed = start_time.elapsed().as_secs_f32();
                let elapsed_str = format!(" [Elapsed: {:.2}s] ", elapsed);
                let l_len = elapsed_str.len();
                let dashes = (content_width + 4).saturating_sub(l_len);
                let left_dashes = dashes / 2;
                let right_dashes = dashes - left_dashes;
                let bottom = format!(
                    "  {}{}{}{}{}",
                    border_color_fn("╰"),
                    border_color_fn(&"─".repeat(left_dashes)),
                    header_color_fn(&elapsed_str),
                    border_color_fn(&"─".repeat(right_dashes)),
                    border_color_fn("╯")
                );
                println!("{}", bottom);
            }

            // Print telemetry after closing the box
            for msg in telemetry_msgs {
                println!("{}", msg);
            }
            io::stdout().flush().ok();
        });

        tokio::select! {
            res = engine.run_turn(&system_prompt, trimmed, Some(tx)) => {
                printer.await.ok(); // flush remaining tokens
                match res {
                    Ok(final_text) => {
                        if final_text.is_empty() {
                            println!("  {} {}", user_pipe, "(empty response)".dimmed());
                        }
                    }
                    Err(e) => println!("\n{}", format!("✘ Error: {}", e).red()),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                printer.abort();
                println!("\n{}", "[⛔ Request cancelled]".red().bold());
            }
        }

        // Flush metrics to disk after every turn so /evolve can see them immediately
        if let Some(m) = &engine.metrics {
            if let Ok(state_dir) = config::get_state_dir() {
                let _ = m.flush_to_disk(&state_dir.join("sessions"));
            }
        }

        // Save SONA state after every turn
        if let Some(ref sona) = engine.sona {
            save_sona_state(&data_dir, sona);
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
