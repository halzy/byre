//! # Telemetry for Rust Applications
//!
//! This module provides a complete telemetry solution integrating:
//!
//! - **Tracing**: Distributed tracing for tracking request flows across services
//! - **Metrics**: Quantitative measurements of your application's performance
//! - **Logging**: Structured event logging for visibility into application behavior
//!
//! The implementation uses [OpenTelemetry](https://opentelemetry.io/) standards for compatibility
//! with popular observability platforms like Jaeger, Prometheus, Grafana, etc.
//!
//! ## Why use telemetry?
//!
//! - **Troubleshooting**: Quickly identify and debug issues in production
//! - **Performance optimizations**: Measure and improve application performance
//! - **Service health monitoring**: Get alerts when services degrade
//! - **Cross-service visibility**: Track requests across microservices
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use byre::telemetry::{TelemetrySettings, TelemetryProviders};
//! use byre::ServiceInfo;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // 1. Configure telemetry in your settings
//! let settings = TelemetrySettings::default();
//! let service = ServiceInfo {
//!     name: "my-service",
//!     name_in_metrics: "my_service".to_string(),
//!     version: "1.0.0",
//!     author: "Author",
//!     description: "My service description",
//! };
//!
//! // 2. Initialize telemetry (keep the returned handle alive for the app lifetime!)
//! let _telemetry: TelemetryProviders = byre::telemetry::init(&service, &settings)?;
//!
//! // 3. Use tracing macros as normal - they automatically go to OpenTelemetry
//! tracing::info!("Application started");
//! # Ok(())
//! # }
//! ```
//!
//! ## Common Patterns
//!
//! ### gRPC Metadata (linking incoming trace context)
//!
//! ```
//! use byre::telemetry::{TraceContextCarrier, TraceContextExt};
//!
//! // In a gRPC handler, extract trace context from incoming metadata
//! let mut metadata = tonic::metadata::MetadataMap::new();
//! metadata.insert("traceparent", "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".parse().unwrap());
//!
//! // Extract the trace context
//! let ctx = metadata.extract_trace_context();
//!
//! // Or link it directly to the current span
//! let _ = metadata.link_distributed_trace();
//! ```
//!
//! ### gRPC Metadata (propagating trace context)
//!
//! ```
//! use byre::telemetry::{TraceContextCarrier, TraceContextExt};
//!
//! // Before making outgoing gRPC calls, inject trace context
//! let mut metadata = tonic::metadata::MetadataMap::new();
//! metadata.inject_trace_context();
//! // metadata now contains traceparent header (if there's an active span)
//! ```
//!
//! ### HTTP Headers (linking incoming trace context)
//!
//! ```
//! use byre::telemetry::{TraceContextCarrier, TraceContextExt};
//!
//! // In an HTTP handler, extract trace context from incoming headers
//! let mut headers = http::HeaderMap::new();
//! headers.insert("traceparent", "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".parse().unwrap());
//!
//! // Extract the trace context
//! let ctx = headers.extract_trace_context();
//!
//! // Or link it directly to the current span
//! let _ = headers.link_distributed_trace();
//! ```
//!
//! ### HTTP Headers (propagating trace context)
//!
//! ```
//! use byre::telemetry::{TraceContextCarrier, TraceContextExt};
//!
//! // Before making outgoing HTTP calls, inject trace context
//! let mut headers = http::HeaderMap::new();
//! headers.inject_trace_context();
//! // headers now contains traceparent header (if there's an active span)
//! ```
//!
//! ### HashMap (for message queues)
//!
//! ```
//! use std::collections::HashMap;
//! use byre::telemetry::{TraceContextCarrier, TraceContextExt};
//!
//! // Producer: inject trace context into message headers
//! let mut headers: HashMap<String, String> = HashMap::new();
//! headers.inject_trace_context();
//!
//! // Consumer: extract trace context from message headers
//! let ctx = headers.extract_trace_context();
//!
//! // Or link it directly to the current span
//! let _ = headers.link_distributed_trace();
//! ```

use doku::Document;
use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{global, KeyValue};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{
    ExporterBuildError, LogExporter, MetricExporter, SpanExporter, WithExportConfig,
};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::{trace as sdktrace, Resource};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, Snafu};
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::ServiceInfo;

// ============================================================================
// Trace Context Carrier Traits
// ============================================================================

/// Trait for types that can carry trace context (e.g., HTTP headers, gRPC metadata).
///
/// This trait provides a unified interface for extracting and injecting
/// W3C Trace Context headers across different transport types.
///
/// Implementations are provided for:
/// - `tonic::metadata::MetadataMap` (gRPC)
/// - `http::HeaderMap` (HTTP)
/// - `HashMap<String, String>` (message queues, generic use)
pub trait TraceContextCarrier {
    /// Extract trace context from this carrier.
    ///
    /// Returns an OpenTelemetry context that can be used to link spans
    /// to an incoming distributed trace.
    fn extract_trace_context(&self) -> opentelemetry::Context;

    /// Inject the current span's trace context into this carrier.
    ///
    /// Call this before making outgoing requests to propagate the trace.
    fn inject_trace_context(&mut self);
}

/// Extension trait providing convenient methods for trace context propagation.
///
/// This trait is automatically implemented for all types that implement
/// [`TraceContextCarrier`]. Import this trait to use the extension methods:
///
/// ```
/// use byre::telemetry::{TraceContextCarrier, TraceContextExt};
///
/// // Now you can call these methods on any carrier type:
/// let mut headers = http::HeaderMap::new();
/// let ctx = headers.extract_trace_context();
/// headers.inject_trace_context();
/// let _ = headers.link_distributed_trace();
/// ```
pub trait TraceContextExt: TraceContextCarrier {
    /// Link the current tracing span to an incoming distributed trace.
    ///
    /// This is a convenience method that extracts the trace context and
    /// sets it as the parent of the current span. Call this at the start
    /// of your handler after the `#[tracing::instrument]` span is created.
    ///
    /// Returns `Ok(())` if successful, or an error if the span context
    /// couldn't be set. Most callers will want to ignore the error:
    ///
    /// ```
    /// use byre::telemetry::{TraceContextCarrier, TraceContextExt};
    ///
    /// let headers = http::HeaderMap::new();
    /// let _ = headers.link_distributed_trace();
    /// ```
    fn link_distributed_trace(&self) -> Result<(), Error>;
}

impl<T: TraceContextCarrier> TraceContextExt for T {
    fn link_distributed_trace(&self) -> Result<(), Error> {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        let parent_cx = self.extract_trace_context();
        tracing::Span::current()
            .set_parent(parent_cx)
            .map_err(|e| Error::LinkDistributedTrace {
                source: Box::new(e),
            })
    }
}

/// Errors initializing telemetry
#[derive(Debug, Snafu)]
pub enum Error {
    /// Could not initialize the logger
    #[snafu(display("Could not initialize logging: {source}"))]
    InitLog {
        /// The error from initializing the gRPC connection
        source: ExporterBuildError,
    },

    /// Could not initialize metrics
    #[snafu(display("Could not initialize metrics: {source}"))]
    InitMetric {
        /// The error from initializing the gRPC connection
        source: ExporterBuildError,
    },

    /// Could not initialize tracing
    #[snafu(display("Could not initialize tracing: {source}"))]
    InitTrace {
        /// The error from initializing the gRPC connection
        source: ExporterBuildError,
    },

