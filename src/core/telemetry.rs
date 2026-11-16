//! # OpenTelemetry & Logfire Tracing Manager
//!
//! Provides dynamic, toggleable OpenTelemetry telemetry backed by Pydantic Logfire.
//! Spans, events, and metrics from Helix agent loops, model calls, and tools can be
//! dynamically routed to Logfire and persisted across sessions without relying on
//! reloadable Per-Layer Filters that can corrupt subscriber FilterIds.

use anyhow::{Context as _, Result};
use opentelemetry::{Context, trace::TracerProvider};
use opentelemetry_sdk::{
    Resource,
    trace::{SdkTracerProvider, Span, SpanData, SpanExporter, SpanProcessor},
};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

/// Detect the Logfire endpoint from the token's embedded region.
fn resolve_logfire_endpoint(token: &str) -> &'static str {
    // Logfire token format: pylf_v{version}_{region}_{secret}
    let region = token
        .strip_prefix("pylf_v")
        .and_then(|r| r.split_once('_'))
        .and_then(|(_, r)| r.split_once('_'))
        .map(|(reg, _)| reg);

    match region {
        Some("eu") => "https://logfire-eu.pydantic.dev",
        Some("stagingus") => "https://logfire-us.pydantic.info",
        Some("stagingeu") => "https://logfire-eu.pydantic.info",
        _ => "https://logfire-us.pydantic.dev",
    }
}

/// Internal message protocol between tracing subscribers and the background Logfire exporter task.
#[derive(Debug)]
enum TelemetryMsg {
    Span(Box<SpanData>),
    Flush(tokio::sync::oneshot::Sender<()>),
    Shutdown(tokio::sync::oneshot::Sender<()>),
}

/// Dynamic Tokio-backed span processor that queues finished spans to an async exporter task.
#[derive(Debug, Clone)]
struct DynamicSpanProcessor {
    sender: Arc<Mutex<Option<mpsc::UnboundedSender<TelemetryMsg>>>>,
    resource: Arc<Mutex<Resource>>,
}

impl DynamicSpanProcessor {
    fn new(resource: Resource) -> Self {
        Self {
            sender: Arc::new(Mutex::new(None)),
            resource: Arc::new(Mutex::new(resource)),
        }
    }

    /// Attach a new Logfire exporter running on the Tokio runtime.
    fn attach_exporter<E: SpanExporter + Send + Sync + 'static>(&self, mut exporter: E) {
        // Detach any previous exporter task
        self.detach_exporter_sync();

        let resource = self.resource.lock().unwrap().clone();
        exporter.set_resource(&resource);

        let (span_tx, mut span_rx) = mpsc::unbounded_channel::<TelemetryMsg>();

        {
            let mut s_lock = self.sender.lock().unwrap();
            *s_lock = Some(span_tx);
        }

