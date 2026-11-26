use crate::tools::ToolDescriptor;
use serde_json::{Value, json};
use std::sync::OnceLock;
use tiktoken_rs::{CoreBPE, cl100k_base};

static BPE: OnceLock<Option<CoreBPE>> = OnceLock::new();

fn get_bpe() -> Option<&'static CoreBPE> {
    BPE.get_or_init(|| cl100k_base().ok()).as_ref()
}

// ──────────────────────────────────────────────
// Token Estimation
// ──────────────────────────────────────────────

pub struct TokenEstimator;

impl TokenEstimator {
    /// Fixed overhead per chat message (role field + JSON structural tokens).
    /// Matches cl100k_base empirical measurements (OpenAI cookbook).
    const MESSAGE_OVERHEAD: usize = 4;

    /// Estimate tokens for a text string using cl100k_base tokenizer.
    pub fn estimate_text(text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        if let Some(bpe) = get_bpe() {
            bpe.encode_with_special_tokens(text).len()
        } else {
            // Heuristic fallback: count structural chars to detect code/JSON density
            let structural = text
                .chars()
                .filter(|c| matches!(c, '{' | '}' | '(' | ')' | '[' | ']' | ';' | '='))
                .count();
            let ratio = structural as f64 / text.len() as f64;

            let chars_per_token = if ratio > 0.08 {
                2.8 // code / JSON — denser than prose
            } else {
                4.0 // natural language
            };
            (text.len() as f64 / chars_per_token).ceil() as usize
        }
    }

    /// Estimate tokens for a single chat message `{"role":..., "content":..., "tool_calls":...}`.
    pub fn estimate_message(msg: &Value) -> usize {
        let mut tokens = 1 + Self::MESSAGE_OVERHEAD;

        if let Some(content) = msg.get("content") {
            if let Some(s) = content.as_str() {
                tokens += Self::estimate_text(s);
            } else if let Some(arr) = content.as_array() {
                for item in arr {
                    if item.get("type").and_then(|t| t.as_str()) == Some("image_url") {
                        // Vision models charge ~85-1600 tokens per image regardless of base64 length
                        tokens += 1024;
                    } else if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        tokens += Self::estimate_text(text);
                    } else if let Some(s) = item.as_str() {
                        tokens += Self::estimate_text(s);
                    } else {
                        tokens += Self::estimate_text(&item.to_string());
                    }
                }
            } else if !content.is_null() {
                tokens += Self::estimate_text(&content.to_string());
            }
        }

        if let Some(tool_calls) = msg.get("tool_calls").and_then(|tc| tc.as_array()) {
            for call in tool_calls {
                tokens += 8; // structural overhead for tool call envelope
                if let Some(id) = call.get("id").and_then(|id| id.as_str()) {
                    tokens += Self::estimate_text(id);
                }
                if let Some(func) = call.get("function") {
                    if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                        tokens += Self::estimate_text(name);
                    }
                    if let Some(args) = func.get("arguments").and_then(|a| a.as_str()) {
                        tokens += Self::estimate_text(args);
                    }
                }
            }
        }

        if let Some(tool_call_id) = msg.get("tool_call_id").and_then(|id| id.as_str()) {
            tokens += Self::estimate_text(tool_call_id) + 4;
        }

        if let Some(reasoning) = msg.get("reasoning_content").and_then(|r| r.as_str()) {
            tokens += Self::estimate_text(reasoning);
        }

        tokens
    }

    /// Estimate the token cost of injecting tool schemas into every API call.
    /// Called once at tool-registration time and cached in `ContextBudget`.
    pub fn estimate_tool_descriptors(tools: &[ToolDescriptor]) -> usize {
        tools
            .iter()
            .map(|t| {
                let schema_str = t.parameters.to_string();
                Self::estimate_text(&t.name)
                    + Self::estimate_text(&t.description)
                    + Self::estimate_text(&schema_str)
                    + 12 // per-tool structural JSON overhead
            })
            .sum()
    }

    /// Estimate token cost of the system prompt (injected once per call).
    pub fn estimate_system_prompt(prompt: &str) -> usize {
        Self::estimate_text(prompt) + Self::MESSAGE_OVERHEAD
    }
}

// ──────────────────────────────────────────────
// Context Budget
// ──────────────────────────────────────────────

