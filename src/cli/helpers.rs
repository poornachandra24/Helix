use crate::config;
use crate::core::context;
use crate::memory::skills;
use crate::model;
use crate::model::registry as model_registry;
use crate::tools;
use crate::tools::builtins;
use crate::tools::mcp;
use crate::tools::sandbox;
use anyhow::Result;

pub fn print_banner(
    config: &config::AppConfig,
    session_id: &str,
    memory_size: usize,
    patterns_count: usize,
    context_limit: usize,
    active_skills: &[String],
) {
    use comfy_table::modifiers::UTF8_ROUND_CORNERS;
    use comfy_table::presets::UTF8_BORDERS_ONLY;
    use comfy_table::{Attribute, Cell, CellAlignment, Color, ColumnConstraint, Table, Width};
    use owo_colors::OwoColorize;

    let logo_lines = [
        "   ██╗  ██╗███████╗██╗     ██╗██╗  ██╗",
        "   ██║  ██║██╔════╝██║     ██║╚██╗██╔╝",
        "   ███████║█████╗  ██║     ██║ ╚███╔╝ ",
        "   ██╔══██║██╔══╝  ██║     ██║ ██╔██╗ ",
        "   ██║  ██║███████╗███████╗██║██╔╝ ██╗",
        "   ╚═╝  ╚═╝╚══════╝╚══════╝╚═╝╚═╝  ╚═╝",
    ];

    println!();
    // Print logo side-by-side with retrieval tuning indicators
    let neural_status = if patterns_count > 0 {
        format!("{} patterns adapted", patterns_count)
            .green()
            .bold()
            .to_string()
    } else {
        "initialized".dimmed().to_string()
    };

    let status_lines = [
        "".to_string(),
        format!("   ⚙️ {}", "RETRIEVAL TUNING:".bold().white()),
        format!("    └─ Query Alignment:  {}", neural_status),
        "".to_string(),
        "".to_string(),
        "".to_string(),
    ];

    for (line, status) in logo_lines.iter().zip(status_lines.iter()) {
        println!("{}{}", line.cyan().bold(), status);
    }
    println!();

    let provider = &config.active_provider;
    let model = &config.active_model;
    let ws = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "Unknown".to_string());

    let mut formatted_limit = String::new();
    let limit_str = context_limit.to_string();
    for (i, c) in limit_str.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            formatted_limit.push(',');
        }
        formatted_limit.push(c);
    }
    let formatted_limit: String = formatted_limit.chars().rev().collect();
    let context_str = format!("{} tokens", formatted_limit);

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.apply_modifier(UTF8_ROUND_CORNERS);
    table.set_width(58);
    table.set_constraints(vec![
        ColumnConstraint::Absolute(Width::Fixed(15)),
        ColumnConstraint::Absolute(Width::Fixed(39)),
    ]);

    // Row 1: Header
    table.add_row(vec![
        Cell::new("● HELIX CORE ENGINE")
            .fg(Color::Cyan)
            .add_attribute(Attribute::Bold),
        Cell::new("STATUS: ONLINE")
            .fg(Color::Green)
            .add_attribute(Attribute::Bold)
            .set_alignment(CellAlignment::Right),
    ]);

    // Row 2: Separator
    table.add_row(vec![
        Cell::new("───────────────").fg(Color::DarkGrey),
        Cell::new("───────────────────────────────────────").fg(Color::DarkGrey),
    ]);

    // Row 3, 4, 5: Provider, Model, Context Limit, Workspace
    table.add_row(vec![
        Cell::new("PROVIDER")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(provider).fg(Color::White),
    ]);
    table.add_row(vec![
        Cell::new("MODEL")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(model).fg(Color::White),
    ]);
    table.add_row(vec![
        Cell::new("CONTEXT LIMIT")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(&context_str)
            .fg(Color::Cyan)
            .add_attribute(Attribute::Bold),
    ]);
    table.add_row(vec![
        Cell::new("WORKSPACE")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(&ws).fg(Color::White),
    ]);

    // Row 6: Separator
    table.add_row(vec![
        Cell::new("───────────────").fg(Color::DarkGrey),
        Cell::new("───────────────────────────────────────").fg(Color::DarkGrey),
    ]);

    // Row 7, 8, 9: Session ID, Memory Size, Sona State
    table.add_row(vec![
        Cell::new("SESSION ID")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(session_id).fg(Color::White),
    ]);
    let mem_str = format!("{} documents", memory_size);
    table.add_row(vec![
        Cell::new("MEMORY SIZE")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(&mem_str).fg(Color::White),
    ]);
    let sona_str = format!("{} patterns learned", patterns_count);
    table.add_row(vec![
        Cell::new("SONA STATE")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(&sona_str).fg(Color::White),
    ]);
    let skills_str = if active_skills.is_empty() {
        "none loaded".to_string()
    } else {
        format!(
            "{} loaded ({})",
            active_skills.len(),
            active_skills.join(", ")
        )
    };
    table.add_row(vec![
        Cell::new("ACTIVE SKILLS")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(&skills_str).fg(Color::White),
    ]);

    // Row 10: Separator
    table.add_row(vec![
        Cell::new("───────────────").fg(Color::DarkGrey),
        Cell::new("───────────────────────────────────────").fg(Color::DarkGrey),
    ]);

    // Row 11: Help
    table.add_row(vec![
        Cell::new("💡").set_alignment(CellAlignment::Center),
        Cell::new("Type /help to list system commands.").fg(Color::Cyan),
    ]);

    println!("{table}");
}

