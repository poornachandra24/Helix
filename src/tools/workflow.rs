use anyhow::Result;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

use crate::config::AppConfig;
use crate::model::{ModelAdapter, ModelResponse};
use crate::tools::{Tool, ToolRegistry, sandbox::SharedSandbox};

fn get_journal_path() -> PathBuf {
    PathBuf::from(".helix_journal.json")
}

fn load_journal() -> Result<serde_json::Map<String, Value>> {
    let path = get_journal_path();
    if !path.exists() {
        return Ok(serde_json::Map::new());
    }
    let content = std::fs::read_to_string(&path)?;
    let val: Value = serde_json::from_str(&content)?;
    if let Value::Object(map) = val {
        Ok(map)
    } else {
        Ok(serde_json::Map::new())
    }
}

fn save_journal(map: &serde_json::Map<String, Value>) -> Result<()> {
    let path = get_journal_path();
    let content = serde_json::to_string_pretty(&Value::Object(map.clone()))?;
    std::fs::write(path, content)?;
    Ok(())
}

fn compute_hash(s: &str) -> String {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[derive(Clone)]
pub struct ReportFindingsTool {
    report: Arc<Mutex<Option<Value>>>,
}

impl ReportFindingsTool {
    pub fn new(report: Arc<Mutex<Option<Value>>>) -> Self {
        Self { report }
    }
}

#[async_trait]
impl Tool for ReportFindingsTool {
    fn name(&self) -> &str {
        "report_findings"
    }

    fn description(&self) -> &str {
        "Report the final findings for your subtask. Call this exactly once, when you are \
         done investigating; it ends your task."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Two or three sentences of synthesis"
                },
                "findings": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "claim": {
                                "type": "string",
                                "description": "The finding, one sentence"
                            },
                            "evidence": {
                                "type": "string",
                                "description": "How it was verified (file, line, or command output)"
                            },
                            "severity": {
                                "type": "string",
                                "enum": ["high", "medium", "low", "info"]
                            }
                        },
                        "required": ["claim", "evidence", "severity"]
                    }
                }
            },
            "required": ["summary", "findings"]
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let mut r = self.report.lock().unwrap();
        *r = Some(args);
        Ok("Findings successfully recorded.".to_string())
    }
}

pub struct WorkflowTool {
    config: AppConfig,
    sandbox: SharedSandbox,
    journal_lock: Arc<Mutex<()>>,
}

impl WorkflowTool {
    pub fn new(config: AppConfig, sandbox: SharedSandbox) -> Self {
        Self {
            config,
            sandbox,
            journal_lock: Arc::new(Mutex::new(())),
        }
    }

    fn lookup_journal(&self, prompt: &str) -> Option<String> {
        let _lock = self.journal_lock.lock().unwrap();
        let hash = compute_hash(prompt);
        if let Ok(journal) = load_journal()
            && let Some(val) = journal.get(&hash)
            && let Some(s) = val.as_str()
        {
            return Some(s.to_string());
        }
        None
    }

    fn write_journal(&self, prompt: &str, result: &str) {
        let _lock = self.journal_lock.lock().unwrap();
        let hash = compute_hash(prompt);
        if let Ok(mut journal) = load_journal() {
            journal.insert(hash, Value::String(result.to_string()));
            let _ = save_journal(&journal);
        }
    }
}

#[async_trait]
impl Tool for WorkflowTool {
    fn name(&self) -> &str {
        "workflow"
    }

