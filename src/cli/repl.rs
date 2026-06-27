#![allow(clippy::needless_borrow, clippy::collapsible_if, clippy::print_literal)]

use anyhow::Result;
use console::style;
use owo_colors::OwoColorize;
use std::io::{self, Write};
use tokio::sync::mpsc;
use unicode_width::UnicodeWidthStr;

use crate::config;
use crate::core::context;
use crate::core::engine;
use crate::core::metrics;
use crate::core::persistence;
use crate::memory;
use crate::memory::skills;
use crate::tools::sandbox;
use chrono::Local;

use super::helpers::{
    build_context, build_model, build_system_prompt, build_tool_registry, count_omitted_turns,
    init_mcp_tools, load_sona_state, print_banner, print_status_card, save_sona_state, wrap_text,
};

/// Returns the usable content width for boxed output, re-queried live so
/// resizing the terminal mid-session is automatically reflected next turn.
#[inline]
fn get_content_width() -> usize {
    let cols = console::Term::stdout()
        .size_checked()
        .map(|(_, c)| c as usize)
        .unwrap_or(80);
    // Leave 8 columns for the box borders + padding, floor at 40.
    cols.saturating_sub(8).max(40)
}

fn print_message(role: &str, content: &str) {
    let content_width = get_content_width();
    let trimmed = content.trim();
    if role == "user" {
        let user_border = |s: &str| s.blue().to_string();
        let user_header = |s: &str| s.blue().bold().to_string();
        let user_pipe = "│".blue().to_string();

        let title = "You";
        let dashes_count = content_width.saturating_sub(title.len());
        print!("  ");
        print!("{}", user_border("╭── "));
        print!("{}", user_header(title));
        println!(
            "{}",
            user_border(&format!(" {}╮", "─".repeat(dashes_count)))
        );
        let wrapped_user = wrap_text(trimmed, content_width);
        for line in wrapped_user {
            let expanded_line = line.replace('\t', "    ");
            let padded = format!("  {:width$}  ", expanded_line, width = content_width);
            println!("  {}{}{}", user_pipe, padded, user_pipe);
        }
        println!(
            "  {}",
            user_border(&format!("╰{}╯", "─".repeat(content_width + 4)))
        );
    } else {
        let border_color_fn = |s: &str| s.yellow().to_string();
        let header_color_fn = |s: &str| s.yellow().bold().to_string();
        let assistant_pipe = "│".yellow().to_string();

        let title = "Helix";
        let dashes_count = content_width.saturating_sub(title.len());
        print!("  ");
        print!("{}", border_color_fn("╭── "));
        print!("{}", header_color_fn(title));
        println!(
            "{}",
            border_color_fn(&format!(" {}╮", "─".repeat(dashes_count)))
        );
        let wrapped_assistant = wrap_text(trimmed, content_width);
        for line in wrapped_assistant {
            let expanded_line = line.replace('\t', "    ");
            let padded = format!("  {:width$}  ", expanded_line, width = content_width);
            println!("  {}{}{}", assistant_pipe, padded, assistant_pipe);
        }
        println!(
            "  {}",
            border_color_fn(&format!("╰{}╯", "─".repeat(content_width + 4)))
        );
    }
}

fn print_help() {
    use comfy_table::presets::NOTHING;
    use comfy_table::{Cell, ColumnConstraint, Table, Width};
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
        Cell::new("Core Commands & Session Control:")
            .fg(comfy_table::Color::DarkGrey)
            .add_attribute(comfy_table::Attribute::Bold),
        Cell::new(""),
    ]);

    let general = vec![
        ("/help", "show this command guide"),
        (
            "/status",
            "show active model, context budget, SONA & optimization stats",
        ),
        ("/clear", "reset current chat history context"),
        (
            "/forget | /purge",
            "delete the last user/assistant turn from active history, session file, and memory",
        ),
        ("/config", "reconfigure active provider / model"),
        ("/providers", "list configured API providers"),
        (
            "/use <name> [model]",
            "hot-switch provider/model in the current session",
        ),
        ("/sessions", "list previous chat sessions"),
        (
            "/resume <id>",
            "load a past session into the active context",
        ),
        (
            "/memory [query]",
            "search/manage semantic memory (use --clear to wipe)",
        ),
        (
            "/thinking [level]",
            "set or show reasoning effort/budget (low, medium, high, off, or tokens)",
        ),
        (
            "/optimize",
            "force SONA neural adaptation & parameter consolidation",
        ),
        ("/exit | /quit", "exit the REPL session"),
    ];
    for (cmd, desc) in general {
        table.add_row(vec![
            Cell::new(format!("  {}", cmd)).fg(comfy_table::Color::Cyan),
            Cell::new(format!("—  {}", desc)).fg(comfy_table::Color::White),
        ]);
    }

    println!("{table}");
}

fn get_matching_commands_inline(input: &str, config: &config::AppConfig) -> Vec<(String, String)> {
    let mut commands = vec![
        ("/help".to_string(), "show this command guide".to_string()),
        (
            "/status".to_string(),
            "show active model, context budget, SONA & optimization stats".to_string(),
        ),
        (
            "/clear".to_string(),
            "reset current chat history context".to_string(),
        ),
        (
            "/forget".to_string(),
            "delete the last user/assistant turn from active history, session file, and memory"
                .to_string(),
        ),
        (
            "/config".to_string(),
            "reconfigure active provider / model".to_string(),
        ),
        (
            "/providers".to_string(),
            "list configured API providers".to_string(),
        ),
        (
            "/use".to_string(),
            "hot-switch provider/model in the current session".to_string(),
        ),
        (
            "/sessions".to_string(),
            "list previous chat sessions".to_string(),
        ),
        (
            "/resume".to_string(),
            "load a past session into the active context".to_string(),
        ),
        (
            "/memory".to_string(),
            "search/manage semantic memory (use --clear to wipe)".to_string(),
        ),
        (
            "/thinking".to_string(),
            "set or show reasoning level (low, medium, high, off, or integer budget)".to_string(),
        ),
        (
            "/optimize".to_string(),
            "force SONA neural adaptation & parameter consolidation".to_string(),
        ),
        ("/exit".to_string(), "exit the REPL session".to_string()),
    ];

    let query = input.to_lowercase();
    let query_word = query.split_whitespace().next().unwrap_or("");
    if query_word.is_empty() || !query_word.starts_with('/') {
        return Vec::new();
    }

    if query_word.starts_with("/p") {
        commands.push((
            "/purge".to_string(),
            "delete the last user/assistant turn from active history, session file, and memory"
                .to_string(),
        ));
    }
    if query_word.starts_with("/q") {
        commands.push(("/quit".to_string(), "exit the REPL session".to_string()));
    }

    // Dynamic completions for /use
    if query_word == "/use" {
        let rest = if input.len() > 4 {
            &input[4..].trim_start()
        } else {
            ""
        };
        let parts: Vec<&str> = rest.split_whitespace().collect();
        if parts.is_empty() || (parts.len() == 1 && !rest.ends_with(' ')) {
            let prefix = parts.first().unwrap_or(&"").to_lowercase();
            let mut matches = Vec::new();
            for p in &config.providers {
                if p.name.to_lowercase().starts_with(&prefix) {
                    matches.push((
                        format!("/use {}", p.name),
                        format!("switch active provider to '{}'", p.name),
                    ));
                }
            }
            if "auto".starts_with(&prefix) {
                matches.push((
                    "/use auto".to_string(),
                    "switch active provider to auto".to_string(),
                ));
            }
            return matches;
        } else {
            let provider_name = parts[0];
            let model_prefix = if parts.len() > 1 {
                parts[1].to_lowercase()
            } else {
                "".to_string()
            };
            let mut models = vec![
                "gemini-3.5-flash".to_string(),
                "gemini-3.1-flash-lite".to_string(),
                "gemini-3.5-pro".to_string(),
                "gpt-4o".to_string(),
                "gpt-4o-mini".to_string(),
                "o3-mini".to_string(),
                "claude-3-5-sonnet-latest".to_string(),
                "claude-3-5-haiku-latest".to_string(),
                "deepseek-chat".to_string(),
                "deepseek-reasoner".to_string(),
            ];
            if config
                .providers
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case(provider_name))
            {
                models.insert(0, config.active_model.clone());
            }
            models.dedup();
            let mut matches = Vec::new();
            for m in models {
                if m.to_lowercase().starts_with(&model_prefix) {
                    matches.push((
                        format!("/use {} {}", provider_name, m),
                        format!("select model '{}' on provider '{}'", m, provider_name),
                    ));
                }
            }
            return matches;
        }
    }

    if query_word.starts_with("/re") {
        let sessions = crate::core::persistence::list_sessions().unwrap_or_default();
        let mut matches = Vec::new();
        let rest = if input.len() > 7 {
            &input[7..].trim_start()
        } else {
            ""
        };
        let mut count = 0;
        for s in sessions.iter() {
            if s.id.starts_with(rest) {
                matches.push((
                    format!("/resume {}", s.id),
                    format!(
                        "resume session from {}",
                        s.modified_at.format("%Y-%m-%d %H:%M")
                    ),
                ));
                count += 1;
                if count >= 5 {
                    break;
                }
            }
        }
        if !matches.is_empty() {
            return matches;
        }
    }

    commands
        .into_iter()
        .filter(|(cmd, _)| {
            let cmd_clean = cmd.split_whitespace().next().unwrap_or("");
            query_word == "/"
                || cmd_clean.starts_with(query_word)
                || query_word.starts_with(cmd_clean)
        })
        .collect()
}

