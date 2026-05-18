use serde_json::{json, Value};
use crate::tools::ToolDescriptor;

// ──────────────────────────────────────────────
// Token Estimation
// ──────────────────────────────────────────────

pub struct TokenEstimator;

impl TokenEstimator {
    /// Fixed overhead per chat message (role field + JSON structural tokens).
    /// Matches cl100k_base empirical measurements (OpenAI cookbook).
    const MESSAGE_OVERHEAD: usize = 4;

    /// Estimate tokens for a text string, adjusting ratio by content type.
    pub fn estimate_text(text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        // Heuristic: count structural chars to detect code/JSON density
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

    /// Estimate tokens for a single chat message `{"role":..., "content":...}`.
    pub fn estimate_message(msg: &Value) -> usize {
        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
        // role is ~1 token; add structural overhead
        1 + Self::estimate_text(content) + Self::MESSAGE_OVERHEAD
    }

    /// Estimate the token cost of injecting tool schemas into every API call.
    /// Called once at tool-registration time and cached in `ContextBudget`.
    pub fn estimate_tool_descriptors(tools: &[ToolDescriptor]) -> usize {
        tools.iter().map(|t| {
            let schema_str = t.parameters.to_string();
            Self::estimate_text(&t.name)
                + Self::estimate_text(&t.description)
                + Self::estimate_text(&schema_str)
                + 12 // per-tool structural JSON overhead
        }).sum()
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
        Self { budget, keep_first_n: 1 }
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
            if idx == self.keep_first_n { break; }
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