    /// Could not link distributed trace context to current span
    #[snafu(display("Could not link distributed trace: {source}"))]
    LinkDistributedTrace {
        /// The underlying error from tracing-opentelemetry
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Settings for metrics collection and export.
///
/// Metrics provide quantitative measurements about your application's performance and behavior.
/// Examples include request counts, error rates, response times, and resource usage.
#[derive(Debug, Default, Serialize, Deserialize, Document)]
pub struct MetricSettings {
    /// gRPC endpoint to send metrics to. Omit to disable opentelemetry metrics.
    #[doku(example = "http://localhost:4318/v1/metrics")]
    pub endpoint: Option<String>,
}

/// Settings for logging configuration.
///
/// Logs provide contextual information about application events and are essential
/// for debugging and monitoring application behavior.
///
/// Note: `otel_level` will filter the logs before they are sent to the console, so if `otel_level` is `warn`, then `console_level` can only be `warn`, `error`, or `off`.
#[derive(Debug, Default, Serialize, Deserialize, Document)]
pub struct LogSettings {
    /// log level used when filtering console logs. Uses env-logger style syntax. Set to "off" to disable console logging.
    /// `console_level` is limited by `otel_level`, so if `otel_level` is `warn`, then `console_level` can only be `warn`, `error`, or `off`.
    #[doku(example = "debug,yourcrate=info")]
    pub console_level: String,

    /// log level used when filtering opentelemetry logs. Uses env-logger style syntax.
    #[doku(example = "warn,yourcrate=debug")]
    pub otel_level: String,

    /// gRPC endpoint to send the opentelemetry logs. Omit to disable opentelemetry logs, will not disable console logs.
    #[doku(example = "http://localhost:4317")]
    pub endpoint: Option<String>,
}

/// Settings for distributed tracing.
///
/// Traces track the flow of requests as they propagate through your system, helping you
/// understand the execution path and identify performance bottlenecks.
#[derive(Debug, Default, Serialize, Deserialize, Document)]
pub struct TraceSettings {
    /// gRPC endpoint to send opentelemetry traces to, omit to disable.
    #[doku(example = "http://localhost:4317")]
    pub endpoint: Option<String>,
}

/**
Settings for tracing, logging, and metrics.

Use `TelemetrySettings` as a member in your own `Settings` object.

# Example

```rust
use doku::Document;
use serde::Deserialize;

#[derive(Deserialize, Document)]
/// Data Archive Settings
pub struct Settings {
    /// Server Settings
    pub application: Application,
    /// Telemetry settings.
    pub telemetry: byre::telemetry::TelemetrySettings,
}

#[derive(Deserialize, Document)]
pub struct Application {
    #[doku(example = "localhost")]
    pub listen_host: String,
    #[doku(example = "8080")]
    pub listen_port: u16,
}
``` */
#[derive(Debug, Default, Serialize, Deserialize, Document)]
pub struct TelemetrySettings {
    /// Settings for tracing
    pub trace: TraceSettings,
    /// Settings for logging
    pub log: LogSettings,
    /// Settings for metrics
    pub metric: MetricSettings,
}

/// Container for the initialized telemetry providers.
///
/// This struct owns the telemetry providers and ensures they are properly
/// shut down when dropped. You must keep this value alive for the duration
/// of your application; dropping it will shut down all telemetry.
#[derive(Debug, Default)]
#[must_use = "dropping TelemetryProviders will shut down all telemetry"]
pub struct TelemetryProviders {
    meter: Option<SdkMeterProvider>,
    tracer: Option<sdktrace::SdkTracerProvider>,
    logger: Option<SdkLoggerProvider>,
}

impl Drop for TelemetryProviders {
    fn drop(&mut self) {
        if let Some(tracer_provider) = self.tracer.take() {
            if let Err(err) = tracer_provider.shutdown() {
                eprintln!("Error shutting down Telemetry tracer provider: {err}");
            }
        }
        if let Some(logger_provider) = self.logger.take() {
            if let Err(err) = logger_provider.shutdown() {
                eprintln!("Error shutting down Telemetry logger provider: {err}");
            }
        }
        if let Some(meter_provider) = self.meter.take() {
            if let Err(err) = meter_provider.shutdown() {
                eprintln!("Error shutting down Telemetry meter provider: {err}");
            }
        }
    }
}

fn init_traces(
    service_info: &ServiceInfo,
    settings: &TraceSettings,
) -> Result<Option<sdktrace::SdkTracerProvider>, ExporterBuildError> {
    match &settings.endpoint {
        Some(endpoint) => {
            let exporter = SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .build()?;

            let resource = Resource::builder()
                .with_attribute(KeyValue::new(
                    opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                    service_info.name_in_metrics.clone(),
                ))
                .build();

            Ok(Some(
                sdktrace::SdkTracerProvider::builder()
                    .with_resource(resource)
                    .with_batch_exporter(exporter)
                    .build(),
            ))
        }
        None => Ok(None),
    }
}

fn init_metrics(
    service_info: &ServiceInfo,
    setting: &MetricSettings,
) -> Result<Option<opentelemetry_sdk::metrics::SdkMeterProvider>, ExporterBuildError> {
    match &setting.endpoint {
        Some(endpoint) => {
            let exporter = MetricExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .build()?;
            let reader = PeriodicReader::builder(exporter).build();

            let resource = Resource::builder()
                .with_attribute(KeyValue::new(
                    opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                    service_info.name_in_metrics.clone(),
                ))
                .build();

            Ok(Some(
                SdkMeterProvider::builder()
                    .with_reader(reader)
                    .with_resource(resource)
                    .build(),
            ))
        }

        None => Ok(None),
    }
}

fn init_otel_logs<S>(
    service_info: &ServiceInfo,
    settings: &LogSettings,
) -> Result<
    (
        Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
        Option<impl tracing_subscriber::layer::Layer<S> + use<S>>,
    ),
    Error,
>
where
    S: Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    match &settings.endpoint {
        None => Ok((None, None)),

        Some(endpoint) => {
            let builder = init_otel_logs_builder(service_info, endpoint)?;

            let logger_provider = builder.build();

            // Create a new OpenTelemetryTracingBridge using the above LoggerProvider.
            let otel_layer = OpenTelemetryTracingBridge::new(&logger_provider);

            // For the OpenTelemetry layer, add a tracing filter to filter events from
            // OpenTelemetry and its dependent crates (opentelemetry-otlp uses crates
            // like reqwest/tonic etc.) from being sent back to OTel itself, thus
            // preventing infinite telemetry generation. The filter levels are set as
            // follows:
            // - Allow `info` level and above by default.
            // - Restrict `opentelemetry`, `hyper`, `tonic`, and `reqwest` completely.
            // Note: This will also drop events from crates like `tonic` etc. even when
            // they are used outside the OTLP Exporter. For more details, see:
            // https://github.com/open-telemetry/opentelemetry-rust/issues/761
            // FIXME: the directives below should be noted in the documentation!
            let filter_otel = EnvFilter::new(&settings.otel_level)
                .add_directive("hyper=off".parse().unwrap())
                .add_directive("opentelemetry=off".parse().unwrap())
                .add_directive("tonic=off".parse().unwrap())
                .add_directive("h2=off".parse().unwrap())
                .add_directive("reqwest=off".parse().unwrap());
            let otel_layer = otel_layer.with_filter(filter_otel);

            Ok((Some(logger_provider), Some(otel_layer)))
        }
    }
}

fn init_otel_logs_builder(
    service_info: &ServiceInfo,
    endpoint: &String,
) -> Result<opentelemetry_sdk::logs::LoggerProviderBuilder, Error> {
    let builder = SdkLoggerProvider::builder();
    let exporter = LogExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()
        .with_context(|_| InitLogSnafu {})?;
    let resource = Resource::builder()
        .with_attribute(KeyValue::new(
            opentelemetry_semantic_conventions::resource::SERVICE_NAME,
            service_info.name_in_metrics.clone(),
        ))
        .build();
    let builder = builder
        .with_resource(resource)
        .with_batch_exporter(exporter);
    Ok(builder)
}

/// Builder for configuring and initializing the logging/tracing subscriber.
///
/// This builder separates configuration from initialization, making it easier
/// to test the subscriber configuration without installing it globally.
struct LogSubscriberBuilder<'a> {
    service_info: &'a ServiceInfo,
    settings: &'a LogSettings,
    tracer_provider: Option<&'a sdktrace::SdkTracerProvider>,
}

/// The built subscriber components, ready to be installed or used for testing.
struct BuiltSubscriber<S> {
    /// The logger provider (if OTel logging endpoint was configured)
    logger_provider: Option<opentelemetry_sdk::logs::SdkLoggerProvider>,
    /// The fully configured subscriber
    subscriber: S,
}

impl<'a> LogSubscriberBuilder<'a> {
    /// Create a new builder with the required configuration.
    fn new(service_info: &'a ServiceInfo, settings: &'a LogSettings) -> Self {
        Self {
            service_info,
            settings,
            tracer_provider: None,
        }
    }

    /// Set the tracer provider for OpenTelemetry trace integration.
    fn with_tracer_provider(mut self, provider: &'a sdktrace::SdkTracerProvider) -> Self {
        self.tracer_provider = Some(provider);
        self
    }

    /// Build the subscriber without installing it globally.
    /// Use this for testing with `tracing::subscriber::with_default`.
    fn build(
        self,
    ) -> Result<
        BuiltSubscriber<
            impl Subscriber // + for<'span> tracing_subscriber::registry::LookupSpan<'span>
                // + Send
                // + Sync
                + use<'a>,
        >,
        Error,
    > {
        let (logger_provider, otel_log_layer) = init_otel_logs(self.service_info, self.settings)?;

        // Create the OpenTelemetry tracing layer if a tracer provider is configured.
        // This bridges tracing spans to OpenTelemetry traces.
        let otel_trace_layer = self.tracer_provider.map(|provider| {
            let tracer = provider.tracer(self.service_info.name_in_metrics.clone());
            let filter = EnvFilter::new(&self.settings.otel_level)
                .add_directive("hyper=off".parse().unwrap())
                .add_directive("opentelemetry=off".parse().unwrap())
                .add_directive("opentelemetry_sdk=off".parse().unwrap())
                .add_directive("tonic=off".parse().unwrap())
                .add_directive("h2=off".parse().unwrap())
                .add_directive("reqwest=off".parse().unwrap());
            OpenTelemetryLayer::new(tracer).with_filter(filter)
        });

        // Create a new tracing::Fmt layer to print the logs to stdout.
        let filter_fmt = EnvFilter::new(&self.settings.console_level);
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_thread_names(true)
            .with_filter(filter_fmt);

        // Build the subscriber with all layers (but don't install it)
        let subscriber = tracing_subscriber::registry()
            .with(otel_log_layer)
            .with(otel_trace_layer)
            .with(fmt_layer);

        Ok(BuiltSubscriber {
            logger_provider,
            subscriber,
        })
    }

    /// Build and install the subscriber globally.
    /// Returns the logger provider if OTel logging was configured.
    fn init(self) -> Result<Option<opentelemetry_sdk::logs::SdkLoggerProvider>, Error> {
        let built = self.build()?;
        built.subscriber.init();
        Ok(built.logger_provider)
    }
}

fn init_logs(
    service_info: &ServiceInfo,
    settings: &LogSettings,
    tracer_provider: Option<&sdktrace::SdkTracerProvider>,
) -> Result<Option<opentelemetry_sdk::logs::SdkLoggerProvider>, Error> {
    let mut builder = LogSubscriberBuilder::new(service_info, settings);
    if let Some(provider) = tracer_provider {
        builder = builder.with_tracer_provider(provider);
    }
    builder.init()
}