fn render_suggestions_inline(
    input: &str,
    selected_index: Option<usize>,
    prev_suggestions_printed: &mut bool,
    prompt_width: usize,
    config: &config::AppConfig,
) -> io::Result<()> {
    // Clear from cursor position to end of screen (clears old suggestions)
    print!("\x1b[J");
    *prev_suggestions_printed = false;

    if !input.starts_with('/') {
        return Ok(());
    }

    let matches = get_matching_commands_inline(input, config);
    if matches.is_empty() {
        return Ok(());
    }

    // Print suggestions
    print!("\r\n");
    print!(
        "  {} {}\r\n",
        style("SUGGESTED HELIX SYSTEM COMMANDS").cyan().bold(),
        style("(use ↑/↓ keys to scroll, Enter/Tab to select)").dimmed()
    );
    print!("  {}\r\n", style("─".repeat(50)).dimmed());
    for (i, (cmd, desc)) in matches.iter().enumerate() {
        if Some(i) == selected_index {
            print!(
                "  {} {:22}  {}  {}\r\n",
                style("▶").green().bold(),
                style(cmd).green().bold(),
                style("—").green(),
                style(desc).green().bold()
            );
        } else {
            print!(
                "    {:22}  {}  {}\r\n",
                style(cmd).cyan(),
                style("—").color256(240),
                style(desc).white()
            );
        }
    }

    // Move cursor back up to the input line
    let lines_to_move_up = 3 + matches.len();
    print!("\x1b[{}A", lines_to_move_up);
    // Carriage return to column 0
    print!("\r");
    // Move cursor right to prompt + input length
    let col = prompt_width + input.len();
    if col > 0 {
        print!("\x1b[{}C", col);
    }
    io::stdout().flush()?;
    *prev_suggestions_printed = true;

    Ok(())
}

struct RawModeGuard;
impl RawModeGuard {
    fn new() -> Self {
        let _ = crossterm::terminal::enable_raw_mode();
        Self
    }
}
impl Default for RawModeGuard {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

pub async fn run_repl(mut app_config: config::AppConfig, resume_id: Option<String>) -> Result<()> {
    // Banner is printed after engine is built so we can pass session/memory/SONA info
    let start_time = chrono::Local::now();

    let data_dir = config::get_data_dir()?;
    let skill_reg = skills::SkillRegistry::new(data_dir.join("skills"))?;
    let registered_skills = skill_reg.list_skills();
    let session = persistence::Session::new(resume_id.as_deref())?;
    let sandbox = sandbox::SharedSandbox::new(app_config.sandbox_mode);
    let mut tools = build_tool_registry(sandbox.clone(), data_dir.join("skills"));

    // Initialize MCP tools without automatically registering them into the active tool registry
    let (mut _mcp_registry, mcp_tools) = init_mcp_tools().await?;

    let mcp_config_path = std::path::Path::new("mcp_config.json");
    let user_mcp_config_path = config::get_config_dir()?.join("mcp_config.json");
    let active_mcp_path = if mcp_config_path.exists() {
        mcp_config_path.to_path_buf()
    } else {
        user_mcp_config_path
    };
    let mut last_mcp_modified = active_mcp_path
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok());

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
    let mut registered_mcps = Vec::new();
    for t in mcp_tools {
        let name = t.name.clone();
        registered_mcps.push(name.clone());
        all_mcp_map.insert(
            name,
            std::sync::Arc::new(t) as std::sync::Arc<dyn crate::tools::Tool>,
        );
    }
    registered_mcps.sort();

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
        Use the tools provided to complete the user's goal. \
        When you have a final answer, respond conversationally without calling any tools. \
        CRITICAL FORMATTING RULES FOR TERMINAL ENVIRONMENT:\n\
        1. DO NOT USE MARKDOWN TABLES (e.g. '| Header | Header |') because they get severely mangled and wrapped when displayed inside the terminal's narrow fixed-width box layout (typically 80-110 characters). Instead, represent tabular data using nested bullet points, bold key-value listings, or record blocks (e.g. '◆ Record 1:\n  * Key: Value').\n\
        2. Keep horizontal lines and separators short. DO NOT output long horizontal line dashes like '------------------------------' or ASCII art. Keep horizontal dividers short, e.g., '---'.\n\
        3. Prioritize concise, structured lists and paragraphs so the text reads beautifully on a terminal.\n\
        4. When you need up-to-date facts, current news, search results, or information beyond your training cutoff (e.g. recent events, sports scores, or future events like the 2026 World Cup), you MUST use the 'web_search' tool FIRST. Do not guess, make up, or hallucinate details. Use 'web_search' to find search results, and then use 'web_fetch' on specific result URLs if you need to read the full page content.";

    let system_prompt = base_system.to_string();

    // Shared HTTP client for model info lookups (separate from the inference client)
    let lookup_client = model_registry_build_lookup_client();

    // Dynamically resolve context window from provider APIs / OpenRouter catalogue
    let context = build_context(
        &app_config,
        base_system,
        &tools.descriptors(),
        &lookup_client,
    )
    .await;
    let model = build_model(&app_config);

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

    if let Some(r_id) = &resume_id {
        match persistence::list_sessions() {
            Ok(sessions) => {
                if let Some(meta) = sessions.into_iter().find(|s| &s.id == r_id) {
                    match persistence::Session::load_messages(&meta.path) {
                        Ok(msgs) => {
                            engine.global_messages = msgs;
                            println!(
                                "{}",
                                style(format!(
                                    "✔ Resumed session '{}' ({} messages loaded).",
                                    r_id,
                                    engine.global_messages.len()
                                ))
                                .green()
                            );
                            if !engine.global_messages.is_empty() {
                                println!();
                                for msg in &engine.global_messages {
                                    let role = msg["role"].as_str().unwrap_or("user");
                                    let content = msg["content"].as_str().unwrap_or("");
                                    print_message(role, content);
                                    println!();
                                }
                            }
                        }
                        Err(e) => println!(
                            "{}",
                            style(format!("Warning: failed to load session messages: {}", e))
                                .yellow()
                        ),
                    }
                } else {
                    println!(
                        "{}",
                        style(format!("Warning: session '{}' not found in history.", r_id))
                            .yellow()
                    );
                }
            }
            Err(e) => println!(
                "{}",
                style(format!("Warning: failed to list sessions: {}", e)).yellow()
            ),
        }
    }
    // Wire up metrics using the session ID
    let metrics_collector = metrics::MetricsCollector::new(&engine.session.id);
    engine = engine.with_metrics(metrics_collector);

