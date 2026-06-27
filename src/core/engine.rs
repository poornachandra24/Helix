#![allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::too_many_arguments,
    clippy::for_kv_map
)]

use anyhow::Result;
use console::style;
use owo_colors::OwoColorize;
use ruvector_sona::SonaEngine;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

use super::context::ContextManager;
use super::metrics::{MetricsCollector, TurnTimer};
use super::persistence::Session;
use crate::model::{ModelAdapter, ModelResponse, ToolCall};
use crate::tools::ToolRegistry;

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0;
    let mut norm_a = 0.0;
    let mut norm_b = 0.0;
    for (val_a, val_b) in a.iter().zip(b.iter()) {
        dot += val_a * val_b;
        norm_a += val_a * val_a;
        norm_b += val_b * val_b;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a.sqrt() * norm_b.sqrt())
}

pub struct Engine {
    /// Boxed model adapter — swap providers at runtime without recompiling.
    pub model: Box<dyn ModelAdapter>,
    pub context: ContextManager,
    /// Wrapped in Arc so individual tools can be cloned into parallel tasks.
    pub tools: Arc<ToolRegistry>,
    pub session: Session,
    pub global_messages: Vec<Value>,
    max_iterations: usize,
    max_retries: usize,
    /// Metrics collector. None in headless/benchmark mode to avoid disk writes.
    pub metrics: Option<MetricsCollector>,
    /// Local quantized semantic memory store.
    pub memory: Option<crate::memory::HelixMemoryEngine>,
    /// SONA self-optimizing engine.
    pub sona: Option<Arc<SonaEngine>>,
    /// Pre-computed embeddings for greetings and starter queries.
    pub semantic_cache: Vec<(String, String, Vec<f32>)>,
    pub core_tools: std::collections::HashMap<String, Arc<dyn crate::tools::Tool>>,
    pub dynamic_registry: crate::tools::SharedRegistryState,
}

impl Engine {
    pub fn new(
        model: Box<dyn ModelAdapter>,
        context: ContextManager,
        tools: ToolRegistry,
        session: Session,
        core_tools: std::collections::HashMap<String, Arc<dyn crate::tools::Tool>>,
        dynamic_registry: crate::tools::SharedRegistryState,
    ) -> Self {
        Self {
            model,
            context,
            tools: Arc::new(tools),
            session,
            global_messages: vec![],
            max_iterations: 20,
            max_retries: 3,
            metrics: None,
            memory: None,
            sona: None,
            semantic_cache: vec![],
            core_tools,
            dynamic_registry,
        }
    }

    /// Dynamically updates the engine's active tool registry at runtime.
    /// Re-evaluates token counts for the new tool descriptors and adjusts the active context budget accordingly.
    pub fn update_tools(&mut self, tools: ToolRegistry) {
        let descs = tools.descriptors();
        self.tools = Arc::new(tools);
        self.context.budget.tool_descriptor_tokens =
            crate::core::context::TokenEstimator::estimate_tool_descriptors(&descs);
    }

    pub fn with_memory(mut self, mut memory: crate::memory::HelixMemoryEngine) -> Self {
        let candidates = vec![
            ("hi", "Hello! How can I help you today?"),
            ("hello", "Hello! How can I help you today?"),
            ("hey", "Hey there! How can I help you today?"),
            (
                "sup",
                "Not much! How can I help you with your codebase today?",
            ),
            ("yo", "Yo! How can I help you today?"),
            (
                "hello there",
                "General Kenobi! Or rather, hello! How can I help you today?",
            ),
            ("hi there", "Hello! How can I help you today?"),
            (
                "how are you",
                "I'm doing great, thank you! Ready to help you with your code.",
            ),
            (
                "what are you",
                "I am Helix, a local, high-performance, tool-calling AI agent built in Rust.",
            ),
            ("who are you", "I am Helix, your local AI coding assistant."),
            (
                "what is this",
                "This is Helix, an autonomous coding agent harness designed to run with local or cloud LLMs.",
            ),
            (
                "how does this work",
                "Helix connects to your LLM provider of choice, manages workspace memories, and executes tools like bash and file operations to help you build software.",
            ),
            (
                "what can you do",
                "I can search and read workspace files, execute sandboxed shell commands, run local tools via the Model Context Protocol (MCP), and help you write or debug code.",
            ),
        ];

        let mut cache = Vec::new();
        for (q, r) in candidates {
            if let Ok(emb) = memory.embed_text(q) {
                cache.push((q.to_string(), r.to_string(), emb));
            }
        }
        self.semantic_cache = cache;
        self.memory = Some(memory);
        self
    }

    pub fn with_metrics(mut self, collector: MetricsCollector) -> Self {
        self.metrics = Some(collector);
        self
    }