        tokio::spawn(async move {
            let mut batch = Vec::new();
            let mut interval = tokio::time::interval(Duration::from_millis(500));
            // First tick completes immediately
            interval.tick().await;

            loop {
                tokio::select! {
                    msg = span_rx.recv() => {
                        match msg {
                            Some(TelemetryMsg::Span(span)) => {
                                batch.push(*span);
                                if batch.len() >= 64 {
                                    let to_send = std::mem::take(&mut batch);
                                    let _ = exporter.export(to_send).await;
                                }
                            }
                            Some(TelemetryMsg::Flush(ack_tx)) => {
                                if !batch.is_empty() {
                                    let to_send = std::mem::take(&mut batch);
                                    let _ = exporter.export(to_send).await;
                                }
                                let _ = exporter.force_flush();
                                let _ = ack_tx.send(());
                            }
                            Some(TelemetryMsg::Shutdown(ack_tx)) => {
                                // Drain remaining messages in queue
                                while let Ok(cmd) = span_rx.try_recv() {
                                    match cmd {
                                        TelemetryMsg::Span(s) => batch.push(*s),
                                        TelemetryMsg::Flush(a) => { let _ = a.send(()); }
                                        TelemetryMsg::Shutdown(_) => {}
                                    }
                                }
                                if !batch.is_empty() {
                                    let to_send = std::mem::take(&mut batch);
                                    let _ = exporter.export(to_send).await;
                                }
                                let _ = exporter.force_flush();
                                let _ = exporter.shutdown();
                                let _ = ack_tx.send(());
                                break;
                            }
                            None => {
                                if !batch.is_empty() {
                                    let to_send = std::mem::take(&mut batch);
                                    let _ = exporter.export(to_send).await;
                                }
                                let _ = exporter.force_flush();
                                let _ = exporter.shutdown();
                                break;
                            }
                        }
                    }
                    _ = interval.tick() => {
                        if !batch.is_empty() {
                            let to_send = std::mem::take(&mut batch);
                            let _ = exporter.export(to_send).await;
                        }
                    }
                }
            }
        });
    }

    /// Asynchronously flush all queued spans and wait for the exporter to finish HTTP sending.
    pub async fn flush_async(&self) {
        let sender = {
            let guard = self.sender.lock().unwrap();
            guard.clone()
        };
        if let Some(tx) = sender {
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            if tx.send(TelemetryMsg::Flush(ack_tx)).is_ok() {
                let _ = tokio::time::timeout(Duration::from_secs(5), ack_rx).await;
            }
        }
    }

    /// Asynchronously shutdown the exporter task and wait for final flush and shutdown.
    pub async fn shutdown_async(&self) {
        let sender = {
            let mut guard = self.sender.lock().unwrap();
            guard.take()
        };
        if let Some(tx) = sender {
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            if tx.send(TelemetryMsg::Shutdown(ack_tx)).is_ok() {
                let _ = tokio::time::timeout(Duration::from_secs(5), ack_rx).await;
            }
        }
    }

    /// Synchronously detach the exporter task without waiting for HTTP completion.
    fn detach_exporter_sync(&self) {
        let sender = {
            let mut guard = self.sender.lock().unwrap();
            guard.take()
        };
        if let Some(tx) = sender {
            let (ack_tx, _ack_rx) = tokio::sync::oneshot::channel();
            let _ = tx.send(TelemetryMsg::Shutdown(ack_tx));
        }
    }
}

impl SpanProcessor for DynamicSpanProcessor {
    fn on_start(&self, _span: &mut Span, _cx: &Context) {}

    fn on_end(&self, span: SpanData) {
        if let Ok(guard) = self.sender.lock()
            && let Some(ref tx) = *guard
        {
            let _ = tx.send(TelemetryMsg::Span(Box::new(span)));
        }
    }

    fn force_flush(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        let sender = {
            let guard = self.sender.lock().unwrap();
            guard.clone()
        };
        if let Some(tx) = sender {
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            if tx.send(TelemetryMsg::Flush(ack_tx)).is_ok()
                && let Ok(handle) = tokio::runtime::Handle::try_current()
                && handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
            {
                tokio::task::block_in_place(|| {
                    handle.block_on(async {
                        let _ = tokio::time::timeout(Duration::from_secs(5), ack_rx).await;
                    });
                });
            }
        }
        Ok(())
    }

    fn shutdown_with_timeout(&self, timeout: Duration) -> opentelemetry_sdk::error::OTelSdkResult {
        let sender = {
            let mut guard = self.sender.lock().unwrap();
            guard.take()
        };
        if let Some(tx) = sender {
            let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
            if tx.send(TelemetryMsg::Shutdown(ack_tx)).is_ok()
                && let Ok(handle) = tokio::runtime::Handle::try_current()
                && handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
            {
                tokio::task::block_in_place(|| {
                    handle.block_on(async {
                        let _ = tokio::time::timeout(timeout, ack_rx).await;
                    });
                });
            }
        }
        Ok(())
    }

    fn shutdown(&self) -> opentelemetry_sdk::error::OTelSdkResult {
        self.shutdown_with_timeout(Duration::from_secs(5))
    }

    fn set_resource(&mut self, resource: &Resource) {
        if let Ok(mut res) = self.resource.lock() {
            *res = resource.clone();
        }
    }
}

pub struct TelemetryManager {
    dynamic_processor: DynamicSpanProcessor,
    is_enabled: AtomicBool,
    token: Mutex<Option<String>>,
}