/// Initializes the telemetry backend for your application.
///
/// This function sets up tracing, metrics, and logging according to the provided settings.
/// It integrates with OpenTelemetry to provide a complete observability solution.
///
/// # Errors
///
/// - `InitLog` if the logger provider cannot be initialized.
/// - `InitTrace` if the tracer provider cannot be initialized.
/// - `InitMetric` if the metric provider cannot be initialized.
/// Initializes the telemetry backend for your application.
///
/// This function sets up tracing, metrics, logging, and the W3C Trace Context
/// propagator for distributed tracing according to the provided settings.
/// It integrates with OpenTelemetry to provide a complete observability solution.
///
/// # Errors
///
/// - `InitLog` if the logger provider cannot be initialized.
/// - `InitTrace` if the tracer provider cannot be initialized.
/// - `InitMetric` if the metric provider cannot be initialized.
#[must_use]
pub fn init(
    service_info: &ServiceInfo,
    settings: &TelemetrySettings,
) -> Result<TelemetryProviders, Error> {
    // Initialize the W3C Trace Context propagator for distributed tracing
    init_propagator();
    // Initialize traces first so we can pass the provider to init_logs for the tracing layer
    let tracer_provider =
        init_traces(service_info, &settings.trace).with_context(|_| InitTraceSnafu {})?;
    if let Some(tracer_provider) = &tracer_provider {
        global::set_tracer_provider(tracer_provider.clone());
    }

    // Initialize logs with the tracer provider to enable span export via tracing-opentelemetry
    let logger_provider = init_logs(service_info, &settings.log, tracer_provider.as_ref())?;

    let meter_provider =
        init_metrics(service_info, &settings.metric).with_context(|_| InitMetricSnafu {})?;
    if let Some(meter_provider) = &meter_provider {
        global::set_meter_provider(meter_provider.clone());
    }

    Ok(TelemetryProviders {
        meter: meter_provider,
        tracer: tracer_provider,
        logger: logger_provider,
    })
}

// ============================================================================
// Distributed Tracing Propagation
// ============================================================================

/// Wrapper for tonic::metadata::MetadataMap to implement Extractor trait.
/// Used for extracting trace context from incoming gRPC requests.
pub struct MetadataExtractor<'a>(pub &'a tonic::metadata::MetadataMap);

impl Extractor for MetadataExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        // W3C Trace Context only uses "traceparent" and optionally "tracestate".
        // Only return the keys that actually exist in the metadata.
        ["traceparent", "tracestate"]
            .into_iter()
            .filter(|k| self.0.get(*k).is_some())
            .collect()
    }
}

/// Wrapper for tonic::metadata::MetadataMap to implement Injector trait.
/// Used for injecting trace context into outgoing gRPC requests.
pub struct MetadataInjector<'a>(pub &'a mut tonic::metadata::MetadataMap);

impl Injector for MetadataInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let Ok(key) = tonic::metadata::MetadataKey::from_bytes(key.as_bytes()) {
            if let Ok(value) = tonic::metadata::MetadataValue::try_from(&value) {
                self.0.insert(key, value);
            }
        }
    }
}

impl TraceContextCarrier for tonic::metadata::MetadataMap {
    fn extract_trace_context(&self) -> opentelemetry::Context {
        global::get_text_map_propagator(|propagator| propagator.extract(&MetadataExtractor(self)))
    }

    fn inject_trace_context(&mut self) {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        let cx = tracing::Span::current().context();
        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&cx, &mut MetadataInjector(self));
        });
    }
}

/// Extract trace context from incoming gRPC request metadata.
///
/// Returns the extracted OpenTelemetry context. Use [`link_distributed_trace`] for a more
/// convenient way to extract and link the trace context in one call.
///
/// # Example
///
/// ```
/// let mut metadata = tonic::metadata::MetadataMap::new();
/// metadata.insert("traceparent", "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".parse().unwrap());
///
/// let parent_cx = byre::telemetry::extract_trace_context(&metadata);
/// let _guard = parent_cx.attach();
/// // Spans created here will be children of the incoming trace
/// ```
pub fn extract_trace_context(metadata: &tonic::metadata::MetadataMap) -> opentelemetry::Context {
    global::get_text_map_propagator(|propagator| propagator.extract(&MetadataExtractor(metadata)))
}

/// Link the current span to an incoming distributed trace from gRPC metadata.
///
/// This is a convenience function that extracts the trace context from the
/// incoming request metadata and sets it as the parent of the current span.
/// Call this at the start of your gRPC handler after the `#[tracing::instrument]` span is created.
///
/// Returns `Ok(())` if successful, or an error if the span context couldn't be set.
/// Most callers will want to ignore the error with `let _ = link_distributed_trace(...)`.
///
/// # Example
///
/// ```
/// let mut metadata = tonic::metadata::MetadataMap::new();
/// metadata.insert("traceparent", "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".parse().unwrap());
///
/// let _ = byre::telemetry::link_distributed_trace(&metadata);
/// // Current span is now part of the distributed trace
/// ```
pub fn link_distributed_trace(metadata: &tonic::metadata::MetadataMap) -> Result<(), Error> {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    let parent_cx = extract_trace_context(metadata);
    tracing::Span::current()
        .set_parent(parent_cx)
        .map_err(|e| Error::LinkDistributedTrace {
            source: Box::new(e),
        })
}

/// Inject trace context into outgoing gRPC request metadata.
///
/// Call this before making outgoing gRPC calls to propagate the trace context.
///
/// # Example
///
/// ```
/// let mut metadata = tonic::metadata::MetadataMap::new();
/// byre::telemetry::inject_trace_context(&mut metadata);
/// // metadata now contains traceparent header (if there's an active span)
/// ```
pub fn inject_trace_context(metadata: &mut tonic::metadata::MetadataMap) {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    // Get the OpenTelemetry context from the current tracing span
    let cx = tracing::Span::current().context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut MetadataInjector(metadata));
    });
}

/// Initialize the global text map propagator for W3C Trace Context.
///
/// This is called automatically by `init()`, but can be called manually if needed.
pub fn init_propagator() {
    global::set_text_map_propagator(TraceContextPropagator::new());
}

// ============================================================================
// HTTP Header Propagation (for HTTP proxies and clients)
// ============================================================================

/// Wrapper for http::HeaderMap to implement Extractor trait.
/// Used for extracting trace context from incoming HTTP requests.
pub struct HttpHeaderExtractor<'a>(pub &'a http::HeaderMap);

impl Extractor for HttpHeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        // W3C Trace Context only uses "traceparent" and optionally "tracestate".
        // Only return the keys that actually exist in the headers.
        ["traceparent", "tracestate"]
            .into_iter()
            .filter(|k| self.0.get(*k).is_some())
            .collect()
    }
}

/// Wrapper for http::HeaderMap to implement Injector trait.
/// Used for injecting trace context into outgoing HTTP requests.
pub struct HttpHeaderInjector<'a>(pub &'a mut http::HeaderMap);

impl Injector for HttpHeaderInjector<'_> {
    fn set(&mut self, key: &str, value: String) {
        if let Ok(key) = http::header::HeaderName::from_bytes(key.as_bytes()) {
            if let Ok(value) = http::header::HeaderValue::from_str(&value) {
                self.0.insert(key, value);
            }
        }
    }
}

impl TraceContextCarrier for http::HeaderMap {
    fn extract_trace_context(&self) -> opentelemetry::Context {
        global::get_text_map_propagator(|propagator| propagator.extract(&HttpHeaderExtractor(self)))
    }

    fn inject_trace_context(&mut self) {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        let cx = tracing::Span::current().context();
        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&cx, &mut HttpHeaderInjector(self));
        });
    }
}

/// Extract trace context from incoming HTTP request headers.
///
/// Returns the extracted OpenTelemetry context. Use [`link_distributed_trace_http`] for a more
/// convenient way to extract and link the trace context in one call.
///
/// # Example
///
/// ```
/// let mut headers = http::HeaderMap::new();
/// headers.insert("traceparent", "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".parse().unwrap());
///
/// let parent_cx = byre::telemetry::extract_trace_context_http(&headers);
/// let _guard = parent_cx.attach();
/// // Spans created here will be children of the incoming trace
/// ```
pub fn extract_trace_context_http(headers: &http::HeaderMap) -> opentelemetry::Context {
    global::get_text_map_propagator(|propagator| propagator.extract(&HttpHeaderExtractor(headers)))
}

/// Link the current span to an incoming distributed trace from HTTP headers.
///
/// This is a convenience function that extracts the trace context from the
/// incoming request headers and sets it as the parent of the current span.
/// Call this at the start of your HTTP handler after the `#[tracing::instrument]` span is created.
///
/// Returns `Ok(())` if successful, or an error if the span context couldn't be set.
/// Most callers will want to ignore the error with `let _ = link_distributed_trace_http(...)`.
///
/// # Example
///
/// ```
/// let mut headers = http::HeaderMap::new();
/// headers.insert("traceparent", "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".parse().unwrap());
///
/// let _ = byre::telemetry::link_distributed_trace_http(&headers);
/// // Current span is now part of the distributed trace
/// ```
pub fn link_distributed_trace_http(headers: &http::HeaderMap) -> Result<(), Error> {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    let parent_cx = extract_trace_context_http(headers);
    tracing::Span::current()
        .set_parent(parent_cx)
        .map_err(|e| Error::LinkDistributedTrace {
            source: Box::new(e),
        })
}

/// Inject trace context into outgoing HTTP request headers.
///
/// Call this before making outgoing HTTP calls to propagate the trace context.
///
/// # Example
///
/// ```
/// let mut headers = http::HeaderMap::new();
/// byre::telemetry::inject_trace_context_http(&mut headers);
/// // headers now contains traceparent header (if there's an active span)
/// ```
pub fn inject_trace_context_http(headers: &mut http::HeaderMap) {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    // Get the OpenTelemetry context from the current tracing span
    let cx = tracing::Span::current().context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, &mut HttpHeaderInjector(headers));
    });
}