    pub fn with_sona(mut self, sona: SonaEngine) -> Self {
        self.sona = Some(Arc::new(sona));
        self
    }

    pub fn update_model(&mut self, new_model: Box<dyn ModelAdapter>, new_context: ContextManager) {
        self.model = new_model;
        self.context = new_context;
    }

    // ──────────────────────────────────────────────
    // Internal helpers
    // ──────────────────────────────────────────────

    /// Attempt to get a valid (non-parse-error) response, retrying via the
    /// Local Healer on malformed tool JSON.
    ///
    /// Returns `(response, retries_used)` so the caller can record healer activity.
    fn classify_and_diagnose_parse_error(&self, error: &str, raw_text: &str) -> String {
        let mut tool_name = None;
        if let Ok(parsed) = serde_json::from_str::<Value>(raw_text) {
            if let Some(name) = parsed.get("name").and_then(|n| n.as_str()) {
                tool_name = Some(name.to_string());
            } else if let Some(first) = parsed.as_array().and_then(|c| c.first()) {
                tool_name = first
                    .get("name")
                    .and_then(|n| n.as_str())
                    .map(|n| n.to_string());
            }
        } else if let Some(start_idx) = raw_text.find("\"name\"") {
            let sub = &raw_text[start_idx..];
            if let Some(colon_idx) = sub.find(':') {
                let val_sub = &sub[colon_idx + 1..];
                if let Some(q1) = val_sub.find('"') {
                    let after_q1 = &val_sub[q1 + 1..];
                    if let Some(q2) = after_q1.find('"') {
                        tool_name = Some(after_q1[..q2].to_string());
                    }
                }
            }
        }

        let mut diagnosis = format!(
            "Your tool call failed to parse.\nError details: {}\n",
            error
        );

        if let Some(ref name) = tool_name {
            if self.tools.get(name).is_none() {
                let valid_names: Vec<String> = self
                    .tools
                    .descriptors()
                    .iter()
                    .map(|d| d.name.clone())
                    .collect();
                diagnosis.push_str(&format!(
                    "Tool name '{}' is NOT registered. Available tools are: {:?}.\nChoose only from the registered tools.\n",
                    name, valid_names
                ));
            } else {
                let tool = self.tools.get(name).unwrap();
                let schema = tool.parameters_schema();
                diagnosis.push_str(&format!(
                    "For tool '{}', the expected JSON parameter schema is:\n{}\n",
                    name,
                    serde_json::to_string_pretty(&schema).unwrap_or_default()
                ));

                if let Ok(parsed) = serde_json::from_str::<Value>(raw_text) {
                    let args = parsed
                        .get("arguments")
                        .or_else(|| parsed.get("args"))
                        .unwrap_or(&parsed);
                    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
                        let mut missing = Vec::new();
                        for req_field in required {
                            if let Some(field_name) =
                                req_field.as_str().filter(|name| args.get(*name).is_none())
                            {
                                missing.push(field_name);
                            }
                        }
                        if !missing.is_empty() {
                            diagnosis.push_str(&format!(
                                "Critical: Missing required fields: {:?}. You MUST provide these fields.\n",
                                missing
                            ));
                        }
                    }
                }
            }
        } else {
            diagnosis.push_str("Suggestions:\n- Make sure all quotes and commas are properly closed.\n- Follow the markdown ```json ... ``` format precisely.\n- Do not include conversational text or preamble inside the JSON block if calling a tool.\n");
        }