    // Print combined startup banner with all session details in a single card
    let memory_size = engine.memory.as_ref().map(|m| m.size()).unwrap_or(0);
    let patterns_count = engine
        .sona
        .as_ref()
        .map(|s| s.stats().patterns_stored)
        .unwrap_or(0);
    print_banner(
        &app_config,
        &engine.session.id,
        memory_size,
        patterns_count,
        engine.context.budget.model_window,
        &registered_skills,
        &registered_mcps,
    );

    // Enable bracketed paste mode
    print!("\x1b[?2004h");
    let _ = io::stdout().flush();

    loop {
        let msg_tokens: usize = engine
            .global_messages
            .iter()
            .map(context::TokenEstimator::estimate_message)
            .sum();
        let budget = &engine.context.budget;
        let total_used = msg_tokens + budget.system_prompt_tokens + budget.tool_descriptor_tokens;
        let pct = (total_used as f64 / budget.model_window as f64) * 100.0;

        let pct_color = if pct > 85.0 {
            style(format!("{:.1}%", pct)).red().bold()
        } else if pct > 70.0 {
            style(format!("{:.1}%", pct)).yellow().bold()
        } else {
            style(format!("{:.1}%", pct)).color256(150)
        };

        let mut input = String::new();
        let mut prev_suggestions_printed = false;
        let mut selected_index: Option<usize> = None;
        let prompt_width = 18 + format!("{:.1}%", pct).len();

        print!(
            "\n{} {} {} {} ",
            style("◆").cyan(),
            style("helix").white().bold(),
            style(format!("(ctx: {})", pct_color)).dimmed(),
            style("›").blue().bold()
        );
        io::stdout().flush()?;

        {
            let _raw_guard = RawModeGuard::new();
            print!("\x1b[?2004h");
            let _ = io::stdout().flush();

            loop {
                let event = match tokio::task::spawn_blocking(crossterm::event::read).await {
                    Ok(Ok(ev)) => ev,
                    _ => {
                        generate_and_save_reflection(&mut engine).await;
                        exit_gracefully(&engine, start_time);
                    }
                };

                match event {
                    crossterm::event::Event::Key(key_event) => {
                        if key_event
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL)
                        {
                            match key_event.code {
                                crossterm::event::KeyCode::Char('c') => {
                                    print!("\r\n");
                                    if prev_suggestions_printed {
                                        print!("\x1b[J");
                                        prev_suggestions_printed = false;
                                    }
                                    print!("\x1b[?2004l");
                                    let _ = io::stdout().flush();

                                    let _ = crossterm::terminal::disable_raw_mode();
                                    let confirm = dialoguer::Confirm::with_theme(
                                        &dialoguer::theme::ColorfulTheme::default(),
                                    )
                                    .with_prompt("Do you want to exit?")
                                    .default(false)
                                    .show_default(true)
                                    .interact_opt();
                                    let _ = crossterm::terminal::enable_raw_mode();

                                    match confirm {
                                        Ok(Some(true)) => {
                                            generate_and_save_reflection(&mut engine).await;
                                            exit_gracefully(&engine, start_time);
                                        }
                                        _ => {
                                            print!(
                                                "{}\r\n",
                                                style("Exiting cancelled. Returning to chat.")
                                                    .dimmed()
                                            );
                                            print!("\x1b[?2004h");
                                            let _ = io::stdout().flush();
                                            print!(
                                                "\r\x1b[K{} {} {} {} {}",
                                                style("◆").cyan(),
                                                style("helix").white().bold(),
                                                style(format!("(ctx: {})", pct_color)).dimmed(),
                                                style("›").blue().bold(),
                                                input
                                            );
                                            io::stdout().flush()?;
                                            render_suggestions_inline(
                                                &input,
                                                selected_index,
                                                &mut prev_suggestions_printed,
                                                prompt_width,
                                                &app_config,
                                            )?;
                                        }
                                    }
                                    continue;
                                }
                                crossterm::event::KeyCode::Char('d') => {
                                    if input.is_empty() {
                                        print!("\r\n");
                                        generate_and_save_reflection(&mut engine).await;
                                        exit_gracefully(&engine, start_time);
                                    }
                                    continue;
                                }
                                _ => {}
                            }
                        } else {
                            match key_event.code {
                                crossterm::event::KeyCode::Char(c) => {
                                    selected_index = None;
                                    input.push(c);
                                    print!("{}", c);
                                    io::stdout().flush()?;
                                    render_suggestions_inline(
                                        &input,
                                        selected_index,
                                        &mut prev_suggestions_printed,
                                        prompt_width,
                                        &app_config,
                                    )?;
                                }
                                crossterm::event::KeyCode::Backspace => {
                                    selected_index = None;
                                    if !input.is_empty() {
                                        input.pop();
                                        print!("\x1b[1D \x1b[1D");
                                        io::stdout().flush()?;
                                        render_suggestions_inline(
                                            &input,
                                            selected_index,
                                            &mut prev_suggestions_printed,
                                            prompt_width,
                                            &app_config,
                                        )?;
                                    }
                                }
                                crossterm::event::KeyCode::Down => {
                                    if prev_suggestions_printed {
                                        let matches =
                                            get_matching_commands_inline(&input, &app_config);
                                        if !matches.is_empty() {
                                            let next_idx = match selected_index {
                                                Some(idx) => (idx + 1) % matches.len(),
                                                None => 0,
                                            };
                                            selected_index = Some(next_idx);
                                            render_suggestions_inline(
                                                &input,
                                                selected_index,
                                                &mut prev_suggestions_printed,
                                                prompt_width,
                                                &app_config,
                                            )?;
                                        }
                                    }
                                }
                                crossterm::event::KeyCode::Up => {
                                    if prev_suggestions_printed {
                                        let matches =
                                            get_matching_commands_inline(&input, &app_config);
                                        if !matches.is_empty() {
                                            let next_idx = match selected_index {
                                                Some(idx) => {
                                                    if idx == 0 {
                                                        matches.len() - 1
                                                    } else {
                                                        idx - 1
                                                    }
                                                }
                                                None => matches.len() - 1,
                                            };
                                            selected_index = Some(next_idx);
                                            render_suggestions_inline(
                                                &input,
                                                selected_index,
                                                &mut prev_suggestions_printed,
                                                prompt_width,
                                                &app_config,
                                            )?;
                                        }
                                    }
                                }
                                crossterm::event::KeyCode::Tab => {
                                    if input.starts_with('/') {
                                        let matches =
                                            get_matching_commands_inline(&input, &app_config);
                                        if !matches.is_empty() {
                                            let idx = selected_index.unwrap_or(0);
                                            if idx < matches.len() {
                                                let first_match = &matches[idx].0;
                                                for _ in 0..input.len() {
                                                    print!("\x1b[1D \x1b[1D");
                                                }
                                                input = first_match.to_string();
                                                if input == "/resume"
                                                    || input == "/use"
                                                    || input == "/memory"
                                                    || input == "/thinking"
                                                {
                                                    input.push(' ');
                                                }
                                                selected_index = None;
                                                print!("{}", input);
                                                io::stdout().flush()?;
                                                render_suggestions_inline(
                                                    &input,
                                                    selected_index,
                                                    &mut prev_suggestions_printed,
                                                    prompt_width,
                                                    &app_config,
                                                )?;
                                            }
                                        }
                                    }
                                }
                                crossterm::event::KeyCode::Enter => {
                                    if prev_suggestions_printed {
                                        if let Some(idx) = selected_index {
                                            let matches =
                                                get_matching_commands_inline(&input, &app_config);
                                            if idx < matches.len() {
                                                let chosen = &matches[idx].0;
                                                for _ in 0..input.len() {
                                                    print!("\x1b[1D \x1b[1D");
                                                }
                                                input = chosen.to_string();
                                                print!("{}", input);
                                                io::stdout().flush()?;
                                            }
                                        }
                                        print!("\x1b[J");
                                        io::stdout().flush()?;
                                    }
                                    print!("\r\n");
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                    crossterm::event::Event::Paste(text) => {
                        input.push_str(&text);
                        print!("{}", text);
                        io::stdout().flush()?;
                        selected_index = None;
                        render_suggestions_inline(
                            &input,
                            selected_index,
                            &mut prev_suggestions_printed,
                            prompt_width,
                            &app_config,
                        )?;
                    }
                    _ => {}
                }
            }
        }

        print!("\x1b[?2004l");
        let _ = io::stdout().flush();

        let trimmed = input.trim();

        match trimmed {
            "exit" | "quit" | "/exit" | "/quit" => {
                generate_and_save_reflection(&mut engine).await;
                exit_gracefully(&engine, start_time);
            }
            "/help" => {
                print_help();
                continue;
            }
            "/clear" => {
                let msgs = engine.global_messages.clone();
                let session_id = engine.session.id.clone();
                let config = app_config.clone();
                let memory_dir = data_dir.clone().join("memory");
                tokio::spawn(async move {
                    let model = build_model(&config);
                    if let Ok(memory_engine) = memory::HelixMemoryEngine::new(&memory_dir) {
                        generate_and_save_reflection_bg(model, memory_engine, msgs, session_id)
                            .await;
                    }
                });
                engine.global_messages.clear();
                println!("{}", style("✔ Context cleared.").green());
                continue;
            }
            "/forget" | "/purge" => {
                if let Some(user_idx) = engine
                    .global_messages
                    .iter()
                    .rposition(|m| m["role"].as_str() == Some("user"))
                {
                    engine.global_messages.truncate(user_idx);
                }

                let session_res = engine.session.forget_last_turn();

                let memory_res = if let Some(ref mut memory_engine) = engine.memory
                    && let Ok(workspace_dir) = std::env::current_dir()
                {
                    let workspace_str = workspace_dir.to_string_lossy().to_string();
                    memory_engine.delete_last_memory(&workspace_str)
                } else {
                    Ok(false)
                };

                let msg = match (session_res, memory_res) {
                    (Ok(true), Ok(true)) => {
                        "✔ Last turn removed from active history, session file, and semantic memory database."
                    }
                    (Ok(true), _) => "✔ Last turn removed from active history and session file.",
                    (_, Ok(true)) => {
                        "✔ Last turn removed from active history and semantic memory database."
                    }
                    _ => "✔ Last turn removed from active history.",
                };
                println!("{}", style(msg).green());
                continue;
            }
            "/optimize" | "/learn" => {
                if let Some(ref sona) = engine.sona {
                    println!(
                        "\n  {}",
                        style("SONA NEURAL ADAPTATION CONSOLIDATION").bold().cyan()
                    );
                    println!("  {}", style("─".repeat(50)).color256(240));
                    print!("  ⦿ Compressing trajectories and training projection... ");
                    std::io::stdout().flush()?;
                    let result = sona.force_learn();
                    println!("done.");
                    println!("  {}", style(&result).green());

                    // Save state after training
                    save_sona_state(&data_dir, sona);
                } else {
                    println!("{}", style("✘ Error: SONA engine is not active.").red());
                }
                continue;
            }
            "/status" => {
                let memory_size = engine.memory.as_ref().map(|m| m.size()).unwrap_or(0);
                let active_skills_list = {
                    let state = engine.dynamic_registry.lock().unwrap();
                    let mut list = state.active_skills.iter().cloned().collect::<Vec<_>>();
                    list.sort();
                    list
                };
                print_status_card(
                    &engine.session.id,
                    engine.model.model_name(),
                    memory_size,
                    engine.sona.as_deref(),
                    &active_skills_list,
                    true,
                );

                let budget = &engine.context.budget;
                let msg_tokens: usize = engine
                    .global_messages
                    .iter()
                    .map(context::TokenEstimator::estimate_message)
                    .sum();
                let system_tokens = budget.system_prompt_tokens;
                let tool_tokens = budget.tool_descriptor_tokens;
                let headroom = budget.response_headroom;
                let total_used = msg_tokens + system_tokens + tool_tokens;
                let total_capacity = budget.model_window;
                let remaining = total_capacity
                    .saturating_sub(total_used)
                    .saturating_sub(headroom);

                println!(
                    "\n  {}",
                    style("CONTEXT WINDOW BUDGET").bold().color256(111)
                );
                println!("  {}", style("─".repeat(50)).color256(240));
                println!(
                    "  {:20}: {} tokens",
                    style("Total Model Window").color256(244),
                    style(total_capacity).color256(117)
                );
                println!(
                    "  {:20}: {} tokens",
                    style("System Prompt Cost").color256(244),
                    style(system_tokens).color256(243)
                );
                println!(
                    "  {:20}: {} tokens",
                    style("Tool Definitions").color256(244),
                    style(tool_tokens).color256(243)
                );
                println!(
                    "  {:20}: {} tokens ({} messages)",
                    style("Active Chat History").color256(244),
                    style(msg_tokens).color256(117),
                    engine.global_messages.len()
                );
                println!(
                    "  {:20}: {} tokens",
                    style("Response Headroom").color256(244),
                    style(headroom).color256(243)
                );
                println!(
                    "  {:20}: {} tokens",
                    style("Remaining Headroom").color256(244),
                    if remaining > 1000 {
                        style(remaining).green().bold()
                    } else {
                        style(remaining).red().bold()
                    }
                );

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
                println!(
                    "  {:20}: [{}] {:.1}% used",
                    style("Utilization").color256(244),
                    bar,
                    pct
                );

                let omitted_turns = count_omitted_turns(&engine.global_messages);
                let compaction_events = if let Some(ref m) = engine.metrics {
                    m.turns().iter().filter(|t| t.compaction_fired).count()
                } else {
                    0
                };

                println!(
                    "\n  {}",
                    style("CONTEXT COMPACTION SUMMARY").bold().color256(111)
                );
                println!("  {}", style("─".repeat(50)).color256(240));
                println!(
                    "  {:20}: {} triggered",
                    style("Compaction Events").color256(244),
                    style(compaction_events).color256(117)
                );
                println!(
                    "  {:20}: {} intermediate turns discarded",
                    style("Omitted History").color256(244),
                    style(omitted_turns).color256(203)
                );

                if let Some(ref m) = engine.metrics {
                    let summary = m.summary();
                    println!(
                        "\n  {}",
                        style("OPTIMIZATION & HEALER METRICS").bold().color256(111)
                    );
                    println!("  {}", style("─".repeat(50)).color256(240));
                    println!(
                        "  {:20}: {}",
                        style("Total Session Turns").color256(244),
                        style(summary.turn_count).color256(117)
                    );
                    println!(
                        "  {:20}: {}",
                        style("Total Healer Retries").color256(244),
                        style(summary.total_healer_retries).color256(117)
                    );
                    let success_rate = (1.0 - summary.error_rate) * 100.0;
                    println!(
                        "  {:20}: {:.1}%",
                        style("Healer Success Rate").color256(244),
                        style(success_rate).color256(150)
                    );
                }
                println!();

                continue;
            }
            "/clear_memory" | "/clear_memories" => {
                println!(
                    "{}",
                    style("💡 Hint: /clear_memory is deprecated. Use `/memory --clear` instead.")
                        .yellow()
                );
                if let Some(ref mut memory_engine) = engine.memory
                    && let Ok(workspace_dir) = std::env::current_dir()
                {
                    let workspace_str = workspace_dir.to_string_lossy().to_string();
                    match memory_engine.clear_workspace(&workspace_str) {
                        Ok(()) => println!(
                            "{}",
                            style("✔ Workspace memory cleared successfully.").green()
                        ),
                        Err(e) => println!("✘ Error: {}", e),
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
                println!(
                    "{}",
                    style("💡 Hint: /memories is deprecated. Use `/memory [query]` instead.")
                        .yellow()
                );
            }

            if let Some(ref mut memory_engine) = engine.memory
                && let Ok(workspace_dir) = std::env::current_dir()
            {
                let workspace_str = workspace_dir.to_string_lossy().to_string();
                if raw_arg == "--clear" {
                    match memory_engine.clear_workspace(&workspace_str) {
                        Ok(()) => println!(
                            "{}",
                            style("✔ Workspace memory cleared successfully.").green()
                        ),
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
                            println!(
                                "  {} [{}]",
                                style(text.replace('\n', " ↵ ")).cyan(),
                                style(created_at).dim()
                            );
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
                                    let file_info = m
                                        .file_path
                                        .as_ref()
                                        .map(|p| format!(" (file: {})", p))
                                        .unwrap_or_default();
                                    println!(
                                        "  {}{} [similarity: {:.2}]",
                                        style(m.text.replace('\n', " ↵ ")).cyan(),
                                        file_info,
                                        m.score
                                    );
                                }
                            }
                        }
                        Err(e) => println!("✘ Error searching: {}", e),
                    }
                }
            }
            continue;
        }

        // ── /config ───────────────────────────────────────────
        if trimmed == "/config" {
            // Disable bracketed paste mode during setup
            print!("\x1b[?2004l");
            let _ = io::stdout().flush();

            match config::interactive_setup(Some(app_config.clone())) {
                Ok(new_config) => {
                    app_config = new_config.clone();
                    sandbox.set_mode(app_config.sandbox_mode);
                    let new_system = build_system_prompt(base_system, &skill_reg);
                    let new_context = build_context(
                        &new_config,
                        &new_system,
                        &engine.tools.descriptors(),
                        &lookup_client,
                    )
                    .await;
                    engine.update_model(build_model(&new_config), new_context);
                    println!(
                        "{}",
                        style(format!(
                            "✔ Switched to {} / {}",
                            app_config.active_provider, app_config.active_model
                        ))
                        .green()
                    );
                }
                Err(_) => {
                    println!(
                        "\n{}",
                        style("Configuration cancelled. Returning to chat.").yellow()
                    );
                }
            }

            // Re-enable bracketed paste mode
            print!("\x1b[?2004h");
            let _ = io::stdout().flush();
            continue;
        }

        // ── /providers ────────────────────────────────────────
        if trimmed == "/providers" {
            println!("\n{}", style("Saved providers:").bold());
            for p in &app_config.providers {
                let active = if p.name == app_config.active_provider {
                    " ◀ active"
                } else {
                    ""
                };
                println!("  {}{}", style(p).cyan(), style(active).green().bold());
            }
            continue;
        }

        // ── /use <name> [model] ───────────────────────────────
        if let Some(rest) = trimmed.strip_prefix("/use ") {
            let rest = rest.trim();
            let mut matched_provider = None;

            // Build candidate names including "auto", sorted by length descending
            let mut provider_names = vec!["auto".to_string()];
            provider_names.extend(app_config.providers.iter().map(|p| p.name.clone()));
            provider_names.sort_by_key(|b| std::cmp::Reverse(b.len()));

            for cand in provider_names {
                let cand_len = cand.len();
                if rest.len() >= cand_len && rest[..cand_len].eq_ignore_ascii_case(&cand) {
                    if rest.len() == cand_len {
                        matched_provider = Some((cand, None));
                        break;
                    } else if rest.as_bytes()[cand_len] == b' ' {
                        let model = rest[cand_len..].trim();
                        let model_opt = if model.is_empty() {
                            None
                        } else {
                            Some(model.to_string())
                        };
                        matched_provider = Some((cand, model_opt));
                        break;
                    }
                }
            }

            if let Some((provider_name, model_override)) = matched_provider {
                match config::switch_provider(
                    &mut app_config,
                    &provider_name,
                    model_override.as_deref(),
                ) {
                    Ok(()) => {
                        sandbox.set_mode(app_config.sandbox_mode);
                        let new_context = build_context(
                            &app_config,
                            &system_prompt,
                            &engine.tools.descriptors(),
                            &lookup_client,
                        )
                        .await;
                        engine.update_model(build_model(&app_config), new_context);
                        println!(
                            "{}",
                            style(format!(
                                "✔ Switched to {} / {}",
                                app_config.active_provider, app_config.active_model
                            ))
                            .green()
                        );
                    }
                    Err(e) => println!("{}", style(format!("✘ {}", e)).red()),
                }
            } else {
                // Treat the entire string as a model override for the active provider
                if !rest.is_empty() {
                    let active_p = app_config.active_provider.clone();
                    match config::switch_provider(&mut app_config, &active_p, Some(rest)) {
                        Ok(()) => {
                            sandbox.set_mode(app_config.sandbox_mode);
                            let new_context = build_context(
                                &app_config,
                                &system_prompt,
                                &engine.tools.descriptors(),
                                &lookup_client,
                            )
                            .await;
                            engine.update_model(build_model(&app_config), new_context);
                            println!(
                                "{}",
                                style(format!(
                                    "✔ Switched model to '{}' on active provider '{}'",
                                    app_config.active_model, app_config.active_provider
                                ))
                                .green()
                            );
                        }
                        Err(e) => println!("{}", style(format!("✘ {}", e)).red()),
                    }
                } else {
                    println!(
                        "{}",
                        style("✘ Usage: /use <provider> [model] OR /use <model>").red()
                    );
                }
            }
            continue;
        }

        // ── /sessions ─────────────────────────────────────────
        if trimmed == "/sessions" {
            match persistence::list_sessions() {
                Ok(sessions) if sessions.is_empty() => println!("No saved sessions."),
                Ok(sessions) => {
                    use comfy_table::presets::NOTHING;
                    use comfy_table::{Cell, ColumnConstraint, Table, Width};
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
                        Cell::new("SESSION ID")
                            .fg(comfy_table::Color::DarkGrey)
                            .add_attribute(comfy_table::Attribute::Bold),
                        Cell::new("EVENTS")
                            .fg(comfy_table::Color::DarkGrey)
                            .add_attribute(comfy_table::Attribute::Bold),
                        Cell::new("LAST ACTIVE")
                            .fg(comfy_table::Color::DarkGrey)
                            .add_attribute(comfy_table::Attribute::Bold),
                    ]);

                    for s in sessions.iter().take(15) {
                        table.add_row(vec![
                            Cell::new(&s.id).fg(comfy_table::Color::Cyan),
                            Cell::new(s.event_count.to_string()).fg(comfy_table::Color::White),
                            Cell::new(s.modified_at.format("%Y-%m-%d %H:%M").to_string())
                                .fg(comfy_table::Color::White),
                        ]);
                    }
                    println!("{table}");
                    println!("  Use {} to load one.\n", "/resume <id>".yellow());
                }
                Err(e) => println!("{}", style(format!("❌ {}", e)).red()),
            }
            continue;
        }

        // ── /resume [id] ──────────────────────────────────────
        if trimmed == "/resume" || trimmed.starts_with("/resume ") {
            let id = trimmed.strip_prefix("/resume").unwrap_or("").trim();
            if id.is_empty() {
                match persistence::list_sessions() {
                    Ok(sessions) => {
                        if sessions.is_empty() {
                            println!("{}", style("No saved sessions found.").yellow());
                        } else {
                            println!(
                                "\n  {}",
                                style("Recent sessions (newest first):").cyan().bold()
                            );
                            println!("  {}", style("─".repeat(50)).dimmed());
                            for s in sessions.iter().take(3) {
                                println!(
                                    "    {}  —  {} events, last active: {}",
                                    style(&s.id).cyan(),
                                    style(s.event_count).white(),
                                    style(s.modified_at.format("%Y-%m-%d %H:%M")).dimmed()
                                );
                            }
                            println!("\n  Use {} to load one.\n", "/resume <id>".yellow());
                        }
                    }
                    Err(e) => println!("{}", style(format!("✘ {}", e)).red()),
                }
                continue;
            }

            match persistence::list_sessions() {
                Ok(sessions) => {
                    if let Some(meta) = sessions.into_iter().find(|s| s.id == id) {
                        match persistence::Session::load_messages(&meta.path) {
                            Ok(msgs) => {
                                engine.global_messages = msgs;
                                engine.session.id = id.to_string();
                                engine.session.path = meta.path.clone();
                                if let Some(ref mut m) = engine.metrics {
                                    m.session_id = id.to_string();
                                }
                                println!(
                                    "{}",
                                    style(format!(
                                        "✔ Resumed '{}' ({} messages).",
                                        id,
                                        engine.global_messages.len()
                                    ))
                                    .green()
                                );
                                if !engine.global_messages.is_empty() {
                                    println!();
                                    for msg in &engine.global_messages {
                                        let role = msg["role"].as_str().unwrap_or("user");
                                        let content = msg["content"].as_str().unwrap_or("");
                                        print_message(role, content);
                                        println!();
                                    }
                                }
                            }
                            Err(e) => println!("{}", style(format!("✘ {}", e)).red()),
                        }
                    } else {
                        println!(
                            "{}",
                            style(format!(
                                "✘ Session '{}' not found. Run /sessions to list.",
                                id
                            ))
                            .red()
                        );
                    }
                }
                Err(e) => println!("{}", style(format!("✘ {}", e)).red()),
            }
            continue;
        }

        // ── /thinking [level] ──────────────────────────────────
        if trimmed == "/thinking" || trimmed.starts_with("/thinking ") {
            let arg = trimmed.strip_prefix("/thinking").unwrap_or("").trim();
            if arg.is_empty() {
                let current = app_config.thinking_level.as_deref().unwrap_or("default");
                println!(
                    "\n  {}\n  {}",
                    style("REASONING EFFORT & THINKING BUDGET").bold().cyan(),
                    style("─".repeat(50)).color256(240)
                );
                println!(
                    "  Current thinking level: {}",
                    style(current).green().bold()
                );
                println!("\n  Usage:");
                println!("    {} {}", "/thinking".yellow(), "<level>".cyan());
                println!("\n  Available Levels:");
                println!(
                    "    {:22}  {}",
                    style("low, medium, high").cyan(),
                    "qualitative reasoning effort (OpenAI o1/o3-mini)"
                );
                println!(
                    "    {:22}  {}",
                    style("<integer> (e.g. 2048)").cyan(),
                    "exact thinking token budget (Claude 3.7 / DeepSeek R1)"
                );
                println!(
                    "    {:22}  {}",
                    style("off, disabled").cyan(),
                    "turn off thinking block to save cost & latency"
                );
                println!();
            } else {
                let level = match arg.to_lowercase().as_str() {
                    "low" | "medium" | "high" | "off" | "disabled" => {
                        if arg.to_lowercase() == "off" || arg.to_lowercase() == "disabled" {
                            Some("off".to_string())
                        } else {
                            Some(arg.to_lowercase())
                        }
                    }
                    other => {
                        if other.parse::<u64>().is_ok() {
                            Some(arg.to_string())
                        } else {
                            println!("{}", style("✘ Invalid level. Allowed: low, medium, high, off, disabled, or an integer budget (e.g., 2048)").red());
                            continue;
                        }
                    }
                };

                app_config.thinking_level = level.clone();
                let _ = app_config.save();

                // Hot update the adapter config as well!
                engine.model.set_thinking_level(level.clone());

                let desc = level.as_deref().unwrap_or("default");
                println!(
                    "{}",
                    style(format!("✔ Thinking level set to: {}", desc)).green()
                );
            }
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }

        if trimmed.starts_with('/') {
            let matches = get_matching_commands_inline(trimmed, &app_config);
            if trimmed != "/" {
                println!(
                    "\n  {} {}",
                    style("❌ Unknown or incomplete command:").red(),
                    style(trimmed).yellow()
                );
            }

            println!(
                "\n  {}",
                style("SUGGESTED HELIX SYSTEM COMMANDS").cyan().bold()
            );
            println!("  {}", style("─".repeat(50)).dimmed());

            for (cmd, desc) in &matches {
                println!(
                    "    {:22}  {}  {}",
                    style(cmd).cyan(),
                    style("—").color256(240),
                    style(desc).white()
                );
            }
            println!();
            continue;
        }

        // ── Normal turn with streaming output ─────────────────
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        let content_width = get_content_width();

        // Print User Input Box — cool blue/cyan scheme (adapts to terminal theme)
        let user_border = |s: &str| s.blue().to_string();
        let user_header = |s: &str| s.blue().bold().to_string();
        let user_pipe = "│".blue().to_string();

        // Clear the user's raw input line (move cursor up 1 line, reset to column 0, and clear it)
        print!("\x1B[1A\r\x1B[K");
        io::stdout().flush().ok();

        // Print boxed user input
        let title = "You";
        let dashes_count = content_width.saturating_sub(title.len());
        print!("  ");
        print!("{}", user_border("╭── "));
        print!("{}", user_header(title));
        println!(
            "{}",
            user_border(&format!(" {}╮", "─".repeat(dashes_count)))
        );
        let wrapped_user = wrap_text(trimmed, content_width);
        for line in wrapped_user {
            let expanded_line = line.replace('\t', "    ");
            let padded = format!("  {:width$}  ", expanded_line, width = content_width);
            println!("  {}{}{}", user_pipe, padded, user_pipe);
        }
        println!(
            "  {}",
            user_border(&format!("╰{}╯", "─".repeat(content_width + 4)))
        );

        // Spawn printer task — handles streaming tokens and interactive spinner
        let printer = tokio::spawn(async move {
            // Assistant Response — warm amber/gold scheme (visually distinct from user box)
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(80));
            let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut frame_idx = 0;
            let mut show_spinner = false;
            let mut spinner_suffix = "thinking...".to_string();
            let thinking_style_fn = |s: &str| s.dimmed().to_string();
            let spinner_style_fn = |s: &str| s.cyan().bold().to_string();

            let start_time = std::time::Instant::now();
            let mut full_response = String::new();
            let mut telemetry_messages = Vec::new();
            let mut stream_tracker = StreamTracker::new(content_width);

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
                                } else if token == "\x04" {
                                    show_spinner = false;
                                } else if token.starts_with("\x1b[S") {
                                    spinner_suffix = token.strip_prefix("\x1b[S").unwrap_or("thinking...").to_string();
                                } else if token.starts_with("\x1b[T") {
                                    let msg = token.strip_prefix("\x1b[T").unwrap_or("").to_string();
                                    if !stream_tracker.started {
                                        print!("\r\x1B[K");
                                        println!("{}", msg);
                                        io::stdout().flush().ok();
                                    } else {
                                        telemetry_messages.push(msg);
                                    }
                                } else {
                                    if show_spinner {
                                        print!("\r\x1B[K");
                                        io::stdout().flush().ok();
                                        show_spinner = false;
                                    }
                                    stream_tracker.print_token(&token);
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

            // Finish the stream tracker
            stream_tracker.finish();

            let elapsed = start_time.elapsed().as_secs_f32();
            let elapsed_str = format!(" [Elapsed: {:.2}s] ", elapsed);

            let term_height = console::Term::stdout()
                .size_checked()
                .map(|(h, _)| h as usize)
                .unwrap_or(24);
            let is_short_enough = stream_tracker.printed_lines < term_height.saturating_sub(4);

            if full_response.trim().is_empty() {
                if stream_tracker.printed_lines > 0 {
                    print!("\x1B[{}A", stream_tracker.printed_lines);
                    print!("\x1B[J");
                    io::stdout().flush().ok();
                }
            } else if stream_tracker.printed_lines > 0 && is_short_enough {
                // Erase the live-streamed text box to replace it with formatted markdown
                print!("\x1B[{}A", stream_tracker.printed_lines);
                print!("\x1B[J");
                io::stdout().flush().ok();
                print_boxed_response(content_width, &full_response, &elapsed_str);
            } else {
                // Either no lines were printed, or the response was too long and scrolled.
                // In the latter case, we do not erase (to avoid leaving orphaned lines in scrollback).
                // Instead, we just print the bottom border of the streamed box.
                let l_len = elapsed_str.len();
                let dashes = (content_width + 4).saturating_sub(l_len);
                let left_dashes = dashes / 2;
                let right_dashes = dashes - left_dashes;

                let border_color_fn = |s: &str| s.yellow().to_string();
                let header_color_fn = |s: &str| s.yellow().bold().to_string();

                let bottom = format!(
                    "  {}{}{}{}{}",
                    border_color_fn("╰"),
                    border_color_fn(&"─".repeat(left_dashes)),
                    header_color_fn(&elapsed_str),
                    border_color_fn(&"─".repeat(right_dashes)),
                    border_color_fn("╯")
                );
                println!("{}", bottom);
                std::io::stdout().flush().ok();
            }

            // Print telemetry footnotes
            for msg in telemetry_messages {
                println!("{}", msg);
            }
            io::stdout().flush().ok();
        });