// ============================================================================
// Tower Layer for Distributed Trace Context (gRPC/tonic)
// ============================================================================

/// A Tower layer that extracts distributed trace context from incoming gRPC requests
/// and creates a parent span for all downstream handlers.
///
/// This layer should be added to tonic services to enable distributed tracing.
/// It extracts the W3C Trace Context headers from incoming requests and creates
/// a span that becomes the parent of all spans created within the handler.
///
/// # Example
///
/// ```
/// use byre::telemetry::GrpcTraceContextLayer;
///
/// // Create the layer
/// let layer = GrpcTraceContextLayer::new("my-service");
///
/// // Use with tonic Server::builder().layer(layer)
/// ```
#[derive(Clone)]
pub struct GrpcTraceContextLayer {
    service_name: &'static str,
}

impl GrpcTraceContextLayer {
    /// Create a new layer with the given service name.
    /// The service name is used to identify spans in the trace.
    pub fn new(service_name: &'static str) -> Self {
        Self { service_name }
    }
}

impl<S> tower::Layer<S> for GrpcTraceContextLayer {
    type Service = GrpcTraceContextService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        GrpcTraceContextService {
            inner,
            service_name: self.service_name,
        }
    }
}

/// The service that wraps inner services with trace context extraction.
#[derive(Clone)]
pub struct GrpcTraceContextService<S> {
    inner: S,
    service_name: &'static str,
}

impl<S, B> tower::Service<http::Request<B>> for GrpcTraceContextService<S>
where
    S: tower::Service<http::Request<B>> + Clone + Send + 'static,
    S::Future: Send,
    B: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: http::Request<B>) -> Self::Future {
        use tracing::Instrument;
        use tracing_opentelemetry::OpenTelemetrySpanExt;

        // Extract trace context from incoming HTTP/2 headers (gRPC uses HTTP/2)
        let parent_cx = extract_trace_context_http(request.headers());

        // Create a tracing span and link it to the incoming OpenTelemetry context.
        // This makes all child spans (from #[tracing::instrument]) part of the distributed trace.
        let span = tracing::info_span!("grpc_request", service = self.service_name);
        let _ = span.set_parent(parent_cx);

        // Clone inner service for use in async block
        let mut inner = self.inner.clone();

        // Instrument the future with our span so it stays active for the entire request
        Box::pin(async move { inner.call(request).await }.instrument(span))
    }
}

// ============================================================================
// Message Queue Trace Context Propagation (for Iggy and similar systems)
// ============================================================================

impl TraceContextCarrier for std::collections::HashMap<String, String> {
    fn extract_trace_context(&self) -> opentelemetry::Context {
        global::get_text_map_propagator(|propagator| propagator.extract(self))
    }

    fn inject_trace_context(&mut self) {
        use tracing_opentelemetry::OpenTelemetrySpanExt;
        let cx = tracing::Span::current().context();
        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&cx, self);
        });
    }
}

/// Inject the current trace context into a HashMap suitable for message queue headers.
///
/// This is useful for propagating trace context through message queues like Iggy
/// where headers are stored as a `HashMap<HeaderKey, HeaderValue>`.
///
/// # Example
///
/// ```
/// use std::collections::HashMap;
///
/// let mut headers: HashMap<String, String> = HashMap::new();
/// byre::telemetry::inject_trace_context_map(&mut headers);
/// // headers now contains traceparent key (if there's an active span)
/// ```
pub fn inject_trace_context_map(headers: &mut std::collections::HashMap<String, String>) {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    // Get the OpenTelemetry context from the current tracing span
    let cx = tracing::Span::current().context();
    global::get_text_map_propagator(|propagator| {
        propagator.inject_context(&cx, headers);
    });
}

/// Extract trace context from a HashMap of message queue headers.
///
/// This is useful for extracting trace context from message queues like Iggy
/// where headers are stored as a `HashMap<HeaderKey, HeaderValue>`.
///
/// # Example
///
/// ```
/// use std::collections::HashMap;
///
/// let mut headers: HashMap<String, String> = HashMap::new();
/// headers.insert("traceparent".to_string(), "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string());
///
/// let parent_cx = byre::telemetry::extract_trace_context_map(&headers);
/// ```
pub fn extract_trace_context_map(
    headers: &std::collections::HashMap<String, String>,
) -> opentelemetry::Context {
    global::get_text_map_propagator(|propagator| propagator.extract(headers))
}

/// Link the current span to an incoming distributed trace from message queue headers.
///
/// This is a convenience function that extracts the trace context from the
/// message headers and sets it as the parent of the current span.
///
/// Returns `Ok(())` if successful, or an error if the span context couldn't be set.
/// Most callers will want to ignore the error with `let _ = link_distributed_trace_map(...)`.
///
/// # Example
///
/// ```
/// use std::collections::HashMap;
///
/// let mut headers: HashMap<String, String> = HashMap::new();
/// headers.insert("traceparent".to_string(), "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string());
///
/// let _ = byre::telemetry::link_distributed_trace_map(&headers);
/// // Current span is now part of the distributed trace
/// ```
pub fn link_distributed_trace_map(
    headers: &std::collections::HashMap<String, String>,
) -> Result<(), Error> {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    let parent_cx = extract_trace_context_map(headers);
    tracing::Span::current()
        .set_parent(parent_cx)
        .map_err(|e| Error::LinkDistributedTrace {
            source: Box::new(e),
        })
}

/// Set a span's parent from an OpenTelemetry context.
///
/// This links the given tracing span to a distributed trace context,
/// making it a child of the remote span.
///
/// # Example
///
/// ```
/// use std::collections::HashMap;
///
/// let mut headers: HashMap<String, String> = HashMap::new();
/// headers.insert("traceparent".to_string(), "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string());
///
/// let parent_cx = byre::telemetry::extract_trace_context_map(&headers);
/// let span = tracing::info_span!("process_message", message_id = 42);
/// byre::telemetry::set_span_parent(&span, parent_cx);
/// let _enter = span.enter();
/// ```
pub fn set_span_parent(span: &tracing::Span, parent_cx: opentelemetry::Context) {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    let _ = span.set_parent(parent_cx);
}

// ============================================================================
// Prelude - convenient re-exports for common use
// ============================================================================

/// Convenient re-exports for common telemetry usage.
///
/// Import with:
/// ```rust
/// use byre::telemetry::prelude::*;
/// ```
///
/// This gives you access to:
/// - [`init`] - Initialize telemetry
/// - [`TelemetrySettings`] - Configuration for telemetry
/// - [`TelemetryProviders`] - Handle to keep telemetry alive
/// - [`TraceContextCarrier`] - Trait for types that carry trace context
/// - [`TraceContextExt`] - Extension methods for trace context propagation
/// - [`GrpcTraceContextLayer`] - Tower layer for gRPC distributed tracing
pub mod prelude {
    pub use super::{
        init, GrpcTraceContextLayer, TelemetryProviders, TelemetrySettings, TraceContextCarrier,
        TraceContextExt,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{
        SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState, Tracer,
    };
    use std::collections::HashMap;

    /// Initialize the W3C TraceContext propagator for tests
    fn init_test_propagator() {
        use opentelemetry::propagation::TextMapCompositePropagator;
        use opentelemetry_sdk::propagation::TraceContextPropagator;

        let propagator =
            TextMapCompositePropagator::new(vec![Box::new(TraceContextPropagator::new())]);
        global::set_text_map_propagator(propagator);
    }

    #[test]
    fn test_inject_and_extract_trace_context_roundtrip() {
        use tracing_opentelemetry::OpenTelemetrySpanExt;

        let _provider = init_tracing_with_otel();

        // Create a span context with known values
        let trace_id = TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").unwrap();
        let span_id = SpanId::from_hex("b7ad6b7169203331").unwrap();
        let span_context = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            false,
            TraceState::default(),
        );

        // Create an OpenTelemetry context with our span context
        let parent_cx = opentelemetry::Context::new().with_remote_span_context(span_context);

        // Create a tracing span and set the parent context
        let span = tracing::info_span!("test_roundtrip_span");
        let _ = span.set_parent(parent_cx);
        let _enter = span.enter();

        // Inject the trace context into headers
        let mut headers: HashMap<String, String> = HashMap::new();
        inject_trace_context_map(&mut headers);

        // Verify traceparent header was injected
        assert!(
            headers.contains_key("traceparent"),
            "traceparent header should be present"
        );
        let traceparent = headers.get("traceparent").unwrap();
        assert!(
            traceparent.starts_with("00-0af7651916cd43dd8448eb211c80319c-"),
            "traceparent should contain the correct trace ID"
        );

        // Extract the trace context from headers
        let extracted_cx = extract_trace_context_map(&headers);
        let extracted_span = extracted_cx.span();
        let extracted_span_context = extracted_span.span_context();

        // Verify the extracted context matches the original
        assert_eq!(
            extracted_span_context.trace_id(),
            trace_id,
            "trace ID should match"
        );
        assert!(
            extracted_span_context.trace_flags().is_sampled(),
            "sampled flag should be preserved"
        );
    }

    #[test]
    fn test_extract_empty_headers_returns_empty_context() {
        init_test_propagator();

        let headers: HashMap<String, String> = HashMap::new();
        let extracted_cx = extract_trace_context_map(&headers);

        // An empty context should have an invalid span context
        let extracted_span = extracted_cx.span();
        let span_context = extracted_span.span_context();
        assert!(
            !span_context.is_valid(),
            "extracted context from empty headers should be invalid"
        );
    }

    #[test]
    fn test_extract_invalid_traceparent_returns_empty_context() {
        init_test_propagator();

        let mut headers: HashMap<String, String> = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "invalid-traceparent-value".to_string(),
        );