    fn description(&self) -> &str {
        "Orchestrate a multi-agent workflow: split a large task into independent subtasks \
         and run them as parallel agents, then collect and verify their results."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "subtasks": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Independent subtask prompts to run as parallel agents"
                }
            },
            "required": ["subtasks"]
        })
    }

    async fn call(&self, args: Value) -> Result<String> {
        let subtasks = match args.get("subtasks").and_then(|v| v.as_array()) {
            Some(arr) => arr,
            None => anyhow::bail!("Missing 'subtasks' array argument"),
        };

        if subtasks.is_empty() {
            return Ok("Workflow error: no subtasks provided.".to_string());
        }

        // Limit subtasks to a maximum of 20 to prevent excessive token consumption
        let subtasks_subset: Vec<String> = subtasks
            .iter()
            .take(20)
            .map(|v| v.as_str().unwrap_or("").to_string())
            .filter(|s| !s.is_empty())
            .collect();

        println!(
            "🧬 [Workflow] Starting parallel fan-out of {} subtask(s)...",
            subtasks_subset.len()
        );

        let parent_registry = Arc::new(crate::cli::helpers::build_tool_registry(
            self.sandbox.clone(),
            crate::config::get_data_dir()?.join("skills"),
            self.config.clone(),
        ));

        // Use semaphore to cap concurrency to 3 simultaneous subagents
        let semaphore = Arc::new(Semaphore::new(3));
        let mut first_wave_tasks = Vec::new();

        for subtask in &subtasks_subset {
            let prompt = subtask.clone();
            let config = self.config.clone();
            let reg = parent_registry.clone();
            let sem = semaphore.clone();

            if let Some(cached) = self.lookup_journal(&prompt) {
                first_wave_tasks.push(tokio::spawn(async move { Ok((prompt, cached)) }));
            } else {
                let config_c = config.clone();
                let reg_c = reg.clone();
                let prompt_c = prompt.clone();
                let sem_c = sem.clone();

                first_wave_tasks.push(tokio::spawn(async move {
                    let _permit = sem_c.acquire().await?;
                    println!("🚀 [Workflow] Launching subagent: {}", prompt_c);
                    let res = run_one_subtask(config_c, reg_c, prompt_c.clone(), false).await;
                    res.map(|r| (prompt_c, r))
                }));
            }
        }

        let mut first_wave_results = Vec::new();
        for t in first_wave_tasks {
            match t.await {
                Ok(Ok((prompt, result))) => {
                    self.write_journal(&prompt, &result);
                    first_wave_results.push((prompt, result));
                }
                Ok(Err(e)) => {
                    first_wave_results.push((
                        "Error running subtask".to_string(),
                        format!("Subagent failed: {}", e),
                    ));
                }
                Err(e) => {
                    first_wave_results.push((
                        "Error joining task".to_string(),
                        format!("Task join error: {}", e),
                    ));
                }
            }
        }

        println!("🔍 [Workflow] Subagents finished. Launching verification wave...");

        let mut second_wave_tasks = Vec::new();
        for (subtask, agent_result) in &first_wave_results {
            let verifier_prompt = format!(
                "Adversarially verify the subagent result below: try to REFUTE it. Re-derive the \
                 claims yourself using tools rather than trusting the result, looking for evidence \
                 that contradicts them. Default to refuted if uncertain. Call report_findings with \
                 summary 'refuted: <why>' or 'confirmed: <why>', citing the file:line or command \
                 output that decided it.\n\n\
                 Subtask: {}\n\nResult to verify:\n{}",
                subtask, agent_result
            );

            let prompt = verifier_prompt.clone();
            let config = self.config.clone();
            let reg = parent_registry.clone();
            let sem = semaphore.clone();

            if let Some(cached) = self.lookup_journal(&prompt) {
                second_wave_tasks.push(tokio::spawn(async move { Ok(cached) }));
            } else {
                let prompt_c = prompt.clone();
                let sem_c = sem.clone();
                let config_c = config.clone();
                let reg_c = reg.clone();

                second_wave_tasks.push(tokio::spawn(async move {
                    let _permit = sem_c.acquire().await?;
                    println!("🛡️ [Workflow] Launching verifier subagent...");
                    run_one_subtask(config_c, reg_c, prompt_c, true).await
                }));
            }
        }

        let mut final_report = String::new();
        final_report.push_str("# Workflow Execution & Verification Findings Summary\n\n");

        for (i, ((subtask, agent_result), t)) in first_wave_results
            .into_iter()
            .zip(second_wave_tasks)
            .enumerate()
        {
            let verifier_result = match t.await {
                Ok(Ok(res)) => {
                    let full_verifier_prompt = format!(
                        "Adversarially verify the subagent result below: try to REFUTE it. Re-derive the \
                         claims yourself using tools rather than trusting the result, looking for evidence \
                         that contradicts them. Default to refuted if uncertain. Call report_findings with \
                         summary 'refuted: <why>' or 'confirmed: <why>', citing the file:line or command \
                         output that decided it.\n\n\
                         Subtask: {}\n\nResult to verify:\n{}",
                        subtask, agent_result
                    );
                    self.write_journal(&full_verifier_prompt, &res);
                    res
                }
                Ok(Err(e)) => format!("Verifier failed: {}", e),
                Err(e) => format!("Verifier join error: {}", e),
            };

            final_report.push_str(&format!("## Subtask {}: {}\n\n", i + 1, subtask));
            final_report.push_str("### 📋 Agent Findings:\n");
            final_report.push_str(&agent_result);
            final_report.push_str("\n\n");
            final_report.push_str("### 🛡️ Verifier Verdict:\n");
            final_report.push_str(&verifier_result);
            final_report.push_str("\n\n---\n\n");
        }

        println!("✔ [Workflow] Synthesis complete.");
        Ok(final_report)
    }
}