        tokio::select! {
            res = engine.run_turn(&system_prompt, trimmed, Some(tx)) => {
                printer.await.ok(); // wait for printer task to finish rendering
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

        // Print live context utilization bar
        let msg_tokens: usize = engine
            .global_messages
            .iter()
            .map(context::TokenEstimator::estimate_message)
            .sum();
        let budget = &engine.context.budget;
        let total_used = msg_tokens + budget.system_prompt_tokens + budget.tool_descriptor_tokens;
        let pct = (total_used as f64 / budget.model_window as f64) * 100.0;

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

        println!(
            "  {} [{}] {}/{} tokens ({:.1}% used)\n",
            style("Context:").dimmed(),
            bar,
            total_used,
            budget.model_window,
            pct
        );

        // Flush metrics to disk after every turn so /evolve can see them immediately
        if let Some(m) = &engine.metrics
            && let Ok(state_dir) = config::get_state_dir()
        {
            let _ = m.flush_to_disk(&state_dir.join("sessions"));
        }

        // Save SONA state after every turn
        if let Some(ref sona) = engine.sona {
            save_sona_state(&data_dir, sona);
        }

        // Check if MCP configuration file was modified during the turn (Safe Reload Scheduler)
        let current_mcp_modified = active_mcp_path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok());

        if current_mcp_modified != last_mcp_modified {
            println!("\n🔄 [MCP] Detected changes in mcp_config.json. Scheduling safe reload...");
            match init_mcp_tools().await {
                Ok((new_mcp_registry, mcp_tools)) => {
                    _mcp_registry = new_mcp_registry; // Drops the old registry and kills old processes

                    let mut all_mcp_map = std::collections::HashMap::new();
                    for t in mcp_tools {
                        all_mcp_map.insert(
                            t.name.clone(),
                            std::sync::Arc::new(t) as std::sync::Arc<dyn crate::tools::Tool>,
                        );
                    }

                    // Update state
                    {
                        let mut state = engine.dynamic_registry.lock().unwrap();
                        state.all_mcp_tools = all_mcp_map;
                        // Clean up any active tools that no longer exist
                        let keys: std::collections::HashSet<String> =
                            state.all_mcp_tools.keys().cloned().collect();
                        state.active_tools.retain(|name| keys.contains(name));
                        state.changed = true;
                    }

                    last_mcp_modified = current_mcp_modified;
                    println!("✔ [MCP] Configuration reloaded and tools updated successfully!");
                }
                Err(e) => {
                    eprintln!("⚠️  [MCP] Failed to reload configuration: {}", e);
                }
            }
        }
    }