static TELEMETRY: OnceLock<TelemetryManager> = OnceLock::new();

impl TelemetryManager {
    /// Initialize the global telemetry subsystem and tracing subscriber.
    ///
    /// Must only be called once at application startup (in `main`).
    pub fn init(
        log_level: &str,
        enable_telemetry: bool,
        initial_token: Option<String>,
    ) -> Result<&'static TelemetryManager> {
        let resource = Resource::builder()
            .with_service_name("helix")
            .with_attribute(opentelemetry::KeyValue::new(
                "service.version",
                env!("CARGO_PKG_VERSION"),
            ))
            .build();

        let dynamic_processor = DynamicSpanProcessor::new(resource.clone());

        let effective_token = initial_token
            .clone()
            .or_else(|| std::env::var("LOGFIRE_TOKEN").ok());

        let is_active = if enable_telemetry {
            if let Some(ref tok) = effective_token {
                match Self::build_and_attach(&dynamic_processor, tok) {
                    Ok(()) => true,
                    Err(e) => {
                        eprintln!("Warning: Failed to initialize Logfire telemetry: {e}");
                        false
                    }
                }
            } else {
                false
            }
        } else {
            false
        };

        let tracer_provider = SdkTracerProvider::builder()
            .with_resource(resource)
            .with_span_processor(dynamic_processor.clone())
            .build();

