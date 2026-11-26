//! Dynamic context window resolver.
//!
//! Resolution order (first success wins):
//!   1. `config.context_window_override` — user-set manual override
//!   2. Ollama native `/api/show` — for local/cloud Ollama providers
//!   3. Provider's own `/v1/models/{id}` — works for OpenAI and compatible providers
//!   4. OpenRouter public model catalogue — covers 1 000+ models, no auth needed
//!   5. Conservative fallback → 8 192

use reqwest::Client;
use serde_json::Value;

use crate::config::{ApiFormat, AppConfig};

const FALLBACK_CONTEXT_WINDOW: usize = 16_384;

// ──────────────────────────────────────────────
// Offline Model Knowledge Base
// ──────────────────────────────────────────────

const KNOWN_MODELS: &[(&str, usize)] = &[
    // Anthropic Claude
    ("claude-3-7-sonnet", 200_000),
    ("claude-3.7-sonnet", 200_000),
    ("claude-3-5-sonnet", 200_000),
    ("claude-3.5-sonnet", 200_000),
    ("claude-3-5-haiku", 200_000),
    ("claude-3.5-haiku", 200_000),
    ("claude-3-opus", 200_000),
    ("claude-3-haiku", 200_000),
    ("claude-2.1", 200_000),
    ("claude-2", 100_000),
    // OpenAI
    ("gpt-4.5", 128_000),
    ("gpt-4o-mini", 128_000),
    ("gpt-4o", 128_000),
    ("gpt-4-turbo", 128_000),
    ("o1-mini", 128_000),
    ("o1-preview", 128_000),
    ("o1", 200_000),
    ("o3-mini", 200_000),
    ("o3", 200_000),
    ("chatgpt-4o", 128_000),
    ("gpt-4-32k", 32_768),
    ("gpt-4", 8_192),
    ("gpt-3.5-turbo-16k", 16_384),
    ("gpt-3.5-turbo", 16_384),
    // DeepSeek
    ("deepseek-r1", 128_000),
    ("deepseek-reasoner", 128_000),
    ("deepseek-chat", 128_000),
    ("deepseek-v3", 128_000),
    ("deepseek-coder-v2", 128_000),
    ("deepseek-coder", 64_000),
    // Google Gemini
    ("gemini-2.0-flash", 1_048_576),
    ("gemini-2.0-pro", 2_097_152),
    ("gemini-1.5-pro", 2_097_152),
    ("gemini-1.5-flash-8b", 1_048_576),
    ("gemini-1.5-flash", 1_048_576),
    ("gemini-pro", 32_768),
    // Meta Llama
    ("llama-3.3", 131_072),
    ("llama3.3", 131_072),
    ("llama-3.2", 131_072),
    ("llama3.2", 131_072),
    ("llama-3.1", 131_072),
    ("llama3.1", 131_072),
    ("llama-3", 8_192),
    ("llama3", 8_192),
    ("llama-2", 4_096),
    ("llama2", 4_096),
    ("code-llama", 16_384),
    ("codellama", 16_384),
    // Qwen
    ("qwen2.5-coder", 131_072),
    ("qwen2.5", 131_072),
    ("qwen-2.5", 131_072),
    ("qwen2", 32_768),
    ("qwq", 32_768),
    // Mistral
    ("codestral", 32_768),
    ("mistral-large", 128_000),
    ("mistral-small", 32_768),
    ("mistral-nemo", 128_000),
    ("pixtral", 128_000),
    ("mixtral-8x22b", 65_536),
    ("mixtral-8x7b", 32_768),
    ("mistral-7b", 32_768),
    // Cohere
    ("command-r-plus", 128_000),
    ("command-r+", 128_000),
    ("command-r", 128_000),
    // Gemma & Google Open Models
    ("gemma4", 131_072),
    ("gemma-4", 131_072),
    ("gemma3", 131_072),
    ("gemma-3", 131_072),
    ("gemma2", 8_192),
    ("gemma-2", 8_192),
    // Ollama Cloud & OSS Models
    ("gpt-oss", 131_072),
    // xAI Grok
    ("grok-2", 131_072),
    ("grok", 131_072),
];

/// Fast offline lookup for known model names and variations.
pub fn lookup_known_model(model: &str) -> Option<usize> {
    let clean = model.to_lowercase().replace('_', "-");
    let model_name = clean.split('/').next_back().unwrap_or(&clean);
    let base_name = model_name.split(':').next().unwrap_or(model_name);

    for (pattern, ctx) in KNOWN_MODELS {
        if model_name.contains(pattern) || base_name.contains(pattern) {
            return Some(*ctx);
        }
    }
    None
}