    #[allow(unreachable_code)]
    Ok(())
}

fn model_registry_build_lookup_client() -> reqwest::Client {
    crate::model::registry::build_lookup_client()
}

fn exit_gracefully(engine: &engine::Engine, start_time: chrono::DateTime<Local>) -> ! {
    let mut total_tool_calls = 0;
    let mut user_turns = 0;
    if let Some(m) = &engine.metrics {
        if let Ok(state_dir) = config::get_state_dir() {
            let sessions_dir = state_dir.join("sessions");
            let _ = m.flush_to_disk(&sessions_dir);
        }
        let summary = m.summary();
        total_tool_calls = summary.total_tool_calls;
        user_turns = summary.turn_count;
    }

    if user_turns == 0 {
        user_turns = engine
            .global_messages
            .iter()
            .filter(|msg| msg.get("role").and_then(|r| r.as_str()) == Some("user"))
            .count();
    }

    let end_time = Local::now();
    let duration = end_time.signed_duration_since(start_time);
    let duration_str = if duration.num_minutes() > 0 {
        format!(
            "{}m {}s",
            duration.num_minutes(),
            duration.num_seconds() % 60
        )
    } else {
        format!("{}s", duration.num_seconds())
    };

    // Restore terminal to cooked mode BEFORE printing so that println! newlines
    // land at column 0 correctly (raw mode requires explicit \r\n otherwise).
    let _ = crossterm::terminal::disable_raw_mode();
    print!("\x1b[?2004l");
    let _ = io::stdout().flush();

    println!();
    println!("{}", style("Resume this session with:").bold().cyan());
    println!("  helix --resume {}", engine.session.id);
    println!();
    println!("Session:        {}", engine.session.id);
    println!("Duration:       {}", duration_str);
    println!(
        "Messages:       {} ({} user, {} tool calls)",
        engine.global_messages.len(),
        user_turns,
        total_tool_calls
    );
    println!();

    std::process::exit(0);
}

