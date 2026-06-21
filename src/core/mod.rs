//! # Core Runtime Subsystem
//!
//! This module contains the core orchestrator, memory/telemetry collectors, and session managers.
//!
//! ## Key Components
//!
//! - [`Engine`]: The central autonomous execution loop driving model query-response-action turns.
//! - [`ContextManager`]: Handles active memory/message token budgets and compactions.
//! - [`MetricsCollector`]: Records step execution latencies, tool call successes, and token costs.
//! - [`Session`]: Tracks historical chats, file targets, and database storage state.

pub mod context;

pub mod engine;
pub mod metrics;
pub mod persistence;

pub use context::{ContextBudget, ContextManager, TokenEstimator};
pub use engine::Engine;
pub use metrics::{MetricsCollector, TurnMetrics, TurnTimer};
pub use persistence::{Session, SessionMeta, list_sessions};