pub fn build_system_prompt(base: &str, skill_reg: &skills::SkillRegistry) -> String {
    let skills_suffix = skill_reg.load_skills_prompt().unwrap_or_default();
    format!("{}{}", base, skills_suffix)
}

pub fn build_tool_registry(
    sandbox: sandbox::SharedSandbox,
    skills_dir: std::path::PathBuf,
) -> tools::ToolRegistry {
    let mut registry = tools::ToolRegistry::new();
    registry.register(builtins::BashTool::new(sandbox.clone()));
    registry.register(builtins::ReadFileTool::new(sandbox.clone()));
    registry.register(builtins::WriteFileTool::new(sandbox.clone()));
    registry.register(builtins::ListDirTool::new(sandbox.clone()));
    registry.register(builtins::WasmExecuteTool::new(sandbox.clone()));
    registry.register(builtins::WebFetchTool::new());
    registry.register(builtins::AddSkillTool::new(skills_dir));
    registry
}

pub async fn init_mcp_tools(tools: &mut tools::ToolRegistry) -> Result<mcp::McpRegistry> {
    let mut mcp_registry = mcp::McpRegistry::new();
    let mcp_config_path = std::path::Path::new("mcp_config.json");
    let user_mcp_config_path = config::get_config_dir()?.join("mcp_config.json");
    let active_mcp_path = if mcp_config_path.exists() {
        mcp_config_path
    } else {
        &user_mcp_config_path
    };

    if active_mcp_path.exists() {
        match mcp_registry.load_and_initialize(active_mcp_path).await {
            Ok(mcp_tools) => {
                for tool in mcp_tools {
                    tools.register(tool);
                }
            }
            Err(e) => {
                eprintln!("⚠️  [MCP] Error loading MCP configuration: {}", e);
            }
        }
    }
    Ok(mcp_registry)
}

pub fn build_model(config: &config::AppConfig) -> Box<dyn model::ModelAdapter> {
    Box::new(model::OpenAiCompatibleAdapter::new(config.clone()))
}

pub async fn build_context(
    config: &config::AppConfig,
    system_prompt: &str,
    tools: &[tools::ToolDescriptor],
    lookup_client: &reqwest::Client,
) -> context::ContextManager {
    let model_window = model_registry::resolve_context_window(config, lookup_client).await;

    let budget = context::ContextBudget::new(
        model_window,
        system_prompt,
        tools,
        config.effective_headroom(),
    );

    tracing::info!(
        model_window,
        system_tokens = budget.system_prompt_tokens,
        tool_tokens = budget.tool_descriptor_tokens,
        available = budget.available_for_messages(),
        "Context budget initialised"
    );

    context::ContextManager::new(budget)
}

pub fn count_omitted_turns(messages: &[serde_json::Value]) -> usize {
    let mut total = 0;
    for msg in messages {
        if let Some(content) = msg.get("content").and_then(|v| v.as_str())
            && content.contains("intermediate turns were omitted")
            && let Some(part) = content.split("intermediate turns").next()
            && let Some(num_str) = part.split(':').next_back()
            && let Some(num_only) = num_str.split_whitespace().next()
            && let Ok(num) = num_only.parse::<usize>()
        {
            total += num;
        }
    }
    total
}