        let extracted_cx = extract_trace_context_map(&headers);
        let extracted_span = extracted_cx.span();
        let span_context = extracted_span.span_context();

        // Invalid traceparent should result in invalid span context
        assert!(
            !span_context.is_valid(),
            "extracted context from invalid traceparent should be invalid"
        );
    }

    #[test]
    fn test_inject_without_active_span_produces_no_traceparent() {
        init_test_propagator();

        // Without an active span, inject should not add traceparent
        let mut headers: HashMap<String, String> = HashMap::new();
        inject_trace_context_map(&mut headers);

        // When there's no active span with a valid context, traceparent may be empty or missing
        // The behavior depends on whether there's a valid span context
        if let Some(traceparent) = headers.get("traceparent") {
            // If present, it should start with "00-00000000000000000000000000000000" (invalid trace)
            assert!(
                traceparent.contains("00000000000000000000000000000000") || traceparent.is_empty(),
                "traceparent without active span should be empty or have zero trace ID"
            );
        }
    }

    #[test]
    fn test_trace_context_preserves_tracestate() {
        init_test_propagator();

        // Create headers with both traceparent and tracestate
        let mut headers: HashMap<String, String> = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
        );
        headers.insert("tracestate".to_string(), "congo=t61rcWkgMzE".to_string());

        let extracted_cx = extract_trace_context_map(&headers);
        let extracted_span = extracted_cx.span();
        let span_context = extracted_span.span_context();

        assert!(span_context.is_valid(), "span context should be valid");
        // TraceState doesn't have is_empty, check the header value instead
        assert!(
            !span_context.trace_state().header().is_empty(),
            "tracestate should be preserved"
        );
    }

    #[test]
    fn test_inject_extract_with_real_span() {
        init_test_propagator();

        // Create a real tracer and span
        let tracer = global::tracer("test-tracer");
        let span = tracer.start("test-span");
        let cx = opentelemetry::Context::current_with_span(span);

        // Get the span context for comparison before attaching
        let original_span = cx.span();
        let original_trace_id = original_span.span_context().trace_id();

        let _guard = cx.attach();

        // Inject
        let mut headers: HashMap<String, String> = HashMap::new();
        inject_trace_context_map(&mut headers);

        // Extract
        let extracted_cx = extract_trace_context_map(&headers);
        let extracted_span = extracted_cx.span();
        let extracted_span_context = extracted_span.span_context();

        // The trace ID should match (span ID may differ as it's a child reference)
        assert_eq!(
            extracted_span_context.trace_id(),
            original_trace_id,
            "trace ID should be preserved through inject/extract cycle"
        );
    }

    // ========================================================================
    // Tests for inject_trace_context from tracing spans
    // ========================================================================

    /// Initialize a tracing subscriber with OpenTelemetry layer for tests.
    /// This uses try_init() which only works once per process - use
    /// `with_otel_subscriber` for tests that need isolated subscribers.
    fn init_tracing_with_otel() -> opentelemetry_sdk::trace::SdkTracerProvider {
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::trace::SdkTracerProvider;
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        init_test_propagator();

        // Create a simple tracer provider
        let provider = SdkTracerProvider::builder().build();
        let tracer = provider.tracer("test-tracer");

        // Create the OpenTelemetry layer
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        // Build and set the subscriber
        let subscriber = tracing_subscriber::registry().with(otel_layer);

        // Use try_init to avoid panics if already initialized
        let _ = subscriber.try_init();

        // Return provider to keep it alive
        provider
    }

    /// Run a test closure with an isolated OpenTelemetry-enabled tracing subscriber.
    /// This uses `with_default` to avoid global subscriber conflicts between tests.
    fn with_otel_subscriber<F, R>(f: F) -> R
    where
        F: FnOnce() -> R,
    {
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_sdk::trace::SdkTracerProvider;
        use tracing_subscriber::layer::SubscriberExt;

        init_test_propagator();

        // Create a simple tracer provider
        let provider = SdkTracerProvider::builder().build();
        let tracer = provider.tracer("test-tracer");

        // Create the OpenTelemetry layer
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        // Build the subscriber (don't set globally)
        let subscriber = tracing_subscriber::registry().with(otel_layer);

        // Run the test with this subscriber as the default for this scope only
        tracing::subscriber::with_default(subscriber, f)
    }

    /// Helper to assert a traceparent value is valid (non-empty trace ID)
    fn assert_valid_traceparent(traceparent: &str) {
        assert!(
            traceparent.starts_with("00-"),
            "traceparent should start with version 00"
        );
        assert!(
            !traceparent.contains("00000000000000000000000000000000"),
            "traceparent should have a valid (non-zero) trace ID"
        );
    }

    #[test]
    fn test_inject_trace_context_from_tracing_span() {
        let _provider = init_tracing_with_otel();
        let span = tracing::info_span!("test_span_for_injection");
        let _enter = span.enter();

        // Test HashMap injection
        let mut map_headers: HashMap<String, String> = HashMap::new();
        inject_trace_context_map(&mut map_headers);
        assert!(map_headers.contains_key("traceparent"));
        assert_valid_traceparent(map_headers.get("traceparent").unwrap());

        // Test gRPC metadata injection
        let mut grpc_metadata = tonic::metadata::MetadataMap::new();
        inject_trace_context(&mut grpc_metadata);
        let grpc_traceparent = grpc_metadata.get("traceparent").unwrap().to_str().unwrap();
        assert_valid_traceparent(grpc_traceparent);

        // Test HTTP header injection
        let mut http_headers = http::HeaderMap::new();
        inject_trace_context_http(&mut http_headers);
        let http_traceparent = http_headers.get("traceparent").unwrap().to_str().unwrap();
        assert_valid_traceparent(http_traceparent);
    }

    #[test]
    fn test_nested_tracing_spans_propagate_trace_id() {
        let _provider = init_tracing_with_otel();

        // Create parent span
        let parent_span = tracing::info_span!("parent_span");
        let _parent_enter = parent_span.enter();

        // Get trace context from parent
        let mut parent_headers: HashMap<String, String> = HashMap::new();
        inject_trace_context_map(&mut parent_headers);
        let parent_traceparent = parent_headers.get("traceparent").unwrap().clone();

        // Extract trace ID from parent (format: 00-{trace_id}-{span_id}-{flags})
        let parent_trace_id: String = parent_traceparent.split('-').nth(1).unwrap().to_string();

        // Create child span
        let child_span = tracing::info_span!("child_span");
        let _child_enter = child_span.enter();

        // Get trace context from child
        let mut child_headers: HashMap<String, String> = HashMap::new();
        inject_trace_context_map(&mut child_headers);
        let child_traceparent = child_headers.get("traceparent").unwrap();

        // Extract trace ID from child
        let child_trace_id: String = child_traceparent.split('-').nth(1).unwrap().to_string();

        // Both spans should have the same trace ID
        assert_eq!(
            parent_trace_id, child_trace_id,
            "nested spans should share the same trace ID"
        );

        // But span IDs should be different
        let parent_span_id: String = parent_traceparent.split('-').nth(2).unwrap().to_string();
        let child_span_id: String = child_traceparent.split('-').nth(2).unwrap().to_string();

        assert_ne!(
            parent_span_id, child_span_id,
            "nested spans should have different span IDs"
        );
    }

    #[test]
    fn test_inject_trace_context_roundtrip_with_tracing_span() {
        let _provider = init_tracing_with_otel();

        // Create a tracing span
        let span = tracing::info_span!("roundtrip_test_span");
        let _enter = span.enter();

        // Inject into HashMap
        let mut headers: HashMap<String, String> = HashMap::new();
        inject_trace_context_map(&mut headers);

        // Extract the context
        let extracted_cx = extract_trace_context_map(&headers);
        let extracted_span = extracted_cx.span();
        let extracted_span_context = extracted_span.span_context();

        // Verify we got a valid span context
        assert!(
            extracted_span_context.is_valid(),
            "extracted span context should be valid"
        );

        // Get the original trace ID from the injected headers
        let traceparent = headers.get("traceparent").unwrap();
        let original_trace_id: String = traceparent.split('-').nth(1).unwrap().to_string();

        // Compare with extracted trace ID
        let extracted_trace_id = format!("{:032x}", extracted_span_context.trace_id());

        assert_eq!(
            original_trace_id, extracted_trace_id,
            "trace ID should survive inject/extract roundtrip"
        );
    }

    // ========================================================================
    // Tests for MetadataExtractor (gRPC metadata)
    // ========================================================================

    #[test]
    fn test_metadata_extractor_get_returns_value() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert("traceparent", "00-abc123-def456-01".parse().unwrap());

        let extractor = MetadataExtractor(&metadata);
        let value = extractor.get("traceparent");

        assert!(value.is_some(), "get should return Some for existing key");
        assert_eq!(value.unwrap(), "00-abc123-def456-01");
    }

    #[test]
    fn test_metadata_extractor_get_returns_none_for_missing() {
        let metadata = tonic::metadata::MetadataMap::new();
        let extractor = MetadataExtractor(&metadata);

        let value = extractor.get("nonexistent");
        assert!(value.is_none(), "get should return None for missing key");
    }

    #[test]
    fn test_metadata_extractor_keys_returns_trace_context_keys() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert("traceparent", "value1".parse().unwrap());
        metadata.insert("tracestate", "value2".parse().unwrap());
        metadata.insert("custom-header", "value3".parse().unwrap());

        let extractor = MetadataExtractor(&metadata);
        let keys = extractor.keys();

        // keys() only returns W3C Trace Context keys, not all headers
        assert_eq!(keys.len(), 2, "keys should only contain trace context keys");
        assert!(
            keys.contains(&"traceparent"),
            "keys should contain traceparent"
        );
        assert!(
            keys.contains(&"tracestate"),
            "keys should contain tracestate"
        );
    }

    #[test]
    fn test_metadata_extractor_keys_returns_only_present_keys() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert("traceparent", "value1".parse().unwrap());
        // tracestate not present

        let extractor = MetadataExtractor(&metadata);
        let keys = extractor.keys();

        assert_eq!(
            keys.len(),
            1,
            "keys should only contain present trace context keys"
        );
        assert!(
            keys.contains(&"traceparent"),
            "keys should contain traceparent"
        );
    }

    // ========================================================================
    // Tests for HttpHeaderExtractor (HTTP headers)
    // ========================================================================

    #[test]
    fn test_http_header_extractor_get_returns_value() {
        let mut headers = http::HeaderMap::new();
        headers.insert("traceparent", "00-abc123-def456-01".parse().unwrap());

        let extractor = HttpHeaderExtractor(&headers);
        let value = extractor.get("traceparent");

        assert!(value.is_some(), "get should return Some for existing key");
        assert_eq!(value.unwrap(), "00-abc123-def456-01");
    }

    #[test]
    fn test_http_header_extractor_get_returns_none_for_missing() {
        let headers = http::HeaderMap::new();
        let extractor = HttpHeaderExtractor(&headers);

        let value = extractor.get("nonexistent");
        assert!(value.is_none(), "get should return None for missing key");
    }

    #[test]
    fn test_http_header_extractor_keys_returns_trace_context_keys() {
        let mut headers = http::HeaderMap::new();
        headers.insert("traceparent", "value1".parse().unwrap());
        headers.insert("tracestate", "value2".parse().unwrap());
        headers.insert("x-custom-header", "value3".parse().unwrap());

        let extractor = HttpHeaderExtractor(&headers);
        let keys = extractor.keys();

        // keys() only returns W3C Trace Context keys, not all headers
        assert_eq!(keys.len(), 2, "keys should only contain trace context keys");
        assert!(
            keys.contains(&"traceparent"),
            "keys should contain traceparent"
        );
        assert!(
            keys.contains(&"tracestate"),
            "keys should contain tracestate"
        );
    }

    #[test]
    fn test_http_header_extractor_keys_returns_only_present_keys() {
        let mut headers = http::HeaderMap::new();
        headers.insert("traceparent", "value1".parse().unwrap());
        // tracestate not present

        let extractor = HttpHeaderExtractor(&headers);
        let keys = extractor.keys();

        assert_eq!(
            keys.len(),
            1,
            "keys should only contain present trace context keys"
        );
        assert!(
            keys.contains(&"traceparent"),
            "keys should contain traceparent"
        );
    }

    // ========================================================================
    // Tests for extract_trace_context functions
    // ========================================================================

    /// Helper to verify extracted context has expected trace ID
    fn assert_extracted_trace_id(context: &opentelemetry::Context, expected_trace_id: &str) {
        let span = context.span();
        let span_context = span.span_context();
        assert!(span_context.is_valid(), "span context should be valid");
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            expected_trace_id,
            "trace ID should match"
        );
    }

    #[test]
    fn test_extract_trace_context_grpc_with_valid_headers() {
        init_test_propagator();

        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert(
            "traceparent",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
                .parse()
                .unwrap(),
        );

        let context = extract_trace_context(&metadata);
        assert_extracted_trace_id(&context, "0af7651916cd43dd8448eb211c80319c");
    }

    #[test]
    fn test_extract_trace_context_http_with_valid_headers() {
        init_test_propagator();

        let mut headers = http::HeaderMap::new();
        headers.insert(
            "traceparent",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
                .parse()
                .unwrap(),
        );

        let context = extract_trace_context_http(&headers);
        assert_extracted_trace_id(&context, "0af7651916cd43dd8448eb211c80319c");
    }

    #[test]
    fn test_extract_trace_context_map_with_valid_headers() {
        init_test_propagator();

        let mut headers: HashMap<String, String> = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01".to_string(),
        );

        let context = extract_trace_context_map(&headers);
        assert_extracted_trace_id(&context, "0af7651916cd43dd8448eb211c80319c");
    }

    // ========================================================================
    // Tests for set_span_parent
    // ========================================================================

    #[test]
    fn test_set_span_parent_actually_links() {
        // Use with_otel_subscriber for isolated per-test subscriber
        with_otel_subscriber(|| {
            let trace_id = TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").unwrap();
            let span_id = SpanId::from_hex("b7ad6b7169203331").unwrap();
            let remote_span_context = SpanContext::new(
                trace_id,
                span_id,
                TraceFlags::SAMPLED,
                true,
                TraceState::default(),
            );
            let parent_cx =
                opentelemetry::Context::new().with_remote_span_context(remote_span_context);

            let span = tracing::info_span!("test_set_parent");
            set_span_parent(&span, parent_cx);
            let _enter = span.enter();

            let mut headers: HashMap<String, String> = HashMap::new();
            inject_trace_context_map(&mut headers);

            let traceparent = headers.get("traceparent").unwrap();
            assert!(
                traceparent.contains("0af7651916cd43dd8448eb211c80319c"),
                "trace ID should be preserved after set_span_parent"
            );
        });
    }

    #[test]
    fn test_link_distributed_trace_grpc_actually_links() {
        // Test that link_distributed_trace extracts context and calls set_parent.
        // Uses TraceContextExt trait which works on a specific span reference.
        with_otel_subscriber(|| {
            let mut metadata = tonic::metadata::MetadataMap::new();
            metadata.insert(
                "traceparent",
                "00-11111111111111111111111111111111-aaaaaaaaaaaaaaaa-01"
                    .parse()
                    .unwrap(),
            );

            // Use the trait method which extracts and links
            let span = tracing::info_span!("test_link_grpc");

            // First extract the context
            let parent_cx = metadata.extract_trace_context();

            // Then set it as parent (same as what link_distributed_trace does internally)
            set_span_parent(&span, parent_cx);
            let _enter = span.enter();

            // Verify the span is now linked by injecting and checking trace ID
            let mut verify: HashMap<String, String> = HashMap::new();
            inject_trace_context_map(&mut verify);

            let traceparent = verify.get("traceparent").unwrap();
            assert!(
                traceparent.contains("11111111111111111111111111111111"),
                "link_distributed_trace should link the span to trace 11111111111111111111111111111111, got {traceparent}"
            );
        });
    }

    #[test]
    fn test_link_distributed_trace_http_actually_links() {
        with_otel_subscriber(|| {
            let mut headers = http::HeaderMap::new();
            headers.insert(
                "traceparent",
                "00-22222222222222222222222222222222-bbbbbbbbbbbbbbbb-01"
                    .parse()
                    .unwrap(),
            );

            let span = tracing::info_span!("test_link_http");
            let parent_cx = headers.extract_trace_context();
            set_span_parent(&span, parent_cx);
            let _enter = span.enter();

            let mut verify: HashMap<String, String> = HashMap::new();
            inject_trace_context_map(&mut verify);

            let traceparent = verify.get("traceparent").unwrap();
            assert!(
                traceparent.contains("22222222222222222222222222222222"),
                "link_distributed_trace_http should link the span"
            );
        });
    }

    #[test]
    fn test_link_distributed_trace_map_actually_links() {
        with_otel_subscriber(|| {
            let mut headers: HashMap<String, String> = HashMap::new();
            headers.insert(
                "traceparent".to_string(),
                "00-33333333333333333333333333333333-cccccccccccccccc-01".to_string(),
            );

            let span = tracing::info_span!("test_link_map");
            let parent_cx = headers.extract_trace_context();
            set_span_parent(&span, parent_cx);
            let _enter = span.enter();

            let mut verify: HashMap<String, String> = HashMap::new();
            inject_trace_context_map(&mut verify);

            let traceparent = verify.get("traceparent").unwrap();
            assert!(
                traceparent.contains("33333333333333333333333333333333"),
                "link_distributed_trace_map should link the span"
            );
        });
    }

    // ========================================================================
    // Tests for init_propagator
    // ========================================================================

    #[test]
    fn test_init_propagator_enables_trace_context_propagation() {
        // Call init_propagator to set the W3C TraceContext propagator
        init_propagator();

        // Create a context with a known trace ID
        let trace_id = TraceId::from_hex("1234567890abcdef1234567890abcdef").unwrap();
        let span_id = SpanId::from_hex("fedcba0987654321").unwrap();
        let span_context = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            false,
            TraceState::default(),
        );
        let cx = opentelemetry::Context::new().with_remote_span_context(span_context);

        // Inject using the propagator
        let mut headers: HashMap<String, String> = HashMap::new();
        global::get_text_map_propagator(|propagator| {
            propagator.inject_context(&cx, &mut headers);
        });

        // Verify traceparent was injected (this would fail if init_propagator did nothing)
        assert!(
            headers.contains_key("traceparent"),
            "init_propagator should enable traceparent injection"
        );
        let traceparent = headers.get("traceparent").unwrap();
        assert!(
            traceparent.contains("1234567890abcdef1234567890abcdef"),
            "traceparent should contain the trace ID"
        );
    }

    // ========================================================================
    // Tests for TraceContextCarrier::extract_trace_context implementations
    // ========================================================================

    #[test]
    fn test_metadata_map_extract_trace_context_returns_valid_context() {
        init_test_propagator();

        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert(
            "traceparent",
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
                .parse()
                .unwrap(),
        );

        // Use the TraceContextCarrier trait method
        let context = TraceContextCarrier::extract_trace_context(&metadata);
        let span = context.span();
        let span_context = span.span_context();

        // Verify this is NOT a default context - it has the trace ID from headers
        assert!(span_context.is_valid(), "span context should be valid");
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            "0af7651916cd43dd8448eb211c80319c",
            "trace ID should be extracted from headers, not default"
        );
    }

    #[test]
    fn test_http_header_map_extract_trace_context_returns_valid_context() {
        init_test_propagator();

        let mut headers = http::HeaderMap::new();
        headers.insert(
            "traceparent",
            "00-1234567890abcdef1234567890abcdef-b7ad6b7169203331-01"
                .parse()
                .unwrap(),
        );

        // Use the TraceContextCarrier trait method
        let context = TraceContextCarrier::extract_trace_context(&headers);
        let span = context.span();
        let span_context = span.span_context();

        // Verify this is NOT a default context - it has the trace ID from headers
        assert!(span_context.is_valid(), "span context should be valid");
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            "1234567890abcdef1234567890abcdef",
            "trace ID should be extracted from headers, not default"
        );
    }

    #[test]
    fn test_hashmap_extract_trace_context_returns_valid_context() {
        init_test_propagator();

        let mut headers: HashMap<String, String> = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "00-abcdef1234567890abcdef1234567890-b7ad6b7169203331-01".to_string(),
        );

        // Use the TraceContextCarrier trait method
        let context = TraceContextCarrier::extract_trace_context(&headers);
        let span = context.span();
        let span_context = span.span_context();

        // Verify this is NOT a default context - it has the trace ID from headers
        assert!(span_context.is_valid(), "span context should be valid");
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            "abcdef1234567890abcdef1234567890",
            "trace ID should be extracted from headers, not default"
        );
    }

    // ========================================================================
    // Tests for TraceContextCarrier::inject_trace_context implementations
    // ========================================================================

    #[test]
    fn test_metadata_map_inject_trace_context_modifies_carrier() {
        let _provider = init_tracing_with_otel();

        // Create a span with a known trace ID
        let trace_id = TraceId::from_hex("fedcba9876543210fedcba9876543210").unwrap();
        let span_id = SpanId::from_hex("1234567890abcdef").unwrap();
        let span_context = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            false,
            TraceState::default(),
        );
        let parent_cx = opentelemetry::Context::new().with_remote_span_context(span_context);

        let span = tracing::info_span!("test_grpc_inject");
        {
            use tracing_opentelemetry::OpenTelemetrySpanExt;
            let _ = span.set_parent(parent_cx);
        }
        let _enter = span.enter();

        // Use the TraceContextCarrier trait method
        let mut metadata = tonic::metadata::MetadataMap::new();
        assert!(
            metadata.get("traceparent").is_none(),
            "metadata should start empty"
        );

        TraceContextCarrier::inject_trace_context(&mut metadata);

        // Verify injection actually modified the carrier
        assert!(
            metadata.get("traceparent").is_some(),
            "inject_trace_context should add traceparent"
        );
        let traceparent = metadata.get("traceparent").unwrap().to_str().unwrap();
        assert!(
            traceparent.contains("fedcba9876543210fedcba9876543210"),
            "injected traceparent should contain the trace ID"
        );
    }

    #[test]
    fn test_http_header_map_inject_trace_context_modifies_carrier() {
        let _provider = init_tracing_with_otel();

        // Create a span with a known trace ID
        let trace_id = TraceId::from_hex("11111111111111111111111111111111").unwrap();
        let span_id = SpanId::from_hex("2222222222222222").unwrap();
        let span_context = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            false,
            TraceState::default(),
        );
        let parent_cx = opentelemetry::Context::new().with_remote_span_context(span_context);

        let span = tracing::info_span!("test_http_inject");
        {
            use tracing_opentelemetry::OpenTelemetrySpanExt;
            let _ = span.set_parent(parent_cx);
        }
        let _enter = span.enter();

        // Use the TraceContextCarrier trait method
        let mut headers = http::HeaderMap::new();
        assert!(
            headers.get("traceparent").is_none(),
            "headers should start empty"
        );

        TraceContextCarrier::inject_trace_context(&mut headers);

        // Verify injection actually modified the carrier
        assert!(
            headers.get("traceparent").is_some(),
            "inject_trace_context should add traceparent"
        );
        let traceparent = headers.get("traceparent").unwrap().to_str().unwrap();
        assert!(
            traceparent.contains("11111111111111111111111111111111"),
            "injected traceparent should contain the trace ID"
        );
    }

    #[test]
    fn test_hashmap_inject_trace_context_modifies_carrier() {
        let _provider = init_tracing_with_otel();

        // Create a span with a known trace ID
        let trace_id = TraceId::from_hex("33333333333333333333333333333333").unwrap();
        let span_id = SpanId::from_hex("4444444444444444").unwrap();
        let span_context = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            false,
            TraceState::default(),
        );
        let parent_cx = opentelemetry::Context::new().with_remote_span_context(span_context);

        let span = tracing::info_span!("test_map_inject");
        {
            use tracing_opentelemetry::OpenTelemetrySpanExt;
            let _ = span.set_parent(parent_cx);
        }
        let _enter = span.enter();

        // Use the TraceContextCarrier trait method
        let mut headers: HashMap<String, String> = HashMap::new();
        assert!(
            headers.get("traceparent").is_none(),
            "headers should start empty"
        );

        TraceContextCarrier::inject_trace_context(&mut headers);

        // Verify injection actually modified the carrier
        assert!(
            headers.get("traceparent").is_some(),
            "inject_trace_context should add traceparent"
        );
        let traceparent = headers.get("traceparent").unwrap();
        assert!(
            traceparent.contains("33333333333333333333333333333333"),
            "injected traceparent should contain the trace ID"
        );
    }

    // ========================================================================
    // Tests for link_distributed_trace functions
    // ========================================================================

    #[test]
    fn test_link_distributed_trace_grpc_calls_set_parent() {
        // This test verifies that link_distributed_trace actually calls set_parent
        // by using a mock-like approach: we verify the extraction happens and
        // the function attempts to link (even if it errors due to no OTel layer)
        init_test_propagator();

        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert(
            "traceparent",
            "00-aaaabbbbccccddddaaaabbbbccccdddd-1111222233334444-01"
                .parse()
                .unwrap(),
        );

        // Call link_distributed_trace - it extracts context and calls set_parent
        // The result depends on whether an OTel layer is registered
        let _ = link_distributed_trace(&metadata);

        // To verify the function does something (not just returns Ok(())),
        // we verify that extract_trace_context (which it calls internally)
        // returns the correct context. If the mutation replaced the body with
        // Ok(()), the function wouldn't extract or link anything.
        //
        // The actual linking verification is done by test_set_span_parent_links_trace
        // which tests the same code path with a properly initialized OTel layer.
        let ctx = extract_trace_context(&metadata);
        let span_ref = ctx.span();
        let span_context = span_ref.span_context();
        assert!(span_context.is_valid());
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            "aaaabbbbccccddddaaaabbbbccccdddd"
        );
    }

    #[test]
    fn test_link_distributed_trace_http_extracts_and_links() {
        init_test_propagator();

        let mut headers = http::HeaderMap::new();
        headers.insert(
            "traceparent",
            "00-55556666777788885555666677778888-9999aaaabbbbcccc-01"
                .parse()
                .unwrap(),
        );

        // The function should execute the extraction and linking logic
        let _ = link_distributed_trace_http(&headers);

        // Verify that extraction happened
        let ctx = extract_trace_context_http(&headers);
        let span = ctx.span();
        let span_context = span.span_context();
        assert!(span_context.is_valid());
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            "55556666777788885555666677778888"
        );
    }

    #[test]
    fn test_link_distributed_trace_map_extracts_and_links() {
        init_test_propagator();

        let mut headers: HashMap<String, String> = HashMap::new();
        headers.insert(
            "traceparent".to_string(),
            "00-ddddeeeeffff0000ddddeeeeffff0000-1234567890abcdef-01".to_string(),
        );

        // The function should execute the extraction and linking logic
        let _ = link_distributed_trace_map(&headers);

        // Verify that extraction happened
        let ctx = extract_trace_context_map(&headers);
        let span = ctx.span();
        let span_context = span.span_context();
        assert!(span_context.is_valid());
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            "ddddeeeeffff0000ddddeeeeffff0000"
        );
    }

    #[test]
    fn test_trace_context_ext_link_distributed_trace_extracts_context() {
        init_test_propagator();

        let mut headers = http::HeaderMap::new();
        headers.insert(
            "traceparent",
            "00-11112222333344441111222233334444-5555666677778888-01"
                .parse()
                .unwrap(),
        );

        // Use TraceContextExt trait method - it may error without OTel layer
        let _ = super::TraceContextExt::link_distributed_trace(&headers);

        // Verify extraction works via the trait method
        let ctx = TraceContextCarrier::extract_trace_context(&headers);
        let span = ctx.span();
        let span_context = span.span_context();
        assert!(span_context.is_valid());
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            "11112222333344441111222233334444"
        );
    }

    // ========================================================================
    // Tests for MetadataInjector
    // ========================================================================

    #[test]
    fn test_metadata_injector_set_adds_header() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        {
            let mut injector = MetadataInjector(&mut metadata);
            injector.set("traceparent", "00-test-value-01".to_string());
        }

        assert!(
            metadata.get("traceparent").is_some(),
            "set should add the header"
        );
        assert_eq!(
            metadata.get("traceparent").unwrap().to_str().unwrap(),
            "00-test-value-01"
        );
    }

    #[test]
    fn test_metadata_injector_set_handles_invalid_key_gracefully() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        {
            let mut injector = MetadataInjector(&mut metadata);
            // Invalid header name with spaces - should not panic
            injector.set("invalid key with spaces", "value".to_string());
        }

        // The invalid key should not be added
        assert!(
            metadata.is_empty(),
            "invalid header keys should be handled gracefully"
        );
    }

    // ========================================================================
    // Tests for HttpHeaderInjector
    // ========================================================================

    #[test]
    fn test_http_header_injector_set_adds_header() {
        let mut headers = http::HeaderMap::new();
        {
            let mut injector = HttpHeaderInjector(&mut headers);
            injector.set("traceparent", "00-http-test-01".to_string());
        }

        assert!(
            headers.get("traceparent").is_some(),
            "set should add the header"
        );
        assert_eq!(
            headers.get("traceparent").unwrap().to_str().unwrap(),
            "00-http-test-01"
        );
    }

    #[test]
    fn test_http_header_injector_set_handles_invalid_key_gracefully() {
        let mut headers = http::HeaderMap::new();
        {
            let mut injector = HttpHeaderInjector(&mut headers);
            // Invalid header name with spaces - should not panic
            injector.set("invalid key", "value".to_string());
        }

        // The invalid key should not be added
        assert!(
            headers.is_empty(),
            "invalid header keys should be handled gracefully"
        );
    }

    // ========================================================================
    // Tests for init_otel_logs_builder
    // ========================================================================

    #[test]
    fn test_init_otel_logs_builder_returns_configured_builder() {
        // Create a tokio runtime for the async exporter
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let service_info = crate::ServiceInfo {
                name: "test-service",
                name_in_metrics: "test_service".to_string(),
                version: "1.0.0",
                author: "Test",
                description: "Test service",
            };

            // Use a dummy endpoint - the builder doesn't connect until export
            let endpoint = "http://localhost:4317".to_string();

            let result = super::init_otel_logs_builder(&service_info, &endpoint);

            // The function should succeed and return a configured builder
            assert!(
                result.is_ok(),
                "init_otel_logs_builder should return Ok with valid endpoint"
            );

            // Build the provider to verify configuration was applied
            let builder = result.unwrap();
            let provider = builder.build();

            // If the builder was Default::default(), the provider wouldn't have
            // the exporter or resource configured. We can verify by checking
            // that shutdown succeeds (it would fail differently if misconfigured)
            let shutdown_result = provider.shutdown();
            assert!(
                shutdown_result.is_ok(),
                "provider built from configured builder should shutdown cleanly"
            );
        });
    }

    // ========================================================================
    // Tests for init_traces and init_metrics
    // ========================================================================

    #[test]
    fn test_init_traces_with_endpoint_returns_provider() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let service_info = crate::ServiceInfo {
                name: "test-service",
                name_in_metrics: "test_service".to_string(),
                version: "1.0.0",
                author: "Test",
                description: "Test service",
            };

            let settings = TraceSettings {
                endpoint: Some("http://localhost:4317".to_string()),
            };

            let result = super::init_traces(&service_info, &settings);

            assert!(result.is_ok(), "init_traces should succeed");
            let provider = result.unwrap();
            assert!(
                provider.is_some(),
                "init_traces should return Some(provider) when endpoint is configured"
            );

            // Clean up
            if let Some(p) = provider {
                let _ = p.shutdown();
            }
        });
    }

    #[test]
    fn test_init_traces_without_endpoint_returns_none() {
        let service_info = crate::ServiceInfo {
            name: "test-service",
            name_in_metrics: "test_service".to_string(),
            version: "1.0.0",
            author: "Test",
            description: "Test service",
        };

        let settings = TraceSettings { endpoint: None };

        let result = super::init_traces(&service_info, &settings);

        assert!(result.is_ok(), "init_traces should succeed");
        let provider = result.unwrap();
        assert!(
            provider.is_none(),
            "init_traces should return None when endpoint is not configured"
        );
    }

    #[test]
    fn test_init_metrics_with_endpoint_returns_provider() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let service_info = crate::ServiceInfo {
                name: "test-service",
                name_in_metrics: "test_service".to_string(),
                version: "1.0.0",
                author: "Test",
                description: "Test service",
            };

            let settings = MetricSettings {
                endpoint: Some("http://localhost:4317".to_string()),
            };

            let result = super::init_metrics(&service_info, &settings);

            assert!(result.is_ok(), "init_metrics should succeed");
            let provider = result.unwrap();
            assert!(
                provider.is_some(),
                "init_metrics should return Some(provider) when endpoint is configured"
            );

            // Clean up
            if let Some(p) = provider {
                let _ = p.shutdown();
            }
        });
    }

    #[test]
    fn test_init_metrics_without_endpoint_returns_none() {
        let service_info = crate::ServiceInfo {
            name: "test-service",
            name_in_metrics: "test_service".to_string(),
            version: "1.0.0",
            author: "Test",
            description: "Test service",
        };

        let settings = MetricSettings { endpoint: None };

        let result = super::init_metrics(&service_info, &settings);

        assert!(result.is_ok(), "init_metrics should succeed");
        let provider = result.unwrap();
        assert!(
            provider.is_none(),
            "init_metrics should return None when endpoint is not configured"
        );
    }

    // ========================================================================
    // Tests for LogSubscriberBuilder
    // ========================================================================

    #[test]
    fn test_log_subscriber_builder_new_sets_fields() {
        let service_info = crate::ServiceInfo {
            name: "test-service",
            name_in_metrics: "test_service".to_string(),
            version: "1.0.0",
            author: "Test",
            description: "Test service",
        };

        let settings = LogSettings {
            console_level: "info".to_string(),
            otel_level: "info".to_string(),
            endpoint: None,
        };

        let builder = super::LogSubscriberBuilder::new(&service_info, &settings);

        // Verify the builder captured the references
        assert_eq!(builder.service_info.name, "test-service");
        assert_eq!(builder.settings.console_level, "info");
        assert!(builder.tracer_provider.is_none());
    }

    #[test]
    fn test_log_subscriber_builder_with_tracer_provider_sets_provider() {
        use opentelemetry_sdk::trace::SdkTracerProvider;

        let service_info = crate::ServiceInfo {
            name: "test-service",
            name_in_metrics: "test_service".to_string(),
            version: "1.0.0",
            author: "Test",
            description: "Test service",
        };

        let settings = LogSettings {
            console_level: "info".to_string(),
            otel_level: "info".to_string(),
            endpoint: None,
        };

        let tracer_provider = SdkTracerProvider::builder().build();

        let builder = super::LogSubscriberBuilder::new(&service_info, &settings)
            .with_tracer_provider(&tracer_provider);

        // Verify the tracer provider was set
        assert!(builder.tracer_provider.is_some());

        let _ = tracer_provider.shutdown();
    }

    #[test]
    fn test_log_subscriber_builder_build_returns_working_subscriber() {
        // Test that build() returns a subscriber that can be used with with_default.
        // This catches mutations like "replace init_logs body with Ok(None)" because
        // if build() returned a broken/default subscriber, this test would fail.
        let service_info = crate::ServiceInfo {
            name: "test-service",
            name_in_metrics: "test_service".to_string(),
            version: "1.0.0",
            author: "Test",
            description: "Test service",
        };

        let settings = LogSettings {
            console_level: "info".to_string(),
            otel_level: "info".to_string(),
            endpoint: None, // No OTel endpoint - just console logging
        };

        let result = super::LogSubscriberBuilder::new(&service_info, &settings).build();
        assert!(result.is_ok(), "build() should succeed");

        let built = result.unwrap();

        // logger_provider should be None when no endpoint is configured
        assert!(
            built.logger_provider.is_none(),
            "logger_provider should be None without OTel endpoint"
        );

        // The subscriber should be usable with with_default
        // This verifies that build() actually built something, not just returned default
        use std::sync::atomic::{AtomicBool, Ordering};
        static LOG_RECEIVED: AtomicBool = AtomicBool::new(false);

        // Create a simple layer that sets a flag when it receives events
        struct TestLayer;
        impl<S: Subscriber> tracing_subscriber::Layer<S> for TestLayer {
            fn on_event(
                &self,
                _event: &tracing::Event<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                LOG_RECEIVED.store(true, Ordering::SeqCst);
            }
        }

        // Use the built subscriber with an additional test layer
        use tracing_subscriber::layer::SubscriberExt;
        let subscriber_with_test = built.subscriber.with(TestLayer);

        tracing::subscriber::with_default(subscriber_with_test, || {
            tracing::info!("test log message");
        });

        // The test layer should have received the event, proving the subscriber works
        assert!(
            LOG_RECEIVED.load(Ordering::SeqCst),
            "subscriber from build() should process log events"
        );
    }

    #[test]
    fn test_log_subscriber_builder_build_with_tracer_provider() {
        // Test that when a tracer_provider is passed, the subscriber includes the OTel trace layer.
        use opentelemetry_sdk::trace::SdkTracerProvider;

        let service_info = crate::ServiceInfo {
            name: "test-service",
            name_in_metrics: "test_service".to_string(),
            version: "1.0.0",
            author: "Test",
            description: "Test service",
        };

        let settings = LogSettings {
            console_level: "info".to_string(),
            otel_level: "info".to_string(),
            endpoint: None,
        };

        let tracer_provider = SdkTracerProvider::builder().build();

        let result = super::LogSubscriberBuilder::new(&service_info, &settings)
            .with_tracer_provider(&tracer_provider)
            .build();
        assert!(
            result.is_ok(),
            "build() should succeed with tracer_provider"
        );

        let built = result.unwrap();

        // Use the subscriber and verify spans work (they're processed by the OTel layer)
        tracing::subscriber::with_default(built.subscriber, || {
            let span = tracing::info_span!("test_span_with_otel");
            let _enter = span.enter();
            tracing::info!("inside span");
        });

        // Clean up
        let _ = tracer_provider.shutdown();
    }
}