fn print_boxed_response(content_width: usize, full_response: &str, elapsed_str: &str) {
    let title = "Helix";
    let border_color_fn = |s: &str| s.yellow().to_string();
    let header_color_fn = |s: &str| s.yellow().bold().to_string();
    let dashes_count = content_width.saturating_sub(title.len());
    print!("  ");
    print!("{}", border_color_fn("╭── "));
    print!("{}", header_color_fn(title));
    println!(
        "{}",
        border_color_fn(&format!(" {}╮", "─".repeat(dashes_count)))
    );

    let pipe = "│".yellow().to_string();

    // Render using termimad with default skin
    let skin = termimad::MadSkin::default();
    let fmt_text = termimad::FmtText::from_text(&skin, full_response.into(), Some(content_width));
    let rendered_str = fmt_text.to_string();

    for line in rendered_str.lines() {
        let expanded_line = line.replace('\t', "    ");
        let clean_line = console::strip_ansi_codes(&expanded_line);
        let display_width = clean_line.width();
        let pad = content_width.saturating_sub(display_width);
        println!("  {}  {}{}  {}", pipe, expanded_line, " ".repeat(pad), pipe);
    }

    // Print bottom border
    let l_len = elapsed_str.len();
    let dashes = (content_width + 4).saturating_sub(l_len);
    let left_dashes = dashes / 2;
    let right_dashes = dashes - left_dashes;
    let bottom = format!(
        "  {}{}{}{}{}",
        border_color_fn("╰"),
        border_color_fn(&"─".repeat(left_dashes)),
        header_color_fn(elapsed_str),
        border_color_fn(&"─".repeat(right_dashes)),
        border_color_fn("╯")
    );
    println!("{}", bottom);
    std::io::stdout().flush().ok();
}