/// Tracks the token "accounting" for a session.
/// All non-message costs are pre-measured so the compaction logic
/// only needs to work with the message slice.
#[derive(Debug, Clone)]
pub struct ContextBudget {
    /// Total model context window size (tokens).
    pub model_window: usize,
    /// Pre-measured cost of the system prompt.
    pub system_prompt_tokens: usize,
    /// Pre-measured cost of all registered tool schemas.
    pub tool_descriptor_tokens: usize,
    /// Tokens reserved for the model's own response generation.
    pub response_headroom: usize,
}

impl ContextBudget {
    pub fn new(
        model_window: usize,
        system_prompt: &str,
        tools: &[ToolDescriptor],
        response_headroom: usize,
    ) -> Self {
        Self {
            model_window,
            system_prompt_tokens: TokenEstimator::estimate_system_prompt(system_prompt),
            tool_descriptor_tokens: TokenEstimator::estimate_tool_descriptors(tools),
            response_headroom,
        }
    }

    /// Tokens available for the conversation message history.
    pub fn available_for_messages(&self) -> usize {
        self.model_window
            .saturating_sub(self.system_prompt_tokens)
            .saturating_sub(self.tool_descriptor_tokens)
            .saturating_sub(self.response_headroom)
    }
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            model_window: 16_384,
            system_prompt_tokens: 0,
            tool_descriptor_tokens: 0,
            response_headroom: 2_048,
        }
    }
}

// ──────────────────────────────────────────────
// Context Manager
// ──────────────────────────────────────────────

pub struct ContextManager {
    pub budget: ContextBudget,
    /// Number of leading messages to permanently pin (the user's original goal).
    keep_first_n: usize,
}

impl ContextManager {
    pub fn new(budget: ContextBudget) -> Self {
        Self {
            budget,
            keep_first_n: 1,
        }
    }

    /// Truncate large tool outputs in older turns (older than the last keep_recent_count messages).
    /// Returns (pruned_messages, total_chars_saved).
    fn prune_historical_tool_outputs(
        &self,
        mut messages: Vec<Value>,
        keep_recent_count: usize,
    ) -> (Vec<Value>, usize) {
        let len = messages.len();
        if len <= self.keep_first_n + keep_recent_count {
            return (messages, 0);
        }

        let prune_end_idx = len.saturating_sub(keep_recent_count);
        let prune_start_idx = self.keep_first_n;
        let mut chars_saved = 0;

        for msg in messages
            .iter_mut()
            .take(prune_end_idx)
            .skip(prune_start_idx)
        {
            let is_tool = msg.get("role").and_then(|r| r.as_str()) == Some("tool");
            if !is_tool {
                continue;
            }

            if let Some(content_val) = msg.get_mut("content")
                && let Some(text) = content_val.as_str()
            {
                const TRUNCATE_THRESHOLD: usize = 600;
                const HEAD_CHARS: usize = 250;
                const TAIL_CHARS: usize = 150;

                if text.len() > TRUNCATE_THRESHOLD {
                    let original_len = text.len();
                    let head_end = text
                        .char_indices()
                        .nth(HEAD_CHARS)
                        .map(|(idx, _)| idx)
                        .unwrap_or(text.len());
                    let head = &text[..head_end];
                    let tail_start = text
                        .char_indices()
                        .rev()
                        .nth(TAIL_CHARS)
                        .map(|(idx, _)| idx)
                        .unwrap_or(0);
                    let tail = &text[tail_start..];

                    let pruned_text = format!(
                        "{}\n\n... [helix: {} chars truncated to conserve context window] ...\n\n{}",
                        head.trim_end(),
                        original_len - (head.len() + tail.len()),
                        tail.trim_start()
                    );
                    chars_saved += original_len.saturating_sub(pruned_text.len());
                    *content_val = json!(pruned_text);
                }
            }
        }

        (messages, chars_saved)
    }

