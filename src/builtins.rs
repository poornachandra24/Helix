use crate::tools::Tool;
use anyhow::Result;
use serde_json::json;
use std::process::Command;
use console::style;
use dialoguer::Confirm;

pub struct BashTool;
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }
    fn description(&self) -> &str {
        "Run a bash command. Requires 'cmd' parameter."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "cmd": {"type": "string", "description": "Command to run"}
            },
            "required": ["cmd"]
        })
    }
    fn call(&self, args: serde_json::Value) -> Result<String> {
        let cmd = args.get("cmd")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        
        println!("\n⚠️  Agent wants to run: {}", style(cmd).bold().yellow());
        
        let proceed = Confirm::new()
            .with_prompt("Do you want to allow this command?")
            .default(false)
            .interact()?;
            
        if !proceed {
            println!("❌ {}", style("Execution denied.").red());
            return Ok(String::from("User denied permission to execute this command. You must try an alternative approach or stop."));
        }
        
        tracing::debug!("Running bash command: {}", cmd);
        
        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()?;
            
        let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
        
        let max_len = 4000;
        if stdout.len() > max_len {
            stdout = format!("{}... [TRUNCATED - Output exceeded {} chars]\n", &stdout[..max_len], max_len);
        }
        if stderr.len() > max_len {
            stderr = format!("{}... [TRUNCATED - Error exceeded {} chars]\n", &stderr[..max_len], max_len);
        }
        
        Ok(format!("exit_code={}\n{}{}", output.status.code().unwrap_or(-1), stdout, stderr))
    }
}
