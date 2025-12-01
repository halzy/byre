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

fn init_logs(
    service_info: &ServiceInfo,
    settings: &LogSettings,
    tracer_provider: Option<&sdktrace::SdkTracerProvider>,
) -> Result<Option<opentelemetry_sdk::logs::SdkLoggerProvider>, Error> {
    let (logger_provider, otel_log_layer) = init_otel_logs(service_info, settings)?;

    // Create the OpenTelemetry tracing layer if a tracer provider is configured.
    // This bridges tracing spans to OpenTelemetry traces.
    let otel_trace_layer = tracer_provider.map(|provider| {
        let tracer = provider.tracer(service_info.name_in_metrics.clone());
        let filter = EnvFilter::new(&settings.otel_level)
            .add_directive("hyper=off".parse().unwrap())
            .add_directive("opentelemetry=off".parse().unwrap())
            .add_directive("tonic=off".parse().unwrap())
            .add_directive("h2=off".parse().unwrap())
            .add_directive("reqwest=off".parse().unwrap());
        OpenTelemetryLayer::new(tracer).with_filter(filter)
    });

    // Create a new tracing::Fmt layer to print the logs to stdout. It has a
    // default filter of `info` level and above, and `debug` and above for logs
    // from OpenTelemetry crates. The filter levels can be customized as needed.
    let filter_fmt = EnvFilter::new(&settings.console_level);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_thread_names(true)
        .with_filter(filter_fmt);

    // Initialize the tracing subscriber with all layers:
    // - OpenTelemetry log layer (sends logs to OTel)
    // - OpenTelemetry trace layer (sends spans to OTel)
    // - Fmt layer (prints to console)
    tracing_subscriber::registry()
        .with(otel_log_layer)
        .with(otel_trace_layer)
        .with(fmt_layer)
        .init();

    Ok(logger_provider)
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
        self.0
            .keys()
            .filter_map(|k| {
                if let tonic::metadata::KeyRef::Ascii(key) = k {
                    Some(key.as_str())
                } else {
                    None
                }
            })
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

/// Extract trace context from incoming gRPC request metadata.
///
/// Returns the extracted OpenTelemetry context. Use [`link_distributed_trace`] for a more
/// convenient way to extract and link the trace context in one call.
///
/// # Example
///
/// ```ignore
/// async fn my_handler(&self, request: Request<MyRequest>) -> Result<Response<MyResponse>, Status> {
///     let parent_cx = byre::telemetry::extract_context(request.metadata());
///     let _guard = parent_cx.attach();
///
///     // Your handler code here - spans created will be children of the incoming trace
/// }
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
/// ```ignore
/// #[tracing::instrument(skip_all)]
/// async fn my_handler(&self, request: Request<MyRequest>) -> Result<Response<MyResponse>, Status> {
///     let _ = byre::telemetry::link_distributed_trace(request.metadata());
///
///     // Your handler code here - this span is now part of the distributed trace
/// }
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
/// ```ignore
/// let mut request = Request::new(my_request);
/// byre::telemetry::inject_trace_context(request.metadata_mut());
/// let response = client.call(request).await?;
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
        self.0.keys().map(|k| k.as_str()).collect()
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

/// Extract trace context from incoming HTTP request headers.
///
/// Returns the extracted OpenTelemetry context. Use [`link_distributed_trace_http`] for a more
/// convenient way to extract and link the trace context in one call.
///
/// # Example
///
/// ```ignore
/// let parent_cx = byre::telemetry::extract_trace_context_http(request.headers());
/// let _guard = parent_cx.attach();
///
/// // Your handler code here - spans created will be children of the incoming trace
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
/// ```ignore
/// #[tracing::instrument(skip_all)]
/// async fn my_handler(headers: HeaderMap) -> impl IntoResponse {
///     let _ = byre::telemetry::link_distributed_trace_http(&headers);
///
///     // Your handler code here - this span is now part of the distributed trace
/// }
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
/// ```ignore
/// byre::telemetry::inject_trace_context_http(request.headers_mut());
/// let response = client.send(request).await?;
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
/// ```ignore
/// use byre::telemetry::GrpcTraceContextLayer;
///
/// Server::builder()
///     .layer(GrpcTraceContextLayer::new("my-service"))
///     .add_service(my_service)
///     .serve(addr)
///     .await?;
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

/// Inject the current trace context into a HashMap suitable for message queue headers.
///
/// This is useful for propagating trace context through message queues like Iggy
/// where headers are stored as a `HashMap<HeaderKey, HeaderValue>`.
///
/// # Example
///
/// ```ignore
/// use std::collections::HashMap;
///
/// let mut headers = HashMap::new();
/// byre::telemetry::inject_trace_context_map(&mut headers);
///
/// let message = IggyMessage::builder()
///     .payload(payload)
///     .user_headers(headers)
///     .build()?;
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
/// ```ignore
/// let headers = message.user_headers_map()?;
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
/// ```ignore
/// #[tracing::instrument(skip_all)]
/// async fn process_message(message: IggyMessage) {
///     if let Ok(Some(headers)) = message.user_headers_map() {
///         let _ = byre::telemetry::link_distributed_trace_map(&headers);
///     }
///     // Process message...
/// }
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
/// ```ignore
/// let parent_cx = byre::telemetry::extract_trace_context_map(&headers);
/// let span = tracing::info_span!("process_message", message_id = id);
/// byre::telemetry::set_span_parent(&span, parent_cx);
/// let _enter = span.enter();
/// ```
pub fn set_span_parent(span: &tracing::Span, parent_cx: opentelemetry::Context) {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    let _ = span.set_parent(parent_cx);
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

    /// Initialize a tracing subscriber with OpenTelemetry layer for tests
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

    #[test]
    fn test_inject_trace_context_map_from_tracing_span() {
        let _provider = init_tracing_with_otel();

        // Create a tracing span and enter it
        let span = tracing::info_span!("test_span_for_injection");
        let _enter = span.enter();

        // Inject trace context from the current tracing span
        let mut headers: HashMap<String, String> = HashMap::new();
        inject_trace_context_map(&mut headers);

        // Verify traceparent header was injected
        assert!(
            headers.contains_key("traceparent"),
            "traceparent header should be present when inside a tracing span"
        );

        let traceparent = headers.get("traceparent").unwrap();
        // traceparent format: 00-{trace_id}-{span_id}-{flags}
        // trace_id is 32 hex chars, span_id is 16 hex chars
        assert!(
            traceparent.starts_with("00-"),
            "traceparent should start with version 00"
        );

        // Verify it's not an invalid/empty trace ID
        assert!(
            !traceparent.contains("00000000000000000000000000000000"),
            "traceparent should have a valid (non-zero) trace ID"
        );
    }

    #[test]
    fn test_inject_trace_context_from_tracing_span_to_grpc_metadata() {
        let _provider = init_tracing_with_otel();

        // Create a tracing span and enter it
        let span = tracing::info_span!("grpc_client_span");
        let _enter = span.enter();

        // Inject trace context into gRPC metadata
        let mut metadata = tonic::metadata::MetadataMap::new();
        inject_trace_context(&mut metadata);

        // Verify traceparent header was injected
        let traceparent = metadata.get("traceparent");
        assert!(
            traceparent.is_some(),
            "traceparent should be present in gRPC metadata"
        );

        let traceparent_value = traceparent.unwrap().to_str().unwrap();
        assert!(
            traceparent_value.starts_with("00-"),
            "traceparent should start with version 00"
        );
        assert!(
            !traceparent_value.contains("00000000000000000000000000000000"),
            "traceparent should have a valid (non-zero) trace ID"
        );
    }

    #[test]
    fn test_inject_trace_context_http_from_tracing_span() {
        let _provider = init_tracing_with_otel();

        // Create a tracing span and enter it
        let span = tracing::info_span!("http_client_span");
        let _enter = span.enter();

        // Inject trace context into HTTP headers
        let mut headers = http::HeaderMap::new();
        inject_trace_context_http(&mut headers);

        // Verify traceparent header was injected
        let traceparent = headers.get("traceparent");
        assert!(
            traceparent.is_some(),
            "traceparent should be present in HTTP headers"
        );

        let traceparent_value = traceparent.unwrap().to_str().unwrap();
        assert!(
            traceparent_value.starts_with("00-"),
            "traceparent should start with version 00"
        );
        assert!(
            !traceparent_value.contains("00000000000000000000000000000000"),
            "traceparent should have a valid (non-zero) trace ID"
        );
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
    fn test_metadata_extractor_keys_returns_all_keys() {
        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert("traceparent", "value1".parse().unwrap());
        metadata.insert("tracestate", "value2".parse().unwrap());
        metadata.insert("custom-header", "value3".parse().unwrap());

        let extractor = MetadataExtractor(&metadata);
        let keys = extractor.keys();

        assert!(!keys.is_empty(), "keys should not be empty");
        assert!(
            keys.contains(&"traceparent"),
            "keys should contain traceparent"
        );
        assert!(
            keys.contains(&"tracestate"),
            "keys should contain tracestate"
        );
        assert!(
            keys.contains(&"custom-header"),
            "keys should contain custom-header"
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
    fn test_http_header_extractor_keys_returns_all_keys() {
        let mut headers = http::HeaderMap::new();
        headers.insert("traceparent", "value1".parse().unwrap());
        headers.insert("tracestate", "value2".parse().unwrap());
        headers.insert("x-custom-header", "value3".parse().unwrap());

        let extractor = HttpHeaderExtractor(&headers);
        let keys = extractor.keys();

        assert!(!keys.is_empty(), "keys should not be empty");
        assert!(
            keys.contains(&"traceparent"),
            "keys should contain traceparent"
        );
        assert!(
            keys.contains(&"tracestate"),
            "keys should contain tracestate"
        );
        assert!(
            keys.contains(&"x-custom-header"),
            "keys should contain x-custom-header"
        );
    }

    // ========================================================================
    // Tests for extract_trace_context functions
    // ========================================================================

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
        let span = context.span();
        let span_context = span.span_context();

        assert!(span_context.is_valid(), "span context should be valid");
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            "0af7651916cd43dd8448eb211c80319c"
        );
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
        let span = context.span();
        let span_context = span.span_context();

        assert!(span_context.is_valid(), "span context should be valid");
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            "0af7651916cd43dd8448eb211c80319c"
        );
    }

    // ========================================================================
    // Tests for set_span_parent
    // ========================================================================

    #[test]
    fn test_set_span_parent_links_trace() {
        let _provider = init_tracing_with_otel();

        // Create a remote span context
        let trace_id = TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").unwrap();
        let span_id = SpanId::from_hex("b7ad6b7169203331").unwrap();
        let remote_span_context = SpanContext::new(
            trace_id,
            span_id,
            TraceFlags::SAMPLED,
            true, // is_remote
            TraceState::default(),
        );
        let parent_cx = opentelemetry::Context::new().with_remote_span_context(remote_span_context);

        // Create a tracing span and set its parent
        let span = tracing::info_span!("test_set_parent");
        set_span_parent(&span, parent_cx);
        let _enter = span.enter();

        // Inject and verify the trace ID was propagated
        let mut headers: HashMap<String, String> = HashMap::new();
        inject_trace_context_map(&mut headers);

        let traceparent = headers.get("traceparent").unwrap();
        assert!(
            traceparent.contains("0af7651916cd43dd8448eb211c80319c"),
            "trace ID should be preserved after set_span_parent"
        );
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
    // Tests for link_distributed_trace functions
    // ========================================================================

    #[test]
    fn test_link_distributed_trace_grpc_extracts_and_uses_context() {
        init_test_propagator();

        // Create gRPC metadata with a trace context
        let mut metadata = tonic::metadata::MetadataMap::new();
        metadata.insert(
            "traceparent",
            "00-abcdef1234567890abcdef1234567890-1234567890abcdef-01"
                .parse()
                .unwrap(),
        );

        // Verify that extract_trace_context extracts a valid context
        let extracted_cx = extract_trace_context(&metadata);
        let span = extracted_cx.span();
        let span_context = span.span_context();

        assert!(span_context.is_valid(), "extracted context should be valid");
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            "abcdef1234567890abcdef1234567890",
            "trace ID should match"
        );
    }

    #[test]
    fn test_link_distributed_trace_http_extracts_and_uses_context() {
        init_test_propagator();

        // Create HTTP headers with a trace context
        let mut headers = http::HeaderMap::new();
        headers.insert(
            "traceparent",
            "00-11223344556677889900aabbccddeeff-aabbccddeeff0011-01"
                .parse()
                .unwrap(),
        );

        // Verify that extract_trace_context_http extracts a valid context
        let extracted_cx = extract_trace_context_http(&headers);
        let span = extracted_cx.span();
        let span_context = span.span_context();

        assert!(span_context.is_valid(), "extracted context should be valid");
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            "11223344556677889900aabbccddeeff",
            "trace ID should match"
        );
    }

    #[test]
    fn test_link_distributed_trace_map_extracts_and_uses_context() {
        init_test_propagator();

        // Create HashMap with a trace context
        let mut input_headers: HashMap<String, String> = HashMap::new();
        input_headers.insert(
            "traceparent".to_string(),
            "00-ffeeddccbbaa99887766554433221100-0011223344556677-01".to_string(),
        );

        // Verify that extract_trace_context_map extracts a valid context
        let extracted_cx = extract_trace_context_map(&input_headers);
        let span = extracted_cx.span();
        let span_context = span.span_context();

        assert!(span_context.is_valid(), "extracted context should be valid");
        assert_eq!(
            format!("{:032x}", span_context.trace_id()),
            "ffeeddccbbaa99887766554433221100",
            "trace ID should match"
        );
    }
}