struct StreamTracker {
    current_line: String,
    printed_lines: usize,
    started: bool,
    pipe: String,
}

impl StreamTracker {
    fn new(_content_width: usize) -> Self {
        Self {
            current_line: String::new(),
            printed_lines: 0,
            started: false,
            pipe: "│".yellow().to_string(),
        }
    }

    fn start(&mut self) {
        let border_color_fn = |s: &str| s.yellow().to_string();
        let header_color_fn = |s: &str| s.yellow().bold().to_string();

        let content_width = get_content_width();
        let title = "Helix";
        let dashes_count = content_width.saturating_sub(title.len());
        print!("  ");
        print!("{}", border_color_fn("╭── "));
        print!("{}", header_color_fn(title));
        println!(
            "{}",
            border_color_fn(&format!(" {}╮", "─".repeat(dashes_count)))
        );

        self.printed_lines += 1;

        print!("  {}  ", self.pipe);
        std::io::stdout().flush().ok();
        self.started = true;
    }

    fn print_token(&mut self, token: &str) {
        if !self.started {
            self.start();
        }

        let content_width = get_content_width();
        let parts: Vec<&str> = token.split('\n').collect();
        for (i, part) in parts.iter().enumerate() {
            if i > 0 {
                self.flush_current_line();
            }

            self.current_line.push_str(part);

            while self.current_line.width() > content_width {
                let mut last_space_idx = None;
                let chars: Vec<char> = self.current_line.chars().collect();
                let mut prefix_width = 0;
                let mut fit_char_count = 0;
                for (idx, &c) in chars.iter().enumerate() {
                    let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                    if prefix_width + w > content_width {
                        break;
                    }
                    prefix_width += w;
                    fit_char_count += 1;
                    if c == ' ' {
                        last_space_idx = Some(idx + 1);
                    }
                }

                let split_idx = if let Some(space_idx) = last_space_idx {
                    space_idx
                } else {
                    fit_char_count
                };

                let fit: String = chars[..split_idx].iter().collect();
                let rem: String = chars[split_idx..].iter().collect();

                let fit_clean = console::strip_ansi_codes(&fit);
                let fit_display_w = fit_clean.width();
                let fit_pad = content_width.saturating_sub(fit_display_w);
                print!("\r\x1B[K");
                println!(
                    "  {}  {}{}  {}",
                    self.pipe,
                    fit,
                    " ".repeat(fit_pad),
                    self.pipe
                );
                self.printed_lines += 1;

                self.current_line = rem;
                print!("  {}  ", self.pipe);
            }
        }

        print!("\r\x1B[K");
        print!("  {}  {}", self.pipe, self.current_line);
        std::io::stdout().flush().ok();
    }