pub fn load_sona_state(data_dir: &std::path::Path, sona: &ruvector_sona::SonaEngine) {
    let state_path = data_dir.join("sona_state.json");
    if state_path.exists()
        && let Ok(json) = std::fs::read_to_string(&state_path)
    {
        let _ = sona.coordinator().load_state(&json);
    }
    let weights_path = data_dir.join("sona_weights.json");
    if weights_path.exists()
        && let Ok(json) = std::fs::read_to_string(&weights_path)
        && let Ok((down, up)) = serde_json::from_str::<(Vec<f32>, Vec<f32>)>(&json)
    {
        let _ = sona.coordinator().restore_micro_lora_weights(down, up);
    }
}

pub fn save_sona_state(data_dir: &std::path::Path, sona: &ruvector_sona::SonaEngine) {
    let state_path = data_dir.join("sona_state.json");
    let json_state = sona.coordinator().serialize_state();
    let _ = std::fs::write(state_path, json_state);

    let weights_path = data_dir.join("sona_weights.json");
    let weights = sona.coordinator().get_micro_lora_weights();
    if let Ok(json_weights) = serde_json::to_string(&weights) {
        let _ = std::fs::write(weights_path, json_weights);
    }
}

pub fn print_status_card(
    session_id: &str,
    model_name: &str,
    memory_size: usize,
    sona: Option<&ruvector_sona::SonaEngine>,
    active_skills: &[String],
    full_diagnostics: bool,
) {
    use comfy_table::modifiers::UTF8_ROUND_CORNERS;
    use comfy_table::presets::UTF8_BORDERS_ONLY;
    use comfy_table::{Attribute, Cell, Color, ColumnConstraint, Table, Width};

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.apply_modifier(UTF8_ROUND_CORNERS);
    table.set_width(62);
    table.set_constraints(vec![
        ColumnConstraint::Absolute(Width::Fixed(18)),
        ColumnConstraint::Absolute(Width::Fixed(40)),
    ]);

    // Row 1: Header
    table.add_row(vec![
        Cell::new("HELIX SYSTEM INTEGRATION STATUS")
            .fg(Color::Cyan)
            .add_attribute(Attribute::Bold),
        Cell::new(""),
    ]);

    // Row 2: Separator
    table.add_row(vec![
        Cell::new("──────────────────").fg(Color::DarkGrey),
        Cell::new("────────────────────────────────────────").fg(Color::DarkGrey),
    ]);

    // Row 3-6: Basic fields
    table.add_row(vec![
        Cell::new("Session ID")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(session_id).fg(Color::White),
    ]);
    table.add_row(vec![
        Cell::new("Active Model")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(model_name).fg(Color::White),
    ]);
    let mem_str = format!("{} documents", memory_size);
    table.add_row(vec![
        Cell::new("Memory Size")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(&mem_str).fg(Color::White),
    ]);
    let patterns_count = sona
        .as_ref()
        .map(|s| s.stats().patterns_stored)
        .unwrap_or(0);
    let sona_str = format!("{} patterns learned", patterns_count);
    table.add_row(vec![
        Cell::new("SONA State")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(&sona_str).fg(Color::White),
    ]);
    let skills_str = if active_skills.is_empty() {
        "none loaded".to_string()
    } else {
        format!(
            "{} loaded ({})",
            active_skills.len(),
            active_skills.join(", ")
        )
    };
    table.add_row(vec![
        Cell::new("Active Skills")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(&skills_str).fg(Color::White),
    ]);

    if full_diagnostics {
        // Row 7: Separator
        table.add_row(vec![
            Cell::new("──────────────────").fg(Color::DarkGrey),
            Cell::new("────────────────────────────────────────").fg(Color::DarkGrey),
        ]);

        // Row 8: Section Header
        table.add_row(vec![
            Cell::new("OPTIMIZATION & RETRIEVAL TUNING DIAGNOSTICS")
                .fg(Color::Cyan)
                .add_attribute(Attribute::Bold),
            Cell::new(""),
        ]);

        // Row 9: Separator
        table.add_row(vec![
            Cell::new("──────────────────").fg(Color::DarkGrey),
            Cell::new("────────────────────────────────────────").fg(Color::DarkGrey),
        ]);

        let ewc_tasks = sona.as_ref().map(|s| s.stats().ewc_tasks).unwrap_or(0);
        let buf_rate = sona
            .as_ref()
            .map(|s| s.stats().buffer_success_rate * 100.0)
            .unwrap_or(0.0);
        let consolidation = format!("{} tasks, {:.1}% buffer success", ewc_tasks, buf_rate);
        table.add_row(vec![
            Cell::new("Consolidation")
                .fg(Color::DarkGrey)
                .add_attribute(Attribute::Bold),
            Cell::new(&consolidation).fg(Color::White),
        ]);

        let lambda = sona.as_ref().map(|s| s.config().ewc_lambda).unwrap_or(0.0);
        let lambda_str = format!("{:.1}", lambda);
        table.add_row(vec![
            Cell::new("EWC Lambda")
                .fg(Color::DarkGrey)
                .add_attribute(Attribute::Bold),
            Cell::new(&lambda_str).fg(Color::White),
        ]);

        let micro_rank = sona
            .as_ref()
            .map(|s| s.config().micro_lora_rank)
            .unwrap_or(0);
        let micro_dim = sona.as_ref().map(|s| s.config().hidden_dim).unwrap_or(0);
        let micro_params = micro_dim * micro_rank * 2;
        let micro_status = format!("Rank {}, {} active params", micro_rank, micro_params);
        table.add_row(vec![
            Cell::new("Micro-LoRA")
                .fg(Color::DarkGrey)
                .add_attribute(Attribute::Bold),
            Cell::new(&micro_status).fg(Color::White),
        ]);
    }

    println!("{table}");
}

