use anyhow::Result;
use serde_json::json;
use std::fs;
use std::path::Path;

use crate::config::AppConfig;
use crate::model::{ModelAdapter, ModelResponse, OpenAiCompatibleAdapter};

pub struct Proposer {
    config: AppConfig,
}

impl Proposer {
    pub fn new(config: AppConfig) -> Self {
        Self { config }
    }

    pub async fn propose(&self, project_root: &Path, bottleneck: &str, baseline: &str) -> Result<String> {
        let system_prompt = "You are operating in SELF-EVOLUTION MODE for the harness-cli codebase.

## Your task
Propose ONE focused, high-value code improvement based on the runtime metrics.
Output ONLY a unified diff (git diff format). No prose, no explanation outside the diff header.
Enclose the diff in ```diff ... ``` tags.

## Constraints (HARD — violations will be automatically rejected)
- No new `unsafe` blocks
- No modifications to src/evolution/security.rs or src/evolution/validator.rs
- No new network clients (reqwest::Client::new)
- No process::exit calls
- The diff must apply cleanly (patch -p1) and compile with zero warnings";

        let mut source_content = String::new();
        for file in &[
            "src/core/engine.rs",
            "src/core/context.rs",
            "src/model/mod.rs",
            "src/model/registry.rs",
        ] {
            let path = project_root.join(file);
            if let Ok(content) = fs::read_to_string(&path) {
                source_content.push_str(&format!("// File: {}\n{}\n\n", file, content));
            }
        }

        let prompt = format!(
            "## Identified Bottleneck\n{}\n\n## Benchmark Baseline\n{}\n\n## Core Source Files\n{}",
            bottleneck, baseline, source_content
        );

        let messages = vec![json!({"role": "user", "content": prompt})];
        let adapter = OpenAiCompatibleAdapter::new(self.config.clone());
        
        let response = adapter.call(system_prompt, &messages, vec![], None).await?;
        match response {
            ModelResponse::EndTurn(text) => {
                let extracted = if let Some(start) = text.find("```diff") {
                    let end = text[start + 7..].find("```").unwrap_or(text.len() - start - 7);
                    text[start + 7..start + 7 + end].trim().to_string()
                } else {
                    text.trim().to_string()
                };

                if !extracted.contains("--- a/") && !extracted.contains("diff --git") {
                    anyhow::bail!("LLM did not return a valid unified diff format.");
                }
                Ok(extracted)
            },
            _ => anyhow::bail!("Expected text response containing diff"),
        }
    }
}
