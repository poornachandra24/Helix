use anyhow::Result;
use console::style;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use ruvector_sona::SonaEngine;

use super::context::ContextManager;
use super::metrics::{MetricsCollector, TurnTimer};
use crate::model::{ModelAdapter, ModelResponse, ToolCall};
use super::persistence::Session;
use crate::tools::ToolRegistry;

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
    /// TurboQuant-powered local semantic memory store.
    pub memory: Option<crate::memory::HelixMemoryEngine>,
    /// SONA self-optimizing engine.
    pub sona: Option<SonaEngine>,
}

impl Engine {
    pub fn new(
        model: Box<dyn ModelAdapter>,
        context: ContextManager,
        tools: ToolRegistry,
        session: Session,
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
        }
    }

    pub fn with_memory(mut self, memory: crate::memory::HelixMemoryEngine) -> Self {
        self.memory = Some(memory);
        self
    }

    pub fn with_metrics(mut self, collector: MetricsCollector) -> Self {
        self.metrics = Some(collector);
        self
    }

    pub fn with_sona(mut self, sona: SonaEngine) -> Self {
        self.sona = Some(sona);
        self
    }

    pub fn update_model(
        &mut self,
        new_model: Box<dyn ModelAdapter>,
        new_context: ContextManager,
    ) {
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
            let tx = if attempt == 1 { stream_tx.clone() } else { None };

            if attempt > 1 {
                retries_used += 1;
                tracing::warn!(attempt, max = self.max_retries, "Local Healer retry");
            }

            let response = self.model.call(system_prompt, &local_messages, tools.clone(), tx).await?;

            match response {
                ModelResponse::ToolCalls(_) | ModelResponse::EndTurn(_) => {
                    return Ok((response, retries_used));
                }
                ModelResponse::ParseError { raw_text, error } => {
                    println!("⚠️  {}", style("[Syntax Error: Auto-healing...]").yellow());
                    tracing::warn!(%error, "Tool JSON parse error, healing");

                    local_messages.push(json!({"role": "assistant", "content": raw_text}));
                    local_messages.push(json!({
                        "role": "user",
                        "content": format!("Your tool call failed to parse: {}. Please respond with valid JSON.", error)
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
    async fn dispatch_tools(&self, calls: Vec<ToolCall>, stream_tx: Option<UnboundedSender<String>>) -> (Vec<(ToolCall, String)>, usize) {
        let total = calls.len();
        let needs_serial = calls.iter().any(|c| {
            self.tools.get(&c.name).map(|t| t.requires_confirmation()).unwrap_or(false)
        });

        let results = if needs_serial || calls.len() == 1 {
            let mut results = Vec::new();
            for call in calls {
                if let Some(ref tx) = stream_tx {
                    let _ = tx.send(format!("\x1b[SExecuting {}...", call.name));
                }
                let result = match self.tools.dispatch(&call.name, call.args.clone()).await {
                    Ok(r)  => r,
                    Err(e) => format!("Error in tool '{}': {}", call.name, e),
                };
                results.push((call, result));
            }
            results
        } else {
            let msg = format!(
                "{} {} Tool: Executing {} read-only tool(s) concurrently in parallel tasks",
                style("  │  ").color256(240),
                style("⚙").color256(220),
                style(calls.len()).bold()
            );
            if let Some(ref tx) = stream_tx {
                let _ = tx.send(format!("\x1b[T{}", msg));
                let _ = tx.send(format!("\x1b[SRunning {} tools concurrently...", calls.len()));
            } else {
                println!("{}", msg);
            }
            let mut handles = Vec::new();
            for call in &calls {
                let name = call.name.clone();
                let args = call.args.clone();
                let registry = Arc::clone(&self.tools);
                handles.push(tokio::spawn(async move {
                    registry.dispatch(&name, args).await
                        .unwrap_or_else(|e| format!("Error in tool '{}': {}", name, e))
                }));
            }

            let mut results = Vec::new();
            for (call, handle) in calls.into_iter().zip(handles) {
                let result = match handle.await {
                    Ok(r)  => r,
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
        let mut turn_tool_calls: usize   = 0;
        let mut turn_healer_retries: usize = 0;
        let mut turn_compaction_fired  = false;
        let mut turn_steps: usize        = 0;

        let query_embedding = if let Some(ref mut memory_engine) = self.memory {
            memory_engine.embed_text(input).unwrap_or_else(|_| vec![0.0; 384])
        } else {
            vec![0.0; 384]
        };
        let sona_trajectory = self.sona.as_ref().map(|s| s.begin_trajectory(query_embedding));

        self.global_messages.push(json!({"role": "user", "content": input}));
        self.session.append(json!({"event": "user_input", "content": input}))?;

        // 1. Retrieve relevant workspace memories and append them to the system prompt
        let mut final_system_prompt = system_prompt.to_string();
        if let Some(ref mut memory_engine) = self.memory {
            if let Some(ref tx) = stream_tx {
                let _ = tx.send("\x02".to_string());
                let _ = tx.send("\x1b[SQuerying semantic memory...".to_string());
            }
            if let Ok(workspace_dir) = std::env::current_dir() {
                let workspace_str = workspace_dir.to_string_lossy().to_string();
                if let Ok(matches) = memory_engine.search(input, self.sona.as_ref(), &workspace_str, 5) {
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
                        let mut memory_str = String::from("\n\n### RELEVANT MEMORIES (from this workspace):\n");
                        for m in matches {
                            memory_str.push_str(&format!("- {} (similarity: {:.2})\n", m.text, m.score));
                        }
                        final_system_prompt.push_str(&memory_str);
                    } else {
                        if let Some(ref tx) = stream_tx {
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
        }

        let stream_tx_clone = stream_tx.clone();
        let result = self.run_turn_inner(
            &final_system_prompt,
            stream_tx,
            &mut turn_steps,
            &mut turn_tool_calls,
            &mut turn_healer_retries,
            &mut turn_compaction_fired,
        ).await;

        // 2. Commit this turn to memory on success
        if let Ok(ref text) = result {
            if let Some(ref mut memory_engine) = self.memory {
                if let Ok(workspace_dir) = std::env::current_dir() {
                    let workspace_str = workspace_dir.to_string_lossy().to_string();
                    let memory_text = format!("User: {}\nAssistant: {}", input, text);
                    if let Err(e) = memory_engine.insert(&memory_text, None, &workspace_str) {
                        tracing::error!("Failed to save memory: {}", e);
                    }
                }
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
        if let (Some(ref sona_engine), Some(mut trajectory)) = (self.sona.as_ref(), sona_trajectory) {
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

            let mut activations = vec![0.0f32; 384];
            activations[0] = turn_steps as f32;
            activations[1] = turn_healer_retries as f32;
            trajectory.add_step(activations, vec![], final_quality);
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
                    style("·").color256(242), quality_colored
                ));
                if let Some(log_msg) = sona_engine.tick() {
                    let _ = tx.send(format!(
                        "\x1b[T  {} Background loop tick: {}",
                        sona_tag, style(log_msg).color256(117)
                    ));
                }
            } else {
                if let Some(log_msg) = sona_engine.tick() {
                    tracing::info!("{}", log_msg);
                }
            }
        }

        result
    }

    async fn run_turn_inner(
        &mut self,
        system_prompt: &str,
        stream_tx: Option<UnboundedSender<String>>,
        steps_out: &mut usize,
        tool_calls_out: &mut usize,
        healer_retries_out: &mut usize,
        compaction_fired_out: &mut bool,
    ) -> Result<String> {
        for step in 1..=self.max_iterations {
            *steps_out = step;
            tracing::info!(step, max = self.max_iterations, "Agent step");

            if let Some(ref tx) = stream_tx {
                // Compact dim label — no trailing dashes
                let loop_label = style(format!("  · loop {}/{}", step, self.max_iterations)).color256(240);
                let _ = tx.send(format!("\x1b[T{}", loop_label));
                let _ = tx.send(format!("\x1b[SWorking (loop {}/{})...", step, self.max_iterations));
            }

            let (compacted, was_compacted) =
                self.context.compact_if_needed(self.global_messages.clone());
            if was_compacted {
                *compaction_fired_out = true;
                tracing::info!("Context compacted at step {}", step);
                if let Some(ref tx) = stream_tx {
                    let msg = format!(
                        "  Compact: shrinking message history"
                    );
                    let _ = tx.send(format!("\x1b[T{}", msg));
                }
            }
            self.global_messages = compacted;

            let tx = stream_tx.clone();
            let (response, retries) =
                self.get_valid_action(system_prompt, &self.global_messages, tx).await?;
            *healer_retries_out += retries;

            match response {
                ModelResponse::EndTurn(text) => {
                    self.session.append(json!({"event": "end_turn", "content": &text}))?;
                    self.global_messages.push(json!({"role": "assistant", "content": &text}));
                    
                    return Ok(text);
                }

                ModelResponse::ToolCalls(calls) => {
                    let tool_names = calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ");
                    let dispatch_msg = format!(
                        "  {} Tool: dispatching {} → {}",
                        style("⦿").color256(220),
                        style(calls.len()).bold().color256(253),
                        style(&tool_names).cyan().bold()
                    );
                    if let Some(ref tx) = stream_tx {
                        let _ = tx.send(format!("\x1b[T{}", dispatch_msg));
                    } else {
                        println!("{}", dispatch_msg);
                    }

                    let tool_calls_json: Vec<Value> = calls.iter().map(|c| json!({
                        "id": c.id,
                        "type": "function",
                        "function": { "name": c.name, "arguments": c.args.to_string() }
                    })).collect();
                    self.global_messages.push(json!({
                        "role": "assistant",
                        "content": null,
                        "tool_calls": tool_calls_json,
                    }));

                    self.session.append(json!({
                        "event": "tool_calls",
                        "tools": calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>()
                    }))?;

                    let (results, dispatched) = self.dispatch_tools(calls, stream_tx.clone()).await;
                    *tool_calls_out += dispatched;

                    for (call, result) in results {
                        tracing::debug!(tool = %call.name, result = %result, "Tool result");
                        self.session.append(json!({
                            "event": "tool_result",
                            "tool":   call.name,
                            "result": &result,
                        }))?;

                        if let Some(ref tx) = stream_tx {
                            let check = style("✓").color256(46);
                            let status_text = if result.contains("Error") || result.contains("failed") {
                                style("failed").red().bold()
                            } else {
                                style("completed").green()
                            };
                            let msg = format!(
                                "  {} '{}' {} ({} bytes)",
                                check,
                                style(&call.name).cyan(),
                                status_text,
                                result.len()
                            );
                            let _ = tx.send(format!("\x1b[T{}", msg));
                        }

                        // Truncate massively long tool outputs to prevent context window overflow / 500 errors
                        let max_chars = 16_000;
                        let content = if result.len() > max_chars {
                            let head = &result[..max_chars / 2];
                            let tail = &result[result.len() - (max_chars / 2)..];
                            let omitted = result.len() - max_chars;
                            format!("{}\n\n... [{} bytes omitted by context truncator] ...\n\n{}", head, omitted, tail)
                        } else {
                            result.clone()
                        };

                        self.global_messages.push(json!({
                            "role":         "tool",
                            "tool_call_id": call.id,
                            "content":      content,
                        }));
                    }

                    // Close the step box for this tool-use step
                    if let Some(ref tx) = stream_tx {
                        let footer = style("  └──────────────────────────────────────────────────────────").color256(240);
                        let _ = tx.send(format!("\x1b[T{}", footer));
                    }
                }

                ModelResponse::ParseError { .. } => {
                    unreachable!("Healer should have resolved this")
                }
            }
        }

        anyhow::bail!("Stopped after {} iterations without a final answer", self.max_iterations)
    }
}