    /// Enforce LLM API sequence invariants:
    /// 1. Strip leading or orphaned tool responses.
    /// 2. If an assistant message calls tools, ensure all its tool responses are retained;
    ///    otherwise discard the incomplete assistant-tool sequence as an atomic unit.
    fn repair_tool_call_invariants(messages: &mut Vec<Value>) {
        // Step 1: Strip leading orphaned tool responses
        while let Some(first) = messages.first() {
            if first.get("role").and_then(|r| r.as_str()) == Some("tool") {
                messages.remove(0);
            } else {
                break;
            }
        }

        // Step 2: Validate tool calls and tool responses
        let mut i = 0;
        let mut active_call_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        while i < messages.len() {
            let role = messages[i]
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("");

            if role == "assistant" {
                if let Some(tool_calls) = messages[i].get("tool_calls").and_then(|tc| tc.as_array())
                    && !tool_calls.is_empty()
                {
                    let call_ids: std::collections::HashSet<String> = tool_calls
                        .iter()
                        .filter_map(|c| c["id"].as_str().map(String::from))
                        .collect();

                    // Count how many consecutive tool responses follow and collect their IDs
                    let mut answered_ids = std::collections::HashSet::new();
                    let mut tool_count = 0;
                    for other_msg in messages.iter().skip(i + 1) {
                        if other_msg.get("role").and_then(|r| r.as_str()) == Some("tool") {
                            if let Some(id) = other_msg["tool_call_id"].as_str() {
                                answered_ids.insert(id.to_string());
                            }
                            tool_count += 1;
                        } else {
                            break;
                        }
                    }

                    // If all tool calls are answered, accept them
                    if call_ids.is_subset(&answered_ids) {
                        active_call_ids = answered_ids;
                        i += 1;
                        continue;
                    } else {
                        // Incomplete tool calls: remove assistant and any partial tool responses
                        messages.remove(i);
                        for _ in 0..tool_count {
                            if i < messages.len()
                                && messages[i].get("role").and_then(|r| r.as_str()) == Some("tool")
                            {
                                messages.remove(i);
                            }
                        }
                        continue;
                    }
                }
                active_call_ids.clear();
                i += 1;
            } else if role == "tool" {
                let id = messages[i]
                    .get("tool_call_id")
                    .and_then(|id| id.as_str())
                    .unwrap_or("");
                if active_call_ids.contains(id) {
                    i += 1;
                } else {
                    // Orphaned tool response with no active assistant caller
                    messages.remove(i);
                }
            } else {
                // user / system message
                active_call_ids.clear();
                i += 1;
            }
        }

        // Final check: strip any leading tool response that might have been left
        while let Some(first) = messages.first() {
            if first.get("role").and_then(|r| r.as_str()) == Some("tool") {
                messages.remove(0);
            } else {
                break;
            }
        }
    }