    fn flush_current_line(&mut self) {
        let content_width = get_content_width();
        let clean = console::strip_ansi_codes(&self.current_line);
        let display_w = clean.width();
        let pad = content_width.saturating_sub(display_w);
        print!("\r\x1B[K");
        println!(
            "  {}  {}{}  {}",
            self.pipe,
            self.current_line,
            " ".repeat(pad),
            self.pipe
        );
        self.printed_lines += 1;
        self.current_line.clear();
        print!("  {}  ", self.pipe);
        std::io::stdout().flush().ok();
    }

    fn finish(&mut self) {
        if !self.started {
            return;
        }
        let content_width = get_content_width();
        let clean = console::strip_ansi_codes(&self.current_line);
        let display_w = clean.width();
        let pad = content_width.saturating_sub(display_w);
        print!("\r\x1B[K");
        println!(
            "  {}  {}{}  {}",
            self.pipe,
            self.current_line,
            " ".repeat(pad),
            self.pipe
        );
        self.printed_lines += 1;
        self.current_line.clear();
        std::io::stdout().flush().ok();
    }
}

async fn generate_and_save_reflection(engine: &mut engine::Engine) {
    if engine.global_messages.is_empty() {
        return;
    }

    let user_msg_count = engine
        .global_messages
        .iter()
        .filter(|msg| msg.get("role").and_then(|r| r.as_str()) == Some("user"))
        .count();
    if user_msg_count == 0 {
        return;
    }

    println!("\n🧠 [Memory] Generating session reflection summary...");
    let post_mortem_prompt = "You are a professional software engineer summarizing a pair programming session. \
        Generate a high-signal markdown document summarizing this coding session. \
        Be extremely concise and professional, strictly limiting the summary to under 300 words. \
        Avoid any introductory or closing chatter. \
        Use the following format:\n\
        # Session Summary: [Brief Title]\n\
        - **Core Goal**: [One sentence describing the objective]\n\
        - **Successful Implementations**: [Bullet list of verified changes, tools, or commands that worked successfully]\n\
        - **Errors & Blockers Resolved**: [Bullet list of issues encountered and how they were solved]\n\
        - **Key Context Patterns**: [Bullet list of architectural or path conventions discovered]";

    let mut reflection_messages = engine.global_messages.clone();
    reflection_messages.push(serde_json::json!({
        "role": "user",
        "content": "The session has ended. Please generate the structured session reflection summary now based on the programming session history."
    }));

    match engine
        .model
        .call(post_mortem_prompt, &reflection_messages, vec![], None)
        .await
    {
        Ok(model_response) => {
            if let crate::model::ModelResponse::EndTurn(summary_text) = model_response {
                let summary_trimmed = summary_text.trim();
                if !summary_trimmed.is_empty() && config::get_state_dir().is_ok() {
                    let state_dir = config::get_state_dir().unwrap();
                    let memory_sessions_dir = state_dir.join("memory").join("sessions");
                    if let Err(e) = std::fs::create_dir_all(&memory_sessions_dir) {
                        eprintln!("⚠️  Failed to create memory directory: {}", e);
                        return;
                    }
                    let summary_path =
                        memory_sessions_dir.join(format!("{}.md", engine.session.id));
                    if let Err(e) = std::fs::write(&summary_path, &summary_text) {
                        eprintln!("⚠️  Failed to write reflection summary: {}", e);
                    } else {
                        println!(
                            "✔ [Memory] Reflection summary saved to: {}",
                            summary_path.display()
                        );
                        if let Some(ref mut memory_engine) = engine.memory {
                            if let Err(e) = memory_engine.insert(&summary_text, None, "global") {
                                eprintln!("⚠️  Failed to index reflective memory: {}", e);
                            } else {
                                println!(
                                    "✔ [Memory] Reflective summary indexed successfully in local semantic memory!"
                                );
                            }
                        }
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("⚠️  [Memory] Failed to generate reflection: {}", e);
        }
    }
}

async fn generate_and_save_reflection_bg(
    model: Box<dyn crate::model::ModelAdapter>,
    mut memory_engine: memory::HelixMemoryEngine,
    global_messages: Vec<serde_json::Value>,
    session_id: String,
) {
    if global_messages.is_empty() {
        return;
    }

    let user_msg_count = global_messages
        .iter()
        .filter(|msg| msg.get("role").and_then(|r| r.as_str()) == Some("user"))
        .count();
    if user_msg_count == 0 {
        return;
    }

    let post_mortem_prompt = "You are a professional software engineer summarizing a pair programming session. \
        Generate a high-signal markdown document summarizing this coding session. \
        Be extremely concise and professional, strictly limiting the summary to under 300 words. \
        Avoid any introductory or closing chatter. \
        Use the following format:\n\
        # Session Summary: [Brief Title]\n\
        - **Core Goal**: [One sentence describing the objective]\n\
        - **Successful Implementations**: [Bullet list of verified changes, tools, or commands that worked successfully]\n\
        - **Errors & Blockers Resolved**: [Bullet list of issues encountered and how they were solved]\n\
        - **Key Context Patterns**: [Bullet list of architectural or path conventions discovered]";

    let mut reflection_messages = global_messages.clone();
    reflection_messages.push(serde_json::json!({
        "role": "user",
        "content": "The session has ended. Please generate the structured session reflection summary now based on the programming session history."
    }));

    match model
        .call(post_mortem_prompt, &reflection_messages, vec![], None)
        .await
    {
        Ok(model_response) => {
            if let crate::model::ModelResponse::EndTurn(summary_text) = model_response {
                let summary_trimmed = summary_text.trim();
                if !summary_trimmed.is_empty() && config::get_state_dir().is_ok() {
                    let state_dir = config::get_state_dir().unwrap();
                    let memory_sessions_dir = state_dir.join("memory").join("sessions");
                    if let Err(e) = std::fs::create_dir_all(&memory_sessions_dir) {
                        eprintln!("⚠️  Failed to create memory directory in background: {}", e);
                        return;
                    }
                    let summary_path = memory_sessions_dir.join(format!("{}.md", session_id));
                    if let Err(e) = std::fs::write(&summary_path, &summary_text) {
                        eprintln!(
                            "⚠️  Failed to write reflection summary in background: {}",
                            e
                        );
                    } else if let Err(e) = memory_engine.insert(&summary_text, None, "global") {
                        eprintln!("⚠️  Failed to index reflective memory in background: {}", e);
                    }
                }
            }
        }
        Err(e) => {
            eprintln!(
                "⚠️  [Memory] Failed to generate reflection in background: {}",
                e
            );
        }
    }
}