        let otel_tracer = tracer_provider.tracer("helix");
        let otel_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new(
                "helix=info,ruvector_sona=info,tokenizers=off,hyper=off,hyper_util=off,reqwest=off,h2=off,mio=off,want=off,fastembed=warn",
            )
        });
        let otel_layer = tracing_opentelemetry::layer()
            .with_tracer(otel_tracer)
            .with_filter(otel_filter);

        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_filter(EnvFilter::new(format!("helix={}", log_level)));

        tracing_subscriber::registry()
            .with(fmt_layer)
            .with(otel_layer)
            .init();

        let manager = TelemetryManager {
            dynamic_processor,
            is_enabled: AtomicBool::new(is_active),
            token: Mutex::new(effective_token.or(initial_token)),
        };

        let _ = TELEMETRY.set(manager);
        Ok(TELEMETRY.get().expect("TelemetryManager must be set"))
    }

    /// Access the global TelemetryManager instance, if initialized.
    pub fn global() -> Option<&'static TelemetryManager> {
        TELEMETRY.get()
    }

    /// Build and attach a Logfire OpenTelemetry exporter using the write token.
    fn build_and_attach(processor: &DynamicSpanProcessor, token: &str) -> Result<()> {
        let endpoint = resolve_logfire_endpoint(token);
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), format!("Bearer {token}"));

        let exporter = logfire::exporters::span_exporter(endpoint, Some(headers))
            .context("Failed to build Logfire span exporter")?;

        processor.attach_exporter(exporter);
        Ok(())
    }

    /// Enable telemetry with the given Logfire token.
    pub fn enable(&self, token: &str) -> Result<()> {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            anyhow::bail!("Logfire token cannot be empty");
        }

        Self::build_and_attach(&self.dynamic_processor, trimmed)?;

        {
            let mut tok_lock = self.token.lock().unwrap();
            *tok_lock = Some(trimmed.to_string());
            self.is_enabled.store(true, Ordering::SeqCst);
        }

        tracing::info!("Logfire telemetry enabled");
        Ok(())
    }

    /// Disable telemetry without discarding the saved token.
    pub fn disable(&self) -> Result<()> {
        self.dynamic_processor.detach_exporter_sync();
        self.is_enabled.store(false, Ordering::SeqCst);

        tracing::info!("Logfire telemetry disabled");
        Ok(())
    }

    /// Check if telemetry is currently active and exporting.
    pub fn is_enabled(&self) -> bool {
        self.is_enabled.load(Ordering::SeqCst)
    }

    /// Retrieve the currently configured Logfire token, if any.
    pub fn get_token(&self) -> Option<String> {
        self.token.lock().unwrap().clone()
    }

    /// Update or save the Logfire token.
    pub fn set_token(&self, token: &str) {
        let trimmed = token.trim().to_string();
        let mut lock = self.token.lock().unwrap();
        *lock = Some(trimmed);
    }

    /// Forcibly flush buffered spans to Logfire asynchronously.
    pub async fn flush(&self) {
        self.dynamic_processor.flush_async().await;
    }

    /// Gracefully shutdown the exporter and flush remaining telemetry asynchronously.
    pub async fn shutdown(&self) {
        self.dynamic_processor.shutdown_async().await;
    }

    /// Synchronously shutdown the exporter and flush remaining telemetry.
    pub fn shutdown_sync(&self) {
        let _ = self.dynamic_processor.shutdown();
    }

    /// Forcibly flush buffered spans to Logfire synchronously.
    pub fn force_flush(&self) {
        let _ = self.dynamic_processor.force_flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_resource() -> Resource {
        Resource::builder().with_service_name("helix-test").build()
    }

    #[tokio::test]
    async fn test_telemetry_manager_build_and_attach() {
        let dynamic_processor = DynamicSpanProcessor::new(test_resource());
        let result =
            TelemetryManager::build_and_attach(&dynamic_processor, "pylf_v1_us_testtoken12345");
        assert!(result.is_ok(), "Logfire exporter should attach cleanly");
        dynamic_processor.detach_exporter_sync();
    }

    #[test]
    fn test_telemetry_manager_empty_token_fails() {
        let dynamic_processor = DynamicSpanProcessor::new(test_resource());
        let manager = TelemetryManager {
            dynamic_processor,
            is_enabled: AtomicBool::new(false),
            token: Mutex::new(None),
        };

        assert!(!manager.is_enabled());
        let res = manager.enable("   ");
        assert!(res.is_err(), "Empty token should fail");
        assert!(!manager.is_enabled());
    }

    #[test]
    fn test_telemetry_manager_token_getter_setter() {
        let dynamic_processor = DynamicSpanProcessor::new(test_resource());
        let manager = TelemetryManager {
            dynamic_processor,
            is_enabled: AtomicBool::new(false),
            token: Mutex::new(None),
        };

        assert_eq!(manager.get_token(), None);
        manager.set_token("my_logfire_token");
        assert_eq!(manager.get_token().as_deref(), Some("my_logfire_token"));
    }

    #[test]
    fn test_resolve_logfire_endpoint() {
        assert_eq!(
            resolve_logfire_endpoint("pylf_v1_us_abc123"),
            "https://logfire-us.pydantic.dev"
        );
        assert_eq!(
            resolve_logfire_endpoint("pylf_v2_us_abc123"),
            "https://logfire-us.pydantic.dev"
        );
        assert_eq!(
            resolve_logfire_endpoint("pylf_v1_eu_abc123"),
            "https://logfire-eu.pydantic.dev"
        );
        assert_eq!(
            resolve_logfire_endpoint("invalid_token"),
            "https://logfire-us.pydantic.dev"
        );
    }

    #[tokio::test]
    async fn test_tracing_span_forwarding() {
        use opentelemetry::trace::{Span as _, Tracer as _};

        let dynamic_processor = DynamicSpanProcessor::new(test_resource());
        let (span_tx, mut span_rx) = mpsc::unbounded_channel::<TelemetryMsg>();
        {
            let mut s_lock = dynamic_processor.sender.lock().unwrap();
            *s_lock = Some(span_tx);
        }

        let resource = Resource::builder().with_service_name("helix-test").build();

        let tracer_provider = SdkTracerProvider::builder()
            .with_resource(resource)
            .with_span_processor(dynamic_processor)
            .build();

        let tracer = tracer_provider.tracer("helix-test");
        let mut span = tracer.start("test_span");
        span.set_attribute(opentelemetry::KeyValue::new("test.key", "test.val"));
        span.end();

        let received = span_rx.recv().await;
        assert!(received.is_some());
        if let Some(TelemetryMsg::Span(span_data)) = received {
            assert_eq!(span_data.name, "test_span");
        } else {
            panic!("Expected TelemetryMsg::Span");
        }
    }
}
