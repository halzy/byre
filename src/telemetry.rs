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
use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{global, KeyValue};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{
    ExporterBuildError, LogExporter, MetricExporter, SpanExporter, WithExportConfig,
};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
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
pub fn init(
    service_info: &ServiceInfo,
    settings: &TelemetrySettings,
) -> Result<TelemetryProviders, Error> {
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