/// Detect explicit context window size patterns from model naming conventions.
pub fn parse_context_from_name(model: &str) -> Option<usize> {
    let lower = model.to_lowercase();
    if lower.contains("1m") || lower.contains("1000k") {
        return Some(1_000_000);
    }
    if lower.contains("200k") {
        return Some(200_000);
    }
    if lower.contains("128k") || lower.contains("131k") {
        return Some(131_072);
    }
    if lower.contains("64k") || lower.contains("65k") {
        return Some(65_536);
    }
    if lower.contains("32k") {
        return Some(32_768);
    }
    if lower.contains("16k") {
        return Some(16_384);
    }
    if lower.contains("8k") {
        return Some(8_192);
    }
    if lower.contains("4k") {
        return Some(4_096);
    }
    None
}

/// Dynamically determine response headroom based on model size and reasoning level.
pub fn resolve_headroom(
    model_window: usize,
    thinking_level: Option<&str>,
    model_name: &str,
) -> usize {
    let model_lower = model_name.to_lowercase();
    let is_reasoning_model = model_lower.contains("o1")
        || model_lower.contains("o3")
        || model_lower.contains("r1")
        || model_lower.contains("reasoner")
        || model_lower.contains("qwq");

    let is_thinking_active = thinking_level
        .map(|l| {
            let s = l.to_lowercase();
            s != "off" && s != "disabled" && s != "0"
        })
        .unwrap_or(false);

    let base_headroom = if is_reasoning_model || is_thinking_active {
        if let Some(level) = thinking_level {
            if let Ok(budget) = level.parse::<usize>() {
                budget + 4096
            } else if level.eq_ignore_ascii_case("high") {
                16_384
            } else if level.eq_ignore_ascii_case("medium") {
                8_192
            } else {
                4_096
            }
        } else {
            // Default for reasoning models
            12_288
        }
    } else if model_window >= 100_000 {
        8_192
    } else if model_window >= 32_000 {
        4_096
    } else if model_window >= 16_000 {
        2_048
    } else {
        (model_window / 4).clamp(512, 1024)
    };

    // Ensure headroom never consumes more than 30% of total context window
    base_headroom.min(model_window * 3 / 10).max(512)
}

// ──────────────────────────────────────────────
// Public entry point
// ──────────────────────────────────────────────

/// Resolve the context window for the currently active model.
/// Returns the override if set, otherwise probes offline catalog, provider, and external sources.
pub async fn resolve_context_window(config: &AppConfig, client: &Client) -> usize {
    if let Some(override_val) = config.context_window_override {
        tracing::info!(
            context_window = override_val,
            "Using manual context_window_override"
        );
        return override_val;
    }

    let model = &config.active_model;

    // Strategy 1 — Authoritative offline knowledge base (Instant, zero-network, sandbox-safe)
    if let Some(w) = lookup_known_model(model) {
        tracing::info!(
            context_window = w,
            source = "known models catalog",
            model = %model,
            "Context window resolved from offline catalog"
        );
        return w;
    }

    // Strategy 2 — Name pattern heuristic (e.g. "my-model-128k")
    if let Some(w) = parse_context_from_name(model) {
        tracing::info!(
            context_window = w,
            source = "model name pattern",
            model = %model,
            "Context window resolved from model name pattern"
        );
        return w;
    }

    let provider = config.resolve_best_provider(client).await;

    // Strategy 3 — Ollama native /api/show (passes auth for cloud models)
    if provider.api_format == ApiFormat::OllamaNative
        && let Some(w) = query_ollama_show(
            &provider.base_url,
            model,
            provider.api_key.as_deref(), // ← auth required for Ollama Cloud
            client,
        )
        .await
    {
        tracing::info!(
            context_window = w,
            source = "ollama /api/show",
            "Context window resolved"
        );
        return w;
    }

    // Strategy 4 — Provider's own /v1/models/{id} (OpenAI and compatible)
    if provider.api_format == ApiFormat::OpenAiCompatible
        && let Some(w) = query_openai_models(
            &provider.base_url,
            model,
            provider.api_key.as_deref(),
            client,
        )
        .await
    {
        tracing::info!(
            context_window = w,
            source = "provider /v1/models",
            "Context window resolved"
        );
        return w;
    }

    // Strategy 5 — OpenRouter public catalogue (free, no auth, covers 1000+ models)
    if let Some(w) = query_openrouter_catalogue(model, client).await {
        tracing::info!(
            context_window = w,
            source = "openrouter catalogue",
            "Context window resolved"
        );
        return w;
    }

    // Strategy 6 — Family-based heuristic fallback
    let family_fallback = if model.to_lowercase().contains("claude")
        || model.to_lowercase().contains("gpt-4")
        || model.to_lowercase().contains("deepseek")
        || model.to_lowercase().contains("qwen")
    {
        65_536
    } else {
        FALLBACK_CONTEXT_WINDOW
    };

    tracing::warn!(
        model = %model,
        fallback = family_fallback,
        "Could not determine context window from any source; using fallback"
    );
    family_fallback
}