async fn run_one_subtask(
    config: AppConfig,
    parent_registry: Arc<ToolRegistry>,
    prompt: String,
    is_verifier: bool,
) -> Result<String> {
    let model = crate::cli::helpers::build_model(&config);

    let mut sub_registry = ToolRegistry::new();
    for (name, tool) in &parent_registry.tools {
        if name != "workflow" {
            sub_registry.register_arc(tool.clone());
        }
    }
    let report_holder = Arc::new(Mutex::new(None));
    sub_registry.register(ReportFindingsTool::new(report_holder.clone()));

    let system_prompt = if is_verifier {
        "You are a verifier subagent in a larger parallel verification wave. \
         Your goal is to adversarially verify the claims made in the subtask findings by trying to REFUTE them. \
         Re-derive the claims yourself using your tools rather than trusting the result. \
         Look for evidence that contradicts them. Default to refuted if uncertain. \
         Maintain a checklist of the claims to verify, and follow a strict inner feedback loop: \
         Investigate -> Verify -> Correct -> Confirm. \
         Finish by calling report_findings exactly once with summary 'refuted: <why>' or 'confirmed: <why>'."
    } else {
        "You are a subagent in a parallel fan-out assigned a single subtask. \
         Investigate it directly, using your tools to verify facts. \
         Always follow these best practices:\n\
         1. Maintain a progress checklist (e.g. - [ ] Step 1...) in your thoughts to track progress.\n\
         2. Follow a strict feedback loop: make edits or lookups, validate results using checks/compiles/tests, and refine if errors occur. Do not stop until validation passes.\n\
         Finish by calling report_findings exactly once when you are done investigating."
    };

    run_subagent_loop(
        model.as_ref(),
        system_prompt,
        &prompt,
        &sub_registry,
        report_holder,
        8,
    )
    .await
}