    /// Compact the message list if total tokens exceed the available budget.
    /// Stage 1: Prunes oversized historical tool responses in-place.
    /// Stage 2: Turn-aligned compaction with hysteresis if still over budget.
    /// Returns `(compacted_messages, was_compacted)`.
    pub fn compact_if_needed(&self, mut messages: Vec<Value>) -> (Vec<Value>, bool) {
        let available = self.budget.available_for_messages();
        let total: usize = messages.iter().map(TokenEstimator::estimate_message).sum();

        tracing::debug!(
            total_tokens = total,
            available = available,
            messages = messages.len(),
            "Context budget check"
        );

        if total <= available || messages.len() <= self.keep_first_n + 2 {
            return (messages, false);
        }

        // ── Stage 1: Prune oversized historical tool responses in-place ──────────
        let (pruned_messages, chars_saved) = self.prune_historical_tool_outputs(messages, 4);
        messages = pruned_messages;

        let pruned_total: usize = messages.iter().map(TokenEstimator::estimate_message).sum();
        if chars_saved > 0 {
            tracing::info!(
                chars_saved,
                previous_tokens = total,
                pruned_tokens = pruned_total,
                available,
                "Stage 1: Pruned historical tool outputs"
            );
        }

        // If Stage 1 brought us within budget, we are done! No messages dropped.
        if pruned_total <= available {
            return (messages, chars_saved > 0);
        }

        // ── Stage 2: Turn-aligned compaction with hysteresis ─────────────────────
        // Apply 20% hysteresis headroom so compaction doesn't trigger on every single turn.
        let target_budget = (available as f64 * 0.80) as usize;
        let target_budget = target_budget.max(available / 2);

        // Pin the first N messages (original goal)
        let mut compacted = messages[..self.keep_first_n].to_vec();
        let used_pinned: usize = compacted.iter().map(TokenEstimator::estimate_message).sum();
        let notice_overhead = 30;

        let mut recent: Vec<Value> = Vec::new();
        let mut used = used_pinned;

        // Traverse backwards from the latest message to fit within target_budget
        let mut idx = messages.len() - 1;
        loop {
            let cost = TokenEstimator::estimate_message(&messages[idx]);
            if used + cost + notice_overhead > target_budget && !recent.is_empty() {
                break;
            }
            recent.push(messages[idx].clone());
            used += cost;
            if idx <= self.keep_first_n {
                break;
            }
            idx -= 1;
        }

        recent.reverse();

        // Enforce LLM API sequence invariants
        Self::repair_tool_call_invariants(&mut recent);

        if recent.is_empty() && messages.len() > self.keep_first_n {
            // Guarantee at least the last non-tool message is kept
            if let Some(last_non_tool) = messages[self.keep_first_n..]
                .iter()
                .rfind(|m| m.get("role").and_then(|r| r.as_str()) != Some("tool"))
            {
                recent.push(last_non_tool.clone());
            } else if let Some(last) = messages.last() {
                recent.push(last.clone());
            }
        }

        let omitted = messages
            .len()
            .saturating_sub(self.keep_first_n + recent.len());
        if omitted > 0 {
            compacted.push(json!({
                "role": "system",
                "content": format!(
                    "[System Note: {} intermediate turns were omitted due to context window constraints ({} tokens available for messages). The conversation was seamlessly compacted. Maintain focus on your original goal.]",
                    omitted, available
                )
            }));
        }

        compacted.extend(recent);
        tracing::info!(omitted, "Context compacted (Stage 2)");
        (compacted, true)
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new(ContextBudget::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_estimator_exact() {
        // "hello" = 1 token, " world" = 1 token (cl100k)
        assert_eq!(TokenEstimator::estimate_text("hello world"), 2);
        assert_eq!(TokenEstimator::estimate_text(""), 0);

        let msg = json!({
            "role": "user",
            "content": "hello world"
        });
        // 1 + 2 + 4 = 7
        assert_eq!(TokenEstimator::estimate_message(&msg), 7);
    }

    #[test]
    fn test_context_manager_compaction() {
        let budget = ContextBudget {
            model_window: 25,
            system_prompt_tokens: 0,
            tool_descriptor_tokens: 0,
            response_headroom: 5,
        };
        // Available for messages = 20 tokens.
        let manager = ContextManager::new(budget);

        // Keep first N is 1.
        let msg_goal = json!({"role": "user", "content": "goal"}); // "goal" is 1 token => total = 1+1+4 = 6 tokens
        let msg_1 = json!({"role": "assistant", "content": "reply one"}); // "reply" " one" is 2 tokens => total = 1+2+4 = 7 tokens
        let msg_2 = json!({"role": "user", "content": "reply two"}); // "reply" " two" is 2 tokens => total = 1+2+4 = 7 tokens
        let msg_3 = json!({"role": "assistant", "content": "reply three"}); // "reply" " three" is 2 tokens => total = 1+2+4 = 7 tokens
        let msg_4 = json!({"role": "user", "content": "reply four"}); // "reply" " four" is 2 tokens => total = 1+2+4 = 7 tokens

        // Total for [goal, msg_1, msg_2, msg_3, msg_4] = 6 + 7 + 7 + 7 + 7 = 34 tokens (> 20 available)
        let messages = vec![
            msg_goal.clone(),
            msg_1.clone(),
            msg_2.clone(),
            msg_3.clone(),
            msg_4.clone(),
        ];
        let (compacted, was_compacted) = manager.compact_if_needed(messages);

        assert!(was_compacted);
        // Should keep the first (goal) and the last (msg_4), plus a system notice message
        assert_eq!(compacted.len(), 3);
        assert_eq!(compacted[0]["content"], "goal");
        assert!(
            compacted[1]["content"]
                .as_str()
                .unwrap()
                .contains("omitted")
        );
        assert_eq!(compacted[2]["content"], "reply four");
    }

    #[test]
    fn test_token_estimator_tool_calls_and_responses() {
        let msg_with_tools = json!({
            "role": "assistant",
            "content": null,
            "tool_calls": [
                {
                    "id": "call_abc123",
                    "type": "function",
                    "function": {
                        "name": "str_replace",
                        "arguments": "{\"path\":\"src/main.rs\",\"old_str\":\"foo\",\"new_str\":\"bar\"}"
                    }
                }
            ]
        });
        let tokens = TokenEstimator::estimate_message(&msg_with_tools);
        // Previously this was estimated as 5 tokens. Now it measures the full tool call payload (> 20 tokens).
        assert!(tokens > 20);

        let tool_resp = json!({
            "role": "tool",
            "tool_call_id": "call_abc123",
            "content": "successfully replaced"
        });
        let resp_tokens = TokenEstimator::estimate_message(&tool_resp);
        assert!(resp_tokens >= 8);
    }

    #[test]
    fn test_stage1_tool_output_pruning() {
        // Build a history where an older tool response has a massive output (~2000 chars)
        let large_tool_output = "error log failure trace dump line item\n".repeat(50);
        let budget = ContextBudget {
            model_window: 180,
            system_prompt_tokens: 0,
            tool_descriptor_tokens: 0,
            response_headroom: 10,
        };
        // Available tokens = 170
        let manager = ContextManager::new(budget);

        let messages = vec![
            json!({"role": "user", "content": "goal"}),
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "read_file", "arguments": "{}"}}]
            }),
            json!({
                "role": "tool",
                "tool_call_id": "c1",
                "content": large_tool_output
            }),
            json!({"role": "user", "content": "next step"}),
            json!({"role": "assistant", "content": "working"}),
            json!({"role": "user", "content": "status"}),
            json!({"role": "assistant", "content": "done"}),
        ];

        let (compacted, was_compacted) = manager.compact_if_needed(messages);
        assert!(was_compacted);

        // Stage 1 should have pruned the large tool output (index 2) without dropping any messages
        assert_eq!(compacted.len(), 7);
        let tool_msg = &compacted[2];
        assert_eq!(tool_msg["role"], "tool");
        let content = tool_msg["content"].as_str().unwrap();
        assert!(content.contains("[helix:"));
        assert!(content.contains("chars truncated to conserve context window"));
    }

    #[test]
    fn test_repair_tool_call_invariants_removes_orphans() {
        let mut messages = vec![
            // Orphaned tool response at the start
            json!({"role": "tool", "tool_call_id": "orphan1", "content": "result"}),
            // Valid assistant + tool
            json!({
                "role": "assistant",
                "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "bash", "arguments": "{}"}}]
            }),
            json!({"role": "tool", "tool_call_id": "c1", "content": "ok"}),
            // Broken assistant with missing tool response
            json!({
                "role": "assistant",
                "tool_calls": [{"id": "c2", "type": "function", "function": {"name": "bash", "arguments": "{}"}}]
            }),
            json!({"role": "user", "content": "hello"}),
        ];

        ContextManager::repair_tool_call_invariants(&mut messages);

        // Orphaned tool response at index 0 should be gone.
        // Broken assistant with call "c2" (which had no tool response) should be gone.
        // Remaining should be the valid pair and user message.
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0]["role"], "assistant");
        assert_eq!(messages[1]["role"], "tool");
        assert_eq!(messages[2]["role"], "user");
    }

    #[test]
    fn test_stage2_turn_omission_with_hysteresis() {
        let budget = ContextBudget {
            model_window: 40,
            system_prompt_tokens: 0,
            tool_descriptor_tokens: 0,
            response_headroom: 5,
        };
        // Available tokens = 35. Target budget with 80% hysteresis = 28 tokens.
        let manager = ContextManager::new(budget);

        let messages = vec![
            json!({"role": "user", "content": "goal"}),
            json!({"role": "assistant", "content": "step 1"}),
            json!({"role": "user", "content": "step 2"}),
            json!({"role": "assistant", "content": "step 3"}),
            json!({"role": "user", "content": "step 4"}),
            json!({"role": "assistant", "content": "step 5"}),
        ];

        let (compacted, was_compacted) = manager.compact_if_needed(messages);
        assert!(was_compacted);

        // First message is goal
        assert_eq!(compacted[0]["content"], "goal");
        // Intermediate system note
        assert!(
            compacted[1]["content"]
                .as_str()
                .unwrap()
                .contains("intermediate turns were omitted")
        );
        // Final message is preserved
        assert_eq!(compacted.last().unwrap()["content"], "step 5");
    }

    #[test]
    fn test_token_estimator_image_url() {
        let huge_b64 = "A".repeat(100_000);
        let msg = json!({
            "role": "user",
            "content": [
                { "type": "text", "text": "describe" },
                { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{}", huge_b64) } }
            ]
        });
        let est = TokenEstimator::estimate_message(&msg);
        assert!(
            est < 2000,
            "Image token estimate should not count raw base64 bytes, got {}",
            est
        );
        assert!(est >= 1024);
    }
}