// ──────────────────────────────────────────────
// Strategy 1: Ollama /api/show
// ──────────────────────────────────────────────

/// Query Ollama's `/api/show` endpoint.
/// The response's `modelinfo` map contains architecture-specific keys like
/// `llama.context_length`, `gemma.context_length`, etc.
/// The `parameters` string may also carry `num_ctx <value>`.
async fn query_ollama_show(
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    client: &Client,
) -> Option<usize> {
    let base = base_url.trim_end_matches('/');
    let url = if base.ends_with("/api/show") {
        base.to_string()
    } else if base.ends_with("/api") {
        format!("{}/show", base)
    } else {
        format!("{}/api/show", base)
    };
    let body = serde_json::json!({ "model": model });

    let mut req = client
        .post(&url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(10));

    // Ollama Cloud requires Bearer auth; local Ollama ignores it safely
    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }

    let resp = req.send().await.ok()?;

    if !resp.status().is_success() {
        tracing::debug!("Ollama /api/show returned {}", resp.status());
        return None;
    }

    let json: Value = resp.json().await.ok()?;

    // modelinfo keys: "llama.context_length", "gemma.context_length", etc.
    if let Some(info) = json.get("modelinfo").and_then(|v| v.as_object()) {
        for (key, val) in info {
            if (key.ends_with(".context_length") || key == "context_length")
                && let Some(n) = val.as_u64()
            {
                return Some(n as usize);
            }
        }
    }

    // Fallback: parse the parameters string for `num_ctx <value>`
    if let Some(params) = json.get("parameters").and_then(|v| v.as_str()) {
        for line in params.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 2
                && parts[0] == "num_ctx"
                && let Ok(n) = parts[1].parse::<usize>()
            {
                return Some(n);
            }
        }
    }

    tracing::debug!("Ollama /api/show did not contain a context length field");
    None
}

// ──────────────────────────────────────────────
// Strategy 2: Provider /v1/models/{id}
// ──────────────────────────────────────────────

/// Query `GET {base_url}/models/{model_id}`.
/// OpenAI returns a `context_window` integer on model objects.
/// Many OpenAI-compatible providers (Together, Fireworks, etc.) mirror this field.
async fn query_openai_models(
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    client: &Client,
) -> Option<usize> {
    let base = base_url.trim_end_matches('/');
    let url = if base.ends_with("/models") {
        format!("{}/{}", base, model)
    } else {
        format!("{}/models/{}", base, model)
    };

    let mut req = client.get(&url).timeout(std::time::Duration::from_secs(10));

    if let Some(key) = api_key {
        req = req.bearer_auth(key);
    }

    let resp = req.send().await.ok()?;

    if !resp.status().is_success() {
        tracing::debug!("Provider /v1/models/{} returned {}", model, resp.status());
        return None;
    }

    let json: Value = resp.json().await.ok()?;

    // OpenAI field: "context_window"
    if let Some(n) = json.get("context_window").and_then(|v| v.as_u64()) {
        return Some(n as usize);
    }
    // Some providers use "max_context_length" or "max_tokens"
    for key in &["max_context_length", "max_tokens", "context_length"] {
        if let Some(n) = json.get(*key).and_then(|v| v.as_u64()) {
            return Some(n as usize);
        }
    }

    tracing::debug!(
        "Provider model object for '{}' had no context window field",
        model
    );
    None
}

// ──────────────────────────────────────────────
// Strategy 3: OpenRouter public catalogue
// ──────────────────────────────────────────────

