/// Dynamic context window resolver.
///
/// Resolution order (first success wins):
///   1. `config.context_window_override` — user-set manual override
///   2. Ollama native `/api/show` — for local/cloud Ollama providers
///   3. Provider's own `/v1/models/{id}` — works for OpenAI and compatible providers
///   4. OpenRouter public model catalogue — covers 1 000+ models, no auth needed
///   5. Conservative fallback → 8 192

use reqwest::Client;
use serde_json::Value;
use tracing;

use crate::config::{ApiFormat, AppConfig};

const FALLBACK_CONTEXT_WINDOW: usize = 8_192;

// ──────────────────────────────────────────────
// Public entry point
// ──────────────────────────────────────────────

/// Resolve the context window for the currently active model.
/// Returns the override if set, otherwise probes the provider and external sources.
pub async fn resolve_context_window(config: &AppConfig, client: &Client) -> usize {
    if let Some(override_val) = config.context_window_override {
        tracing::info!(context_window = override_val, "Using manual context_window_override");
        return override_val;
    }

    let provider = match config.get_active_provider() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("Could not get active provider for context lookup: {}", e);
            return FALLBACK_CONTEXT_WINDOW;
        }
    };

    let model = &config.active_model;

    // Strategy 1 — Ollama native /api/show
    if provider.api_format == ApiFormat::OllamaNative {
        if let Some(w) = query_ollama_show(&provider.base_url, model, client).await {
            tracing::info!(context_window = w, source = "ollama /api/show", "Context window resolved");
            return w;
        }
    }

    // Strategy 2 — Provider's own /v1/models/{id} (OpenAI and compatible)
    if provider.api_format == ApiFormat::OpenAiCompatible {
        if let Some(w) = query_openai_models(&provider.base_url, model, provider.api_key.as_deref(), client).await {
            tracing::info!(context_window = w, source = "provider /v1/models", "Context window resolved");
            return w;
        }
    }

    // Strategy 3 — OpenRouter public catalogue (free, no auth, covers 1000+ models)
    if let Some(w) = query_openrouter_catalogue(model, client).await {
        tracing::info!(context_window = w, source = "openrouter catalogue", "Context window resolved");
        return w;
    }

    tracing::warn!(
        model = %model,
        fallback = FALLBACK_CONTEXT_WINDOW,
        "Could not determine context window from any source; using conservative fallback"
    );
    FALLBACK_CONTEXT_WINDOW
}

// ──────────────────────────────────────────────
// Strategy 1: Ollama /api/show
// ──────────────────────────────────────────────

/// Query Ollama's `/api/show` endpoint.
/// The response's `modelinfo` map contains architecture-specific keys like
/// `llama.context_length`, `gemma.context_length`, etc.
/// The `parameters` string may also carry `num_ctx <value>`.
async fn query_ollama_show(base_url: &str, model: &str, client: &Client) -> Option<usize> {
    let url = format!("{}/api/show", base_url.trim_end_matches('/'));
    let body = serde_json::json!({ "model": model });

    let resp = client
        .post(&url)
        .json(&body)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .ok()?;

    if !resp.status().is_success() {
        tracing::debug!("Ollama /api/show returned {}", resp.status());
        return None;
    }

    let json: Value = resp.json().await.ok()?;

    // modelinfo keys: "llama.context_length", "gemma.context_length", etc.
    if let Some(info) = json.get("modelinfo").and_then(|v| v.as_object()) {
        for (key, val) in info {
            if key.ends_with(".context_length") || key == "context_length" {
                if let Some(n) = val.as_u64() {
                    return Some(n as usize);
                }
            }
        }
    }

    // Fallback: parse the parameters string for `num_ctx <value>`
    if let Some(params) = json.get("parameters").and_then(|v| v.as_str()) {
        for line in params.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() == 2 && parts[0] == "num_ctx" {
                if let Ok(n) = parts[1].parse::<usize>() {
                    return Some(n);
                }
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
    let url = format!("{}/models/{}", base_url.trim_end_matches('/'), model);

    let mut req = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(10));

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

    tracing::debug!("Provider model object for '{}' had no context window field", model);
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

    let model_lower = model.to_lowercase();

    // Pass 1: exact ID match
    for entry in models.iter() {
        let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id.to_lowercase() == model_lower {
            if let Some(n) = entry.get("context_length").and_then(|v| v.as_u64()) {
                tracing::debug!(id = %id, context_length = n, "OpenRouter exact match");
                return Some(n as usize);
            }
        }
    }

    // Pass 2: partial match — the model name is a substring of the OpenRouter ID
    // e.g. "llama-3.1-8b" matches "meta-llama/llama-3.1-8b-instruct"
    let mut best: Option<(usize, usize)> = None; // (context_length, match_score)
    for entry in models.iter() {
        let id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
        if id.contains(&model_lower) || model_lower.contains(id.split('/').last().unwrap_or("")) {
            if let Some(n) = entry.get("context_length").and_then(|v| v.as_u64()) {
                let score = id.len(); // prefer more specific (longer) IDs
                if best.map(|(_, s)| score > s).unwrap_or(true) {
                    best = Some((n as usize, score));
                }
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
    println!(
        "📐 Context window: {} tokens (model: {})",
        console::style(format!("{:>9}", window)).cyan().bold(),
        console::style(&config.active_model).dim()
    );
    window
}

/// Helper: build a shared reqwest Client suitable for model lookups.
pub fn build_lookup_client() -> Client {
    Client::builder()
        .user_agent("harness-cli/0.1 (model-info-lookup)")
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .expect("Failed to build lookup client")
}