        diagnosis
    }

    /// Attempt to get a valid (non-parse-error) response, retrying via the
    /// Local Healer on malformed tool JSON.
    ///
    /// Returns `(response, retries_used)` so the caller can record healer activity.
    async fn get_valid_action(
        &self,
        system_prompt: &str,
        global_messages: &[Value],
        stream_tx: Option<UnboundedSender<String>>,
    ) -> Result<(ModelResponse, usize)> {
        let tools = self.tools.descriptors();
        let mut local_messages = global_messages.to_vec();
        let mut retries_used = 0usize;

        for attempt in 1..=self.max_retries {
            let tx = if attempt == 1 {
                stream_tx.clone()
            } else {
                None
            };

            if attempt > 1 {
                retries_used += 1;
                tracing::warn!(attempt, max = self.max_retries, "Local Healer retry");
            }

            let response = self
                .model
                .call(system_prompt, &local_messages, tools.clone(), tx)
                .await?;

            match response {
                ModelResponse::ToolCalls(..) | ModelResponse::EndTurn(_) => {
                    return Ok((response, retries_used));
                }
                ModelResponse::ParseError { raw_text, error } => {
                    println!("⚠️  {}", style("[Syntax Error: Auto-healing...]").yellow());
                    tracing::warn!(%error, "Tool JSON parse error, healing");

                    let diagnosis = self.classify_and_diagnose_parse_error(&error, &raw_text);

                    local_messages.push(json!({"role": "assistant", "content": raw_text}));
                    local_messages.push(json!({
                        "role": "user",
                        "content": diagnosis
                    }));
                }
            }
        }

        anyhow::bail!("Local Healer exhausted {} retries", self.max_retries)
    }

    /// Dispatch all tool calls.
    /// - Tools that require confirmation are dispatched sequentially (interactive prompt).
    /// - Read-only tools are dispatched in parallel via `tokio::spawn`.
    ///
    /// Returns `(results, total_tool_count)`.
    async fn dispatch_tools(
        &self,
        calls: Vec<ToolCall>,
        stream_tx: Option<UnboundedSender<String>>,
    ) -> (Vec<(ToolCall, String)>, usize) {
        let total = calls.len();
        let needs_serial = calls.iter().any(|c| {
            self.tools
                .get(&c.name)
                .map(|t| t.requires_confirmation())
                .unwrap_or(false)
        });

        let results = if needs_serial || calls.len() == 1 {
            let mut results = Vec::new();
            for call in calls {
                let requires_confirm = self
                    .tools
                    .get(&call.name)
                    .map(|t| t.requires_confirmation())
                    .unwrap_or(false);

                if let Some(ref tx) = stream_tx {
                    if requires_confirm {
                        let _ = tx.send("\x03".to_string());
                    } else {
                        let _ = tx.send("\x02".to_string());
                        let _ = tx.send(format!("\x1b[SExecuting {}...", call.name));
                    }
                }
                let result = match self.tools.dispatch(&call.name, call.args.clone()).await {
                    Ok(r) => r,
                    Err(e) => format!("Error in tool '{}': {}", call.name, e),
                };
                results.push((call, result));
            }
            results
        } else {
            if let Some(ref tx) = stream_tx {
                let _ = tx.send(format!(
                    "\x1b[SRunning {} tools concurrently...",
                    calls.len()
                ));
            }
            let mut handles = Vec::new();
            for call in &calls {
                let name = call.name.clone();
                let args = call.args.clone();
                let registry = Arc::clone(&self.tools);
                handles.push(tokio::spawn(async move {
                    registry
                        .dispatch(&name, args)
                        .await
                        .unwrap_or_else(|e| format!("Error in tool '{}': {}", name, e))
                }));
            }

            let mut results = Vec::new();
            for (call, handle) in calls.into_iter().zip(handles) {
                let result = match handle.await {
                    Ok(r) => r,
                    Err(e) => format!("Task join error: {}", e),
                };
                results.push((call, result));
            }
            results
        };

        (results, total)
    }

    // ──────────────────────────────────────────────
    // Public API
    // ──────────────────────────────────────────────

    /// Run one user turn through the full agent loop.
    ///
    /// `stream_tx`: if `Some`, text tokens for the final answer are sent through
    /// this channel as they arrive so the REPL can print them progressively.
    #[tracing::instrument(skip(self, system_prompt, stream_tx),
        fields(model = %self.model.model_name(), provider = %self.model.provider_name()))]
    pub async fn run_turn(
        &mut self,
        system_prompt: &str,
        input: &str,
        stream_tx: Option<UnboundedSender<String>>,
    ) -> Result<String> {
        let turn_index = self.metrics.as_ref().map(|m| m.turn_count()).unwrap_or(0);
        let timer = TurnTimer::start(input);

        // Per-turn counters — accumulated across all steps in this turn
        let mut turn_tool_calls: usize = 0;
        let mut turn_healer_retries: usize = 0;
        let mut turn_compaction_fired = false;
        let mut turn_steps: usize = 0;

        let query_embedding = if let Some(ref mut memory_engine) = self.memory {
            memory_engine
                .embed_text(input)
                .unwrap_or_else(|_| vec![0.0; 384])
        } else {
            vec![0.0; 384]
        };
        let sona_trajectory = self
            .sona
            .as_deref()
            .map(|s| s.begin_trajectory(query_embedding.clone()));

        // ── Check Semantic Cache ─────────────────────────────
        let cleaned = input.trim().to_lowercase();
        let mut cache_hit: Option<String> = None;

        // 1. Lexical fallback (instant)
        for (q, r, _) in &self.semantic_cache {
            if cleaned == *q {
                cache_hit = Some(r.clone());
                break;
            }
        }

        // 2. Semantic lookup using the pre-computed embeddings
        if cache_hit.is_none() && !query_embedding.iter().all(|&x| x == 0.0) {
            for (q, r, emb) in &self.semantic_cache {
                let sim = cosine_similarity(&query_embedding, emb);
                if sim > 0.88 {
                    tracing::info!(
                        "Semantic cache hit for '{}' matching '{}' with similarity {:.4}",
                        cleaned,
                        q,
                        sim
                    );
                    cache_hit = Some(r.clone());
                    break;
                }
            }
        }

        if let Some(response) = cache_hit {
            // Echo inputs to local session/global logs
            self.global_messages
                .push(json!({"role": "user", "content": input}));
            self.session
                .append(json!({"event": "user_input", "content": input}))?;

            // Stream response to the user with a dynamic token pacing animation
            if let Some(ref tx) = stream_tx {
                // Clear the querying semantic memory spinner if it started
                let _ = tx.send("\x03".to_string());
                for token in response.split_inclusive(char::is_whitespace) {
                    let _ = tx.send(token.to_string());
                    tokio::time::sleep(std::time::Duration::from_millis(15)).await;
                }
                let _ = tx.send("\x04".to_string());
            }

            self.global_messages
                .push(json!({"role": "assistant", "content": response}));
            self.session
                .append(json!({"event": "assistant_output", "content": response}))?;

            // Commit to memory
            if let Some(ref mut memory_engine) = self.memory
                && let Ok(workspace_dir) = std::env::current_dir()
            {
                let workspace_str = workspace_dir.to_string_lossy().to_string();
                let memory_text = format!("User: {}\nAssistant: {}", input, response);
                let _ = memory_engine.insert(&memory_text, None, &workspace_str);
            }

            // Record metrics
            if let Some(ref mut collector) = self.metrics {
                let m = timer.finish(
                    turn_index, 0,     // steps
                    0,     // tool calls
                    0,     // healer retries
                    false, // compaction fired
                    false, // is_err
                );
                collector.record(m);
            }

            return Ok(response);
        }

        self.global_messages
            .push(json!({"role": "user", "content": input}));
        self.session
            .append(json!({"event": "user_input", "content": input}))?;

        // Classify query complexity
        let complexity_system_prompt = "You are a query complexity classifier. Classify the user query as 'complex' (if it requires planning, tools, coding, file access, bash commands, or multi-step execution) or 'simple' (if it is a greeting, basic question, simple explanation, or conversational prompt). Output ONLY 'complex' or 'simple' with no other text.";
        let mut is_complex = false;
        if let Ok(ModelResponse::EndTurn(classification)) = self
            .model
            .call(
                complexity_system_prompt,
                &[json!({"role": "user", "content": input})],
                vec![],
                None,
            )
            .await
        {
            if classification.trim().to_lowercase().contains("complex") {
                is_complex = true;
            }
        }

        if is_complex {
            // Initialize the workspace scratchpad at the start of a complex turn
            let scratchpad_header = format!(
                "# Helix Scratchpad & Planning Log\n\n**Current Goal**: {}\n",
                input
            );
            let _ = std::fs::write(".helix_scratchpad.md", &scratchpad_header);
        } else {
            // Ensure no stale scratchpad is left over
            let _ = std::fs::remove_file(".helix_scratchpad.md");
        }

        // 1. Retrieve relevant workspace memories and append them to the system prompt
        let mut final_system_prompt = system_prompt.to_string();
        let current_time = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        final_system_prompt.push_str(&format!(
            "\n\n### SYSTEM TEMPORAL CONTEXT:\n- Current Local Time: {}\n",
            current_time
        ));
        let mut retrieved_texts = Vec::new();
        if let Some(ref mut memory_engine) = self.memory {
            if let Some(ref tx) = stream_tx {
                let _ = tx.send("\x02".to_string());
                let _ = tx.send("\x1b[SQuerying semantic memory...".to_string());
            }
            if let Ok(workspace_dir) = std::env::current_dir() {
                let workspace_str = workspace_dir.to_string_lossy().to_string();
                if let Ok(matches) =
                    memory_engine.search(input, self.sona.as_deref(), &workspace_str, 5)
                {
                    if !matches.is_empty() {
                        if let Some(ref tx) = stream_tx {
                            let bullet = style("◆").color256(81);
                            let sona_hint = if self.sona.is_some() {
                                format!(" {}", style("(adapted via Micro-LoRA)").color256(243))
                            } else {
                                String::new()
                            };
                            let msg = format!(
                                "  {} Memory: Found {} relevant workspace memories{}",
                                bullet,
                                style(matches.len()).bold().color256(253),
                                sona_hint
                            );
                            let _ = tx.send(format!("\x1b[T{}", msg));
                        }
                        let mut memory_str =
                            String::from("\n\n### RELEVANT MEMORIES (from this workspace):\n");
                        for m in matches {
                            memory_str
                                .push_str(&format!("- {} (similarity: {:.2})\n", m.text, m.score));
                            retrieved_texts.push(m.text.clone());
                        }
                        final_system_prompt.push_str(&memory_str);
                    } else if let Some(ref tx) = stream_tx {
                        let bullet = style("◇").color256(244);
                        let msg = format!(
                            "  {} Memory: No matching memories found in workspace",
                            bullet
                        );
                        let _ = tx.send(format!("\x1b[T{}", msg));
                    }
                }
            }
        }

        let mut executed_tools = Vec::new();
        let stream_tx_clone = stream_tx.clone();
        let result = self
            .run_turn_inner(
                &final_system_prompt,
                input,
                stream_tx,
                &mut turn_steps,
                &mut turn_tool_calls,
                &mut turn_healer_retries,
                &mut turn_compaction_fired,
                &mut executed_tools,
            )
            .await;

        if !executed_tools.is_empty() {
            if let Some(ref tx) = stream_tx_clone {
                let mut counts = std::collections::HashMap::new();
                for t in &executed_tools {
                    *counts.entry(t).or_insert(0) += 1;
                }
                let mut parts = Vec::new();
                for (name, count) in counts {
                    if count > 1 {
                        parts.push(format!("{} ({})", name, count));
                    } else {
                        parts.push(name.clone());
                    }
                }
                parts.sort();
                let summary = format!(
                    "  ● [Executed {} tool{}: {}]",
                    executed_tools.len(),
                    if executed_tools.len() > 1 { "s" } else { "" },
                    parts.join(", ")
                );
                let _ = tx.send(format!("\x1b[T{}", style(summary).color256(243)));
            }
        }

        if let Some(ref tx) = stream_tx_clone {
            let _ = tx.send("\x04".to_string());
        }

        // 2. Commit this turn to memory on success
        if let Ok(ref text) = result
            && let Some(ref mut memory_engine) = self.memory
            && let Ok(workspace_dir) = std::env::current_dir()
        {
            let workspace_str = workspace_dir.to_string_lossy().to_string();
            let memory_text = format!("User: {}\nAssistant: {}", input, text);
            if let Err(e) = memory_engine.insert(&memory_text, None, &workspace_str) {
                tracing::error!("Failed to save memory: {}", e);
            }
        }

        // Record metrics regardless of success/failure
        if let Some(ref mut collector) = self.metrics {
            let m = timer.finish(
                turn_index,
                turn_steps,
                turn_tool_calls,
                turn_healer_retries,
                turn_compaction_fired,
                result.is_err(),
            );
            collector.record(m);
        }

        // Finalize SONA Trajectory
        if let (Some(sona_engine), Some(mut trajectory)) = (self.sona.clone(), sona_trajectory) {
            let is_err = result.is_err();
            let mut quality = 1.0f32;
            quality -= (turn_healer_retries as f32) * 0.15;
            if turn_compaction_fired {
                quality -= 0.20;
            }
            let step_penalty = (turn_steps as f32 / 20.0).min(0.3);
            quality -= step_penalty;
            if is_err {
                quality = 0.0;
            }
            let final_quality = quality.clamp(0.0, 1.0);

            // Compute target embedding based on retrieved memories
            let mut avg_target = vec![0.0f32; 384];
            let mut count = 0;
            if let Some(ref mut memory_engine) = self.memory {
                for text in &retrieved_texts {
                    if let Ok(emb) = memory_engine.embed_text(text) {
                        for (i, val) in emb.iter().enumerate() {
                            avg_target[i] += val;
                        }
                        count += 1;
                    }
                }
            }
            if count > 0 {
                for val in avg_target.iter_mut() {
                    *val /= count as f32;
                }
            } else {
                avg_target = query_embedding.clone();
            }

            // Train the LoRA projection to map from the query embedding to the target (successful document centroid)
            trajectory.add_step(query_embedding, avg_target, final_quality);
            trajectory.set_model_route(self.model.model_name());

            sona_engine.end_trajectory(trajectory, final_quality);
            if let Some(ref tx) = stream_tx_clone {
                let sona_tag = style("sona").color256(242);
                let quality_colored = if final_quality > 0.8 {
                    style(format!("{:.2}", final_quality)).green()
                } else if final_quality > 0.5 {
                    style(format!("{:.2}", final_quality)).yellow()
                } else {
                    style(format!("{:.2}", final_quality)).red()
                };
                let _ = tx.send(format!(
                    "\x1b[T  {} [sona] quality {}",
                    style("·").color256(242),
                    quality_colored
                ));
                let sona_clone = sona_engine.clone();
                let tx_clone = tx.clone();
                let sona_tag_clone = sona_tag.clone();
                tokio::task::spawn_blocking(move || {
                    if let Some(log_msg) = sona_clone.tick() {
                        let _ = tx_clone.send(format!(
                            "\x1b[T  {} Background loop tick: {}",
                            sona_tag_clone,
                            style(log_msg).color256(117)
                        ));
                    }
                });
            } else {
                let sona_clone = sona_engine.clone();
                tokio::task::spawn_blocking(move || {
                    if let Some(log_msg) = sona_clone.tick() {
                        tracing::info!("{}", log_msg);
                    }
                });
            }
        }

        result
    }

    async fn run_turn_inner(
        &mut self,
        system_prompt: &str,
        input: &str,
        stream_tx: Option<UnboundedSender<String>>,
        steps_out: &mut usize,
        tool_calls_out: &mut usize,
        healer_retries_out: &mut usize,
        compaction_fired_out: &mut bool,
        executed_tools: &mut Vec<String>,
    ) -> Result<String> {
        for step in 1..=self.max_iterations {
            *steps_out = step;
            tracing::info!(step, max = self.max_iterations, "Agent step");

            if let Some(ref tx) = stream_tx {
                let _ = tx.send(format!("\x1b[SWorking (step {})...", step));
            }

            let mut tool_counts = std::collections::HashMap::new();
            for name in executed_tools.iter() {
                *tool_counts.entry(name.clone()).or_insert(0) += 1;
            }
            for (name, count) in tool_counts {
                if count >= 5 {
                    let warning = format!(
                        "[System Note]: You have already called the '{}' tool {} times in this turn. \
                        To prevent infinite loops, further calls to '{}' are restricted. \
                        Please summarize the results you have obtained so far and provide your final answer or state what you could not find.",
                        name, count, name
                    );
                    self.global_messages.push(json!({
                        "role": "user",
                        "content": warning
                    }));
                }
            }

            let (compacted, was_compacted) =
                self.context.compact_if_needed(self.global_messages.clone());
            if was_compacted {
                *compaction_fired_out = true;
                tracing::info!("Context compacted at step {}", step);
                if let Some(ref tx) = stream_tx {
                    let msg = "  Compact: shrinking message history".to_string();
                    let _ = tx.send(format!("\x1b[T{}", msg));
                }
            }
            self.global_messages = compacted;

            // Check if dynamic registry state has changed
            let mut rebuilt_tools = None;
            {
                let mut state = self.dynamic_registry.lock().unwrap();
                if state.changed {
                    state.changed = false;
                    let mut new_tools = ToolRegistry::new();
                    // Register all core tools
                    for (_name, tool) in &self.core_tools {
                        new_tools.register_arc(tool.clone());
                    }
                    // Register all active MCP tools
                    for active_t in &state.active_tools {
                        if let Some(tool) = state.all_mcp_tools.get(active_t) {
                            new_tools.register_arc(tool.clone());
                        }
                    }
                    rebuilt_tools = Some(new_tools);
                }
            }
            if let Some(new_tools) = rebuilt_tools {
                self.update_tools(new_tools);
            }

            // Dynamic workspace scratchpad injection
            let mut step_system_prompt = system_prompt.to_string();
            // Append dynamically active skills
            {
                let state = self.dynamic_registry.lock().unwrap();
                let mut skill_sections = Vec::new();
                for active_s in &state.active_skills {
                    if let Some(content) = state.all_skills.get(active_s) {
                        skill_sections.push(format!(
                            "--- Skill: {} ---\n{}",
                            active_s,
                            content.trim()
                        ));
                    }
                }
                if !skill_sections.is_empty() {
                    step_system_prompt.push_str(&format!(
                        "\n\nYou have the following domain-specific skills loaded in your context:\n{}",
                        skill_sections.join("\n\n")
                    ));
                }
            }

            if let Ok(scratchpad_content) = std::fs::read_to_string(".helix_scratchpad.md") {
                if !scratchpad_content.trim().is_empty() {
                    step_system_prompt.push_str(&format!(
                        "\n\n### ACTIVE WORKSPACE SCRATCHPAD & PLANNING LOG:\n{}\n",
                        scratchpad_content
                    ));
                }
            }

            let tx = stream_tx.clone();
            let (response, retries) = self
                .get_valid_action(&step_system_prompt, &self.global_messages, tx)
                .await?;
            *healer_retries_out += retries;

            match response {
                ModelResponse::EndTurn(text) => {
                    self.session
                        .append(json!({"event": "end_turn", "content": &text}))?;
                    self.global_messages
                        .push(json!({"role": "assistant", "content": &text}));

                    return Ok(text);
                }

                ModelResponse::ToolCalls(calls, raw_msg) => {
                    for call in &calls {
                        executed_tools.push(call.name.clone());
                    }

                    self.global_messages.push(raw_msg);

                    self.session.append(json!({
                        "event": "tool_calls",
                        "tools": calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
                    }))?;

                    let (results, dispatched) = self.dispatch_tools(calls, stream_tx.clone()).await;
                    *tool_calls_out += dispatched;

                    let mut step_failures = Vec::new();
                    for (call, result) in results {
                        tracing::debug!(tool = %call.name, result = %result, "Tool result");
                        self.session.append(json!({
                            "event": "tool_result",
                            "tool":   call.name,
                            "result": &result,
                        }))?;

                        if result.contains("User denied ") {
                            if let Some(ref tx) = stream_tx {
                                let _ = tx.send("\x03".to_string());
                            }
                            anyhow::bail!("Action authorization denied by user.");
                        }

                        let is_failure = result.contains("Error")
                            || result.contains("failed")
                            || result.contains("permission denied");
                        if is_failure {
                            step_failures.push((call.name.clone(), result.clone()));
                        }

                        // Truncate massively long tool outputs to prevent context window overflow / 500 errors
                        let max_chars = 16_000;
                        let content = if result.len() > max_chars {
                            let head = &result[..max_chars / 2];
                            let tail = &result[result.len() - (max_chars / 2)..];
                            let omitted = result.len() - max_chars;
                            format!(
                                "{}\n\n... [{} bytes omitted by context truncator] ...\n\n{}",
                                head, omitted, tail
                            )
                        } else {
                            result.clone()
                        };

                        self.global_messages.push(json!({
                            "role":         "tool",
                            "tool_call_id": call.id,
                            "content":      content,
                        }));
                    }
                    // Self-Correction & Planning Reflection logic (Runs on every step if scratchpad exists - meaning it's a complex query)
                    let scratchpad_exists = std::path::Path::new(".helix_scratchpad.md").exists();
                    if scratchpad_exists {
                        let is_error_reflection = !step_failures.is_empty();

                        // 1. Send warning/status to the user
                        if let Some(ref tx) = stream_tx {
                            let status_msg = if is_error_reflection {
                                format!(
                                    "    [Self-Correction] Failure detected in '{}'. Reflecting...",
                                    style(&step_failures[0].0).cyan()
                                )
                            } else {
                                format!(
                                    "    [Planning] Step {} completed. Updating planning logs...",
                                    step
                                )
                            };
                            let _ = tx.send(format!("\x1b[T{}", status_msg));
                        }

                        // 2. Build the user prompt for reflection
                        let user_content = if is_error_reflection {
                            let mut failures_str = String::new();
                            for (tool_name, error_msg) in &step_failures {
                                failures_str.push_str(&format!(
                                    "Tool '{}' failed with error:\n{}\n\n",
                                    tool_name, error_msg
                                ));
                            }
                            format!(
                                "Here are the failures that just occurred:\n{}\nPlease reflect on these and output the JSON correction plan.",
                                failures_str
                            )
                        } else {
                            "Analyze the tools run and their results in this step. Reflect on progress and output the JSON planning/correction plan to guide the next steps.".to_string()
                        };

                        let reflection_system_prompt = "You are an autonomous AI agent's internal Self-Correction & Planning system. \
                            Analyze the progress of the turn and formulate/update the plan. \
                            Analyze in terms of the following principles:\n\
                            1. Current State: What is the current state of files, progress, and errors?\n\
                            2. Target State: What is the final target condition to resolve the goal?\n\
                            3. Strategy & Gap: How does the strategy close the gap between current and target state?\n\n\
                            Important Tips for Strategies:\n\
                            - Never perform recursive directory searches on binary/build directories (like 'target/', '.git/', or 'node_modules/'). Always restrict grep/find commands to specific directories (e.g. 'src/', 'tests/').\n\
                            - Double check path names and tool arguments before calling tools.\n\n\
                            Output ONLY a JSON object in the following format, with no conversational text or preamble:\n\
                            {\n\
                              \"current_state\": \"1-sentence summary of current state and progress/failures\",\n\
                              \"target_state\": \"1-sentence summary of the target state\",\n\
                              \"strategy_to_close_gap\": \"1-sentence plan to close the gap\",\n\
                              \"next_steps\": [\"Step 1 description\", \"Step 2 description\"]\n\
                            }";

                        let mut reflection_messages = self.global_messages.clone();
                        reflection_messages.push(json!({
                            "role": "user",
                            "content": user_content
                        }));

                        // Call model without tools
                        if let Ok(model_response) = self
                            .model
                            .call(
                                reflection_system_prompt,
                                &reflection_messages,
                                vec![], // No tools for reflection
                                None,
                            )
                            .await
                        {
                            if let ModelResponse::EndTurn(ref reflection_text) = model_response {
                                if let Ok(parsed) =
                                    serde_json::from_str::<Value>(reflection_text.trim())
                                {
                                    let current_state =
                                        parsed["current_state"].as_str().unwrap_or("");
                                    let target_state =
                                        parsed["target_state"].as_str().unwrap_or("");
                                    let strategy_to_close_gap =
                                        parsed["strategy_to_close_gap"].as_str().unwrap_or("");
                                    let next_steps = parsed["next_steps"].as_array();

                                    if let Some(ref tx) = stream_tx {
                                        let trace_header = format!(
                                            "  {} {}",
                                            style("▼").purple().bold(),
                                            style("Thinking Process (Self-Correction & Planning)")
                                                .purple()
                                                .dimmed()
                                        );
                                        let _ = tx.send(format!("\x1b[T{}", trace_header));
                                        let _ = tx.send(format!(
                                            "\x1b[T    {} Current State: {}",
                                            style("├─").purple().dimmed(),
                                            style(current_state).color256(246)
                                        ));
                                        let _ = tx.send(format!(
                                            "\x1b[T    {} Target State:  {}",
                                            style("├─").purple().dimmed(),
                                            style(target_state).color256(246)
                                        ));
                                        let _ = tx.send(format!(
                                            "\x1b[T    {} Gap/Strategy:  {}",
                                            style("├─").purple().dimmed(),
                                            style(strategy_to_close_gap).color256(246)
                                        ));
                                        if let Some(steps) = next_steps {
                                            let steps_str: Vec<String> = steps
                                                .iter()
                                                .map(|s| format!("'{}'", s.as_str().unwrap_or("")))
                                                .collect();
                                            let _ = tx.send(format!(
                                                "\x1b[T    {} Next Steps:   [{}]",
                                                style("└─").purple().dimmed(),
                                                style(steps_str.join(", ")).color256(246)
                                            ));
                                        }
                                        let _ = tx.send("\x1b[T".to_string());
                                    }

                                    let mut next_steps_md = String::new();
                                    if let Some(steps) = next_steps {
                                        for s in steps {
                                            next_steps_md.push_str(&format!(
                                                "  - {}\n",
                                                s.as_str().unwrap_or("")
                                            ));
                                        }
                                    }
                                    let reflection_section = format!(
                                        "\n## Latest Planning & Correction Reflection\n\
                                         - **Current State**: {}\n\
                                         - **Target State**: {}\n\
                                         - **Strategy to Close Gap**: {}\n\
                                         - **Proposed Next Steps**:\n{}\n",
                                        current_state,
                                        target_state,
                                        strategy_to_close_gap,
                                        next_steps_md
                                    );

                                    let clean_scratchpad = format!(
                                        "# Helix Scratchpad & Planning Log\n\n\
                                         **Current Goal**: {}\n\n\
                                         {}\n",
                                        input, reflection_section
                                    );
                                    let _ =
                                        std::fs::write(".helix_scratchpad.md", clean_scratchpad);

                                    let guidance_msg = format!(
                                        "[System Note - Self-Correction Strategy]:\nCurrent State: {}\nTarget State: {}\nStrategy: {}\nFollow this strategy to close the gap.",
                                        current_state, target_state, strategy_to_close_gap
                                    );
                                    self.global_messages.push(json!({
                                        "role": "user",
                                        "content": guidance_msg
                                    }));
                                } else {
                                    if let Some(ref tx) = stream_tx {
                                        let trace_header = format!(
                                            "  {} {}",
                                            style("▼").purple().bold(),
                                            style("Thinking Process (Raw Trace)").purple().dimmed()
                                        );
                                        let _ = tx.send(format!("\x1b[T{}", trace_header));
                                        for line in reflection_text.trim().lines() {
                                            let _ = tx.send(format!(
                                                "\x1b[T    {} {}",
                                                style("│").purple().dimmed(),
                                                style(line).color256(246)
                                            ));
                                        }
                                        let _ = tx.send("\x1b[T".to_string());
                                    }

                                    let clean_scratchpad = format!(
                                        "# Helix Scratchpad & Planning Log\n\n\
                                         **Current Goal**: {}\n\n\
                                         ## Latest Self-Correction Reflection\n\
                                         {}\n",
                                        input,
                                        reflection_text.trim()
                                    );
                                    let _ =
                                        std::fs::write(".helix_scratchpad.md", clean_scratchpad);

                                    let guidance_msg = format!(
                                        "[System Note - Self-Correction Strategy]:\n{}",
                                        reflection_text.trim()
                                    );
                                    self.global_messages.push(json!({
                                        "role": "user",
                                        "content": guidance_msg
                                    }));
                                }
                            }
                        }
                    }
                }

                ModelResponse::ParseError { .. } => {
                    unreachable!("Healer should have resolved this")
                }
            }
        }

        anyhow::bail!(
            "Stopped after {} iterations without a final answer",
            self.max_iterations
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-5);

        let c = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &c).abs() < 1e-5);

        let d = vec![
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        ];
        assert!((cosine_similarity(&a, &d) - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
    }
}
