use thiserror::Error;

#[derive(Error, Debug)]
pub enum HarnessError {
    #[error("API error from {provider}: {message}")]
    ApiError { provider: String, message: String },

    #[error("Tool parse error: {0}")]
    ToolParseError(String),

    #[error("Tool execution denied by user")]
    ToolDenied,

    #[error("Tool '{tool}' not found in registry")]
    ToolNotFound { tool: String },

    #[error("Tool '{tool}' execution failed: {source}")]
    ToolExecError {
        tool: String,
        #[source]
        source: anyhow::Error,
    },

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Session error: {0}")]
    SessionError(String),

    #[error("Max iterations ({0}) reached without completing the task")]
    MaxIterationsReached(usize),

    #[error("Network error: {0}")]
    NetworkError(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}
