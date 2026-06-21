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

    /// Estimate tokens for a single chat message `{"role":..., "content":...}`.
    pub fn estimate_message(msg: &Value) -> usize {
        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
        // role is ~1 token; add structural overhead
        1 + Self::estimate_text(content) + Self::MESSAGE_OVERHEAD
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
            model_window: 8_192,
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

    /// Compact the message list if total tokens exceed the available budget.
    /// Returns `(compacted_messages, was_compacted)`.
    pub fn compact_if_needed(&self, messages: Vec<Value>) -> (Vec<Value>, bool) {
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

        // Pin the first N messages (original goal)
        let mut compacted = messages[..self.keep_first_n].to_vec();
        let mut recent: Vec<Value> = Vec::new();
        let mut used: usize = compacted.iter().map(TokenEstimator::estimate_message).sum();
        let notice_overhead = 30;

        let mut idx = messages.len() - 1;
        loop {
            let cost = TokenEstimator::estimate_message(&messages[idx]);
            if used + cost + notice_overhead > available {
                if recent.is_empty() {
                    recent.push(messages[idx].clone()); // always keep at least the latest
                }
                break;
            }
            recent.push(messages[idx].clone());
            used += cost;
            if idx == self.keep_first_n {
                break;
            }
            idx -= 1;
        }

        recent.reverse();

        let omitted = messages.len() - self.keep_first_n - recent.len();
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
        tracing::info!(omitted, "Context compacted");
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
}