/// Query OpenRouter's public models list (no auth required).
/// Matches the active model name against the catalogue entries by fuzzy ID matching.
///
/// Endpoint: `GET https://openrouter.ai/api/v1/models`
/// Response: `{ "data": [ { "id": "...", "context_length": 131072, ... } ] }`
async fn query_openrouter_catalogue(model: &str, client: &Client) -> Option<usize> {
    let url = "https://openrouter.ai/api/v1/models";

    let resp = client
        .get(url)
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        tracing::debug!("OpenRouter catalogue returned {}", resp.status());
        return None;
    }

    let json: Value = resp.json().await.ok()?;
    let models = json.get("data")?.as_array()?;

    // Strip cloud-provider suffixes before matching:
    // "gpt-oss:120b-cloud" → also try "gpt-oss:120b" and "gpt-oss"
    let model_lower = model.to_lowercase();
    let model_no_cloud = model_lower.trim_end_matches("-cloud").to_string();
    let model_base = model_no_cloud
        .split(':')
        .next()
        .unwrap_or(&model_no_cloud)
        .to_string();

    let candidates: [&str; 3] = [
        model_lower.as_str(),
        model_no_cloud.as_str(),
        model_base.as_str(),
    ];

    // Pass 1: exact ID match against all candidate names
    for entry in models.iter() {
        let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let id_lower = id.to_lowercase();
        if candidates.iter().any(|c| id_lower == *c)
            && let Some(n) = entry.get("context_length").and_then(|v| v.as_u64())
        {
            tracing::debug!(id = %id, context_length = n, "OpenRouter exact match");
            return Some(n as usize);
        }
    }

    // Pass 2: partial match — any candidate is a substring of the OpenRouter ID
    let mut best: Option<(usize, usize)> = None;
    for entry in models.iter() {
        let id = entry
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let id_tail = id.split('/').next_back().unwrap_or("");
        let matched = candidates
            .iter()
            .any(|c| !c.is_empty() && (id.contains(*c) || c.contains(id_tail)));
        if matched && let Some(n) = entry.get("context_length").and_then(|v| v.as_u64()) {
            let score = id.len();
            if best.map(|(_, s)| score > s).unwrap_or(true) {
                best = Some((n as usize, score));
            }
        }
    }

    if let Some((ctx, _)) = best {
        tracing::debug!(context_length = ctx, "OpenRouter partial match");
        return Some(ctx);
    }

    tracing::debug!(model = %model, "No match found in OpenRouter catalogue");
    None
}

// ──────────────────────────────────────────────
// Convenience: resolve + log at session init
// ──────────────────────────────────────────────

/// Wraps `resolve_context_window` with a user-facing print so the REPL
/// shows what was detected.
pub async fn resolve_and_report(config: &AppConfig, client: &Client) -> usize {
    let window = resolve_context_window(config, client).await;
    let mut formatted_window = String::new();
    let n_str = window.to_string();
    for (i, c) in n_str.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            formatted_window.push(',');
        }
        formatted_window.push(c);
    }
    let formatted_window: String = formatted_window.chars().rev().collect();

    println!(
        "  {}  {} tokens (model: {})",
        console::style("Context Limit :").dim(),
        console::style(formatted_window).cyan().bold(),
        console::style(&config.active_model).dim()
    );
    window
}

/// Helper: build a shared reqwest Client suitable for model lookups.
pub fn build_lookup_client() -> Client {
    Client::builder()
        .user_agent("helix-cli/0.1 (model-info-lookup)")
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .expect("Failed to build lookup client")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_known_models() {
        assert_eq!(
            lookup_known_model("claude-3-7-sonnet-20250219"),
            Some(200_000)
        );
        assert_eq!(
            lookup_known_model("anthropic/claude-3.5-sonnet"),
            Some(200_000)
        );
        assert_eq!(lookup_known_model("gpt-4o"), Some(128_000));
        assert_eq!(lookup_known_model("o1-mini"), Some(128_000));
        assert_eq!(lookup_known_model("deepseek-ai/deepseek-r1"), Some(128_000));
        assert_eq!(lookup_known_model("gemini-2.0-flash"), Some(1_048_576));
        assert_eq!(lookup_known_model("qwen2.5-coder:32b"), Some(131_072));
    }

    #[test]
    fn test_parse_context_from_name() {
        assert_eq!(
            parse_context_from_name("my-custom-model-128k"),
            Some(131_072)
        );
        assert_eq!(parse_context_from_name("custom-1m"), Some(1_000_000));
        assert_eq!(parse_context_from_name("unknown-model"), None);
    }

    #[test]
    fn test_resolve_headroom() {
        // Reasoning model gets generous headroom
        assert_eq!(resolve_headroom(128_000, None, "deepseek-r1"), 12_288);
        assert_eq!(resolve_headroom(200_000, None, "o3-mini"), 12_288);
        // Explicit thinking budget
        assert_eq!(
            resolve_headroom(128_000, Some("8192"), "claude-3-7-sonnet"),
            12_288
        );
        // Standard large model (128k) gets 8192
        assert_eq!(resolve_headroom(128_000, None, "gpt-4o"), 8_192);
        // Small model clamped to <= 30% of window
        assert_eq!(resolve_headroom(2_048, None, "tiny-model"), 512);
    }
}
