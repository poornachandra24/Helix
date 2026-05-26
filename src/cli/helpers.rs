use anyhow::Result;
use console::Style;
use crate::tools::builtins;
use crate::config;
use crate::core::context;
use crate::tools::mcp;
use crate::model;
use crate::model::registry as model_registry;
use crate::tools::sandbox;
use crate::memory::skills;
use crate::tools;

pub fn print_banner(config: &config::AppConfig) {
    let logo_lines = [
        "    __  __   ______   __       ____  __  __",
        "   / / / /  / ____/  / /      /  _/  \\ \\/ /",
        "  / /_/ /  / __/    / /       / /     \\  / ",
        " / __  /  / /___   / /___   _/ /      /  \\ ",
        "/_/ /_/  /_____/  /_____/  /___/     /_/\\_\\",
    ];

    let colors = [
        Style::new().color256(81),  // Soft Cyan
        Style::new().color256(75),  // Sky Blue
        Style::new().color256(111), // Pastel Blue
        Style::new().color256(147), // Soft Lavender
        Style::new().color256(183), // Soft Violet/Pink
    ];

    println!();
    for (i, line) in logo_lines.iter().enumerate() {
        println!("{}", colors[i].apply_to(line));
    }
    println!();

    let border_color = Style::new().color256(240); // Subtle dark grey border
    let label_style = Style::new().color256(248).bold(); // Light grey labels
    let val_style = Style::new().color256(253); // Bright white values
    let active_indicator = Style::new().color256(46); // Matrix Green

    let provider = &config.active_provider;
    let model = &config.active_model;
    let ws = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "Unknown".to_string());

    let ws_display = if ws.len() > 38 {
        format!("...{}", &ws[ws.len() - 35..])
    } else {
        ws.clone()
    };

    // Helper to print a key-value row with perfect padding alignment
    let print_labeled_row = |label: &str, val: &str| {
        let left = format!("{:<12}: {}", label, val);
        let left_len = left.chars().count();
        let middle_spaces = 52_usize.saturating_sub(left_len);
        let spaces = " ".repeat(middle_spaces);
        println!(
            "{}  {:<12}: {}{}  {}",
            border_color.apply_to("│"),
            label_style.apply_to(label),
            val_style.apply_to(val),
            spaces,
            border_color.apply_to("│")
        );
    };

    println!("{}", border_color.apply_to("┌────────────────────────────────────────────────────────┐"));
    
    // Header row: Status online
    let header_left = "● HELIX CORE ENGINE v0.1.0";
    let header_right = "STATUS: ONLINE";
    let left_len = header_left.chars().count();
    let right_len = header_right.chars().count();
    let middle_spaces = 52_usize.saturating_sub(left_len).saturating_sub(right_len);
    let spaces = " ".repeat(middle_spaces);
    println!(
        "{}  {} {}{}{}  {}",
        border_color.apply_to("│"),
        active_indicator.apply_to("●"),
        label_style.apply_to("HELIX CORE ENGINE v0.1.0"),
        spaces,
        active_indicator.apply_to("STATUS: ONLINE"),
        border_color.apply_to("│")
    );

    println!("{}", border_color.apply_to("├────────────────────────────────────────────────────────┤"));
    print_labeled_row("PROVIDER", provider);
    print_labeled_row("MODEL", model);
    print_labeled_row("WORKSPACE", &ws_display);
    println!("{}", border_color.apply_to("├────────────────────────────────────────────────────────┤"));
    
    // Help row
    let help_left = "💡 Type /help to list system commands.";
    let help_left_len = help_left.chars().count();
    let middle_spaces = 52_usize.saturating_sub(help_left_len);
    let spaces = " ".repeat(middle_spaces);
    println!(
        "{}  {} {}{}{}{}  {}",
        border_color.apply_to("│"),
        Style::new().color256(220).apply_to("💡"),
        label_style.apply_to("Type "),
        Style::new().color256(51).apply_to("/help"),
        label_style.apply_to(" to list system commands."),
        spaces,
        border_color.apply_to("│")
    );
    
    println!("{}", border_color.apply_to("└────────────────────────────────────────────────────────┘"));
}

pub fn build_system_prompt(base: &str, skill_reg: &skills::SkillRegistry) -> String {
    let skills_suffix = skill_reg.load_skills_prompt().unwrap_or_default();
    format!("{}{}", base, skills_suffix)
}

pub fn build_tool_registry(sandbox: sandbox::SharedSandbox) -> tools::ToolRegistry {
    let mut registry = tools::ToolRegistry::new();
    registry.register(builtins::BashTool::new(sandbox.clone()));
    registry.register(builtins::ReadFileTool::new(sandbox.clone()));
    registry.register(builtins::WriteFileTool::new(sandbox.clone()));
    registry.register(builtins::ListDirTool::new(sandbox.clone()));
    registry.register(builtins::WebFetchTool::new());
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
    let model_window = model_registry::resolve_and_report(config, lookup_client).await;

    let budget = context::ContextBudget::new(
        model_window,
        system_prompt,
        tools,
        config.effective_headroom(),
    );

    tracing::info!(
        model_window,
        system_tokens  = budget.system_prompt_tokens,
        tool_tokens    = budget.tool_descriptor_tokens,
        available      = budget.available_for_messages(),
        "Context budget initialised"
    );

    context::ContextManager::new(budget)
}

pub fn count_omitted_turns(messages: &[serde_json::Value]) -> usize {
    let mut total = 0;
    for msg in messages {
        if let Some(content) = msg.get("content").and_then(|v| v.as_str()) {
            if content.contains("intermediate turns were omitted") {
                if let Some(part) = content.split("intermediate turns").next() {
                    if let Some(num_str) = part.split(':').last() {
                        if let Some(num_only) = num_str.trim().split_whitespace().next() {
                            if let Ok(num) = num_only.parse::<usize>() {
                                total += num;
                            }
                        }
                    }
                }
            }
        }
    }
    total
}