async fn run_subagent_loop(
    model: &dyn ModelAdapter,
    system_prompt: &str,
    prompt: &str,
    tools: &ToolRegistry,
    report_holder: Arc<Mutex<Option<Value>>>,
    max_turns: usize,
) -> Result<String> {
    let mut messages = vec![json!({
        "role": "user",
        "content": prompt
    })];

    let tool_descriptors = tools.descriptors();

    for _turn in 1..=max_turns {
        let response = model
            .call(system_prompt, &messages, tool_descriptors.clone(), None)
            .await?;

        match response {
            ModelResponse::EndTurn(text) => {
                if let Some(r) = report_holder.lock().unwrap().clone() {
                    return Ok(serde_json::to_string_pretty(&r)?);
                }
                return Ok(text);
            }
            ModelResponse::ToolCalls(calls, assistant_msg) => {
                messages.push(assistant_msg);

                let mut tool_results = Vec::new();
                for call in calls {
                    let result = match tools.get(&call.name) {
                        Some(tool) => match tool.call(call.args).await {
                            Ok(res) => res,
                            Err(e) => format!("Error executing tool: {}", e),
                        },
                        None => format!("Error: Tool '{}' not found", call.name),
                    };

                    tool_results.push(json!({
                        "role": "tool",
                        "tool_call_id": call.id,
                        "name": call.name,
                        "content": result
                    }));
                }

                if let Some(r) = report_holder.lock().unwrap().clone() {
                    return Ok(serde_json::to_string_pretty(&r)?);
                }

                messages.push(json!({
                    "role": "user",
                    "content": tool_results
                }));
            }
            ModelResponse::ParseError { raw_text: _, error } => {
                messages.push(json!({
                    "role": "user",
                    "content": format!(
                        "Parse Error: {}. Please output valid tool calls or responses.",
                        error
                    )
                }));
            }
        }
    }

    if let Some(r) = report_holder.lock().unwrap().clone() {
        Ok(serde_json::to_string_pretty(&r)?)
    } else {
        anyhow::bail!("Subagent hit turn limit without reporting findings.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolDescriptor;
    use tokio::sync::mpsc::UnboundedSender;

    struct MockModelAdapter {
        responses: Arc<Mutex<Vec<ModelResponse>>>,
    }

    #[async_trait]
    impl ModelAdapter for MockModelAdapter {
        async fn call(
            &self,
            _system_prompt: &str,
            _messages: &[Value],
            _tools: Vec<ToolDescriptor>,
            _stream_tx: Option<UnboundedSender<String>>,
        ) -> Result<ModelResponse> {
            let mut r = self.responses.lock().unwrap();
            if r.is_empty() {
                Ok(ModelResponse::EndTurn("Mock final response".to_string()))
            } else {
                Ok(r.remove(0))
            }
        }

        fn provider_name(&self) -> &str {
            "Mock"
        }

        fn model_name(&self) -> &str {
            "MockModel"
        }
    }

    #[test]
    fn test_compute_hash() {
        let h1 = compute_hash("hello world");
        let h2 = compute_hash("hello world");
        let h3 = compute_hash("different input");
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
        assert_eq!(h1.len(), 16);
    }

    #[tokio::test]
    async fn test_report_findings_tool() {
        let report_holder = Arc::new(Mutex::new(None));
        let tool = ReportFindingsTool::new(report_holder.clone());

        assert_eq!(tool.name(), "report_findings");
        assert!(tool.description().contains("Report the final findings"));

        let payload = json!({
            "summary": "Synthesized summary.",
            "findings": [
                {
                    "claim": "Claim checked.",
                    "evidence": "Verification complete.",
                    "severity": "low"
                }
            ]
        });

        let res = tool.call(payload.clone()).await.unwrap();
        assert_eq!(res, "Findings successfully recorded.");

        let stored = report_holder.lock().unwrap().clone().unwrap();
        assert_eq!(stored["summary"], "Synthesized summary.");
        assert_eq!(stored["findings"][0]["claim"], "Claim checked.");
    }

    #[tokio::test]
    async fn test_subagent_loop_falls_back_to_text() {
        let mock_model = MockModelAdapter {
            responses: Arc::new(Mutex::new(vec![ModelResponse::EndTurn(
                "Plain text response".to_string(),
            )])),
        };

        let report_holder = Arc::new(Mutex::new(None));
        let tools = ToolRegistry::new();

        let result = run_subagent_loop(
            &mock_model,
            "System prompt",
            "User prompt",
            &tools,
            report_holder,
            5,
        )
        .await
        .unwrap();

        assert_eq!(result, "Plain text response");
    }

    #[tokio::test]
    async fn test_subagent_loop_calls_report_findings() {
        let findings_json = json!({
            "summary": "Task complete.",
            "findings": [
                {
                    "claim": "All tests pass.",
                    "evidence": "cargo test output",
                    "severity": "info"
                }
            ]
        });

        let tool_call = crate::model::ToolCall {
            id: "tc_0".to_string(),
            name: "report_findings".to_string(),
            args: findings_json.clone(),
        };

        let assistant_msg = json!({
            "role": "assistant",
            "tool_calls": [
                {
                    "id": "tc_0",
                    "type": "function",
                    "function": {
                        "name": "report_findings",
                        "arguments": findings_json.to_string()
                    }
                }
            ]
        });

        let mock_model = MockModelAdapter {
            responses: Arc::new(Mutex::new(vec![ModelResponse::ToolCalls(
                vec![tool_call],
                assistant_msg,
            )])),
        };

        let report_holder = Arc::new(Mutex::new(None));
        let mut tools = ToolRegistry::new();
        tools.register(ReportFindingsTool::new(report_holder.clone()));

        let result = run_subagent_loop(
            &mock_model,
            "System prompt",
            "User prompt",
            &tools,
            report_holder,
            5,
        )
        .await
        .unwrap();

        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["summary"], "Task complete.");
        assert_eq!(parsed["findings"][0]["claim"], "All tests pass.");
    }
}
