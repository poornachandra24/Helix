use serde_json::{json, Value};

pub struct ContextManager {
    /// The maximum approximate token budget for the context window.
    /// Default is 6000 tokens (approx 21,000 characters), which safely fits in most 8k models.
    max_tokens_approx: usize, 
    /// The number of initial messages to permanently pin (usually the first user goal)
    keep_first_n: usize, 
}

impl ContextManager {
    pub fn new() -> Self {
        Self {
            max_tokens_approx: 6000, 
            keep_first_n: 1, 
        }
    }

    /// Fast and robust token approximation (average 3.5 chars per English token)
    fn estimate_tokens(msg: &Value) -> usize {
        let content = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
        (content.len() as f64 / 3.5).ceil() as usize
    }

    /// Performs a production-grade Mid-Window Eviction.
    /// It permanently pins the initial instructions/goal, and rolls the newest messages,
    /// selectively omitting the intermediate (middle) turns if the token limit is exceeded.
    pub fn compact_if_needed(&self, messages: Vec<Value>) -> Vec<Value> {
        let total_tokens: usize = messages.iter().map(Self::estimate_tokens).sum();
        
        // If we are within budget, or we don't even have enough messages to safely drop the middle
        if total_tokens <= self.max_tokens_approx || messages.len() <= self.keep_first_n + 2 {
            return messages;
        }

        // 1. Pin the initial messages
        let mut compacted = messages[0..self.keep_first_n].to_vec();
        
        let mut recent_messages = vec![];
        let mut current_tokens = compacted.iter().map(Self::estimate_tokens).sum::<usize>();
        
        let mut recent_idx = messages.len() - 1;
        let notice_tokens = 30; // tokens for the eviction notice itself
        
        // 2. Roll backwards from the newest messages, collecting until we hit the token budget
        while recent_idx >= self.keep_first_n {
            let msg_tokens = Self::estimate_tokens(&messages[recent_idx]);
            
            if current_tokens + msg_tokens + notice_tokens > self.max_tokens_approx {
                // If even the single most recent message is too massive, we MUST include it 
                // so the agent knows what just happened, but we stop looking further back.
                if recent_messages.is_empty() {
                    recent_messages.push(messages[recent_idx].clone());
                }
                break;
            }
            
            recent_messages.push(messages[recent_idx].clone());
            current_tokens += msg_tokens;
            
            if recent_idx == 0 { break; }
            recent_idx -= 1;
        }
        
        // 3. Restore chronological order for the recent slice
        recent_messages.reverse();

        // 4. Calculate how many intermediate turns were sacrificed
        let omitted_count = messages.len() - self.keep_first_n - recent_messages.len();
        
        if omitted_count > 0 {
            compacted.push(json!({
                "role": "system",
                "content": format!("[System Note: {} intermediate turns were omitted due to context length constraints. The memory was seamlessly compacted. Focus on your original goal and the immediate recent context.]", omitted_count)
            }));
        }
        
        compacted.extend(recent_messages);
        compacted
    }
}