pub fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    let options = textwrap::Options::new(max_width)
        .break_words(true)
        .word_separator(textwrap::WordSeparator::AsciiSpace);
    text.lines()
        .flat_map(|line| {
            if line.is_empty() {
                // Preserve blank lines as empty entries
                vec![String::new()]
            } else {
                textwrap::wrap(line, &options)
                    .into_iter()
                    .map(|s| s.into_owned())
                    .collect()
            }
        })
        .collect()
}

/// Redacts common secret patterns (API keys, tokens, passwords) in a string
/// so they are never shown in plain text in the terminal UI.
/// Uses no external crates — pure stdlib string scanning.
fn redact_secrets(input: &str) -> String {
    // Keywords that signal the next token is a secret value.
    const SECRET_KEYWORDS: &[&str] = &[
        "key", "token", "secret", "password", "api", "auth", "bearer", "KEY", "TOKEN", "SECRET",
        "PASSWORD", "API", "AUTH", "BEARER",
    ];

    let mut out = input.to_string();

    // Pass 1 – env-var / shell style:  SOME_KEY=VALUE  or  KEY="VALUE"
    // Walk through the string looking for `=` preceded by a secret keyword.
    let mut result = String::with_capacity(out.len());
    let mut i = 0;
    let chars: Vec<char> = out.chars().collect();
    while i < chars.len() {
        // Look for '='
        if chars[i] == '=' {
            // Check if the word before '=' contains a secret keyword
            let word_start = chars[..i]
                .iter()
                .rposition(|&c| {
                    c == ' ' || c == '"' || c == '\'' || c == ';' || c == '&' || c == '\n'
                })
                .map(|p| p + 1)
                .unwrap_or(0);
            let word: String = chars[word_start..i].iter().collect();
            let word_lower = word.to_lowercase();
            let is_secret = SECRET_KEYWORDS
                .iter()
                .any(|k| word_lower.contains(&k.to_lowercase()));

            if is_secret {
                result.push('=');
                i += 1;
                // Skip optional quote
                let quote = if i < chars.len() && (chars[i] == '"' || chars[i] == '\'') {
                    let q = chars[i];
                    result.push(q);
                    i += 1;
                    Some(q)
                } else {
                    None
                };
                // The value runs until whitespace, matching quote, or end-of-string
                let val_start = i;
                while i < chars.len() {
                    let c = chars[i];
                    if let Some(q) = quote {
                        if c == q {
                            break;
                        }
                    } else if c == ' ' || c == '\'' || c == '"' || c == ';' || c == '&' || c == '\n'
                    {
                        break;
                    }
                    i += 1;
                }
                let val_len = i - val_start;
                if val_len >= 6 {
                    result.push_str("***REDACTED***");
                } else {
                    result.extend(chars[val_start..i].iter());
                }
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    out = result;

    // Pass 2 – HTTP header style:  xi-api-key: VALUE  /  Bearer VALUE
    let header_triggers = ["xi-api-key:", "Authorization:", "bearer ", "Bearer "];
    for trigger in &header_triggers {
        let trigger_lower = trigger.to_lowercase();
        let mut new_out = String::with_capacity(out.len());
        let mut rest = out.as_str();
        while let Some(pos) = rest.to_lowercase().find(trigger_lower.as_str()) {
            new_out.push_str(&rest[..pos + trigger.len()]);
            rest = &rest[pos + trigger.len()..];
            // skip optional space
            let rest_trimmed = rest.trim_start_matches(' ');
            let skipped = rest.len() - rest_trimmed.len();
            for _ in 0..skipped {
                new_out.push(' ');
            }
            rest = rest_trimmed;
            // consume the value token (until whitespace or quote or end)
            let end = rest.find([' ', '"', '\'', '\n']).unwrap_or(rest.len());
            if end >= 6 {
                new_out.push_str("***REDACTED***");
            } else {
                new_out.push_str(&rest[..end]);
            }
            rest = &rest[end..];
        }
        new_out.push_str(rest);
        out = new_out;
    }

    out
}

pub fn confirm_agent_action(
    tool_name: &str,
    description: &str,
    details: Option<&str>,
) -> Result<bool> {
    use comfy_table::modifiers::UTF8_ROUND_CORNERS;
    use comfy_table::presets::UTF8_BORDERS_ONLY;
    use comfy_table::{Attribute, Cell, Color, ColumnConstraint, Table, Width};
    use console::style;
    use dialoguer::Confirm;

    // Fit table to terminal: label col is fixed 18, value col fills rest.
    // Outer borders + padding cost ~6 chars, so subtract that from usable width.
    let term_cols = console::Term::stdout()
        .size_checked()
        .map(|(_, c)| c as usize)
        .unwrap_or(80);
    let table_width = term_cols.min(100); // cap at 100 for readability
    let label_col: u16 = 18;
    let value_col = (table_width.saturating_sub(label_col as usize + 6)) as u16;

    println!();
    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.apply_modifier(UTF8_ROUND_CORNERS);
    table.set_width(table_width as u16);
    table.set_constraints(vec![
        ColumnConstraint::Absolute(Width::Fixed(label_col)),
        ColumnConstraint::Absolute(Width::Fixed(value_col)),
    ]);

    // Header row
    let header_title = format!("⚠️   AGENT REQUEST: {}", tool_name.to_uppercase());
    table.add_row(vec![
        Cell::new(&header_title)
            .fg(Color::Yellow)
            .add_attribute(Attribute::Bold),
        Cell::new("PENDING USER APPROVAL")
            .fg(Color::DarkYellow)
            .add_attribute(Attribute::Bold)
            .set_alignment(comfy_table::CellAlignment::Right),
    ]);

    let div_label: String = "─".repeat(label_col as usize);
    let div_value: String = "─".repeat(value_col as usize);
    table.add_row(vec![
        Cell::new(&div_label).fg(Color::DarkGrey),
        Cell::new(&div_value).fg(Color::DarkGrey),
    ]);

    table.add_row(vec![
        Cell::new("Action Details")
            .fg(Color::DarkGrey)
            .add_attribute(Attribute::Bold),
        Cell::new(description).fg(Color::White),
    ]);

    if let Some(det) = details {
        let redacted = redact_secrets(det);
        table.add_row(vec![
            Cell::new("Target/Payload")
                .fg(Color::DarkGrey)
                .add_attribute(Attribute::Bold),
            Cell::new(&redacted).fg(Color::Cyan),
        ]);
    }

    println!("{table}");

    let allowed = Confirm::new()
        .with_prompt(style("Authorize action?").bold().to_string())
        .default(false)
        .interact()?;

    Ok(allowed)
}
