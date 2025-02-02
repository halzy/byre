//! Tracing, metrics, logging related tools.

use doku::Document;
use opentelemetry::trace::TraceError;
use opentelemetry::{global, KeyValue};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{LogExporter, MetricExporter, SpanExporter, WithExportConfig};
use opentelemetry_sdk::logs::{LogError, LoggerProvider};
use opentelemetry_sdk::metrics::{MetricError, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::runtime::TokioCurrentThread;
use opentelemetry_sdk::{trace as sdktrace, Resource};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt as _, Snafu};
use tracing::Subscriber;
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::ServiceInfo;

/// Errors initializing telemetry
#[derive(Debug, Snafu)]
pub enum Error {
    /// Could not initialize the logger
    #[snafu(display("Could not initialize logging: {source}"))]
    InitLogError {
        /// The error from initializing the gRPC connection
        source: LogError,
    },

    /// Could not initialize metrics
    #[snafu(display("Could not initialize metrics: {source}"))]
    InitMetricError {
        /// The error from initializing the gRPC connection
        source: MetricError,
    },

    /// Could not initialize tracing
    #[snafu(display("Could not initialize tracing: {source}"))]
    InitTraceError {
        /// The error from initializing the gRPC connection
        source: TraceError,
    },
}

/// Settings for Metrics
#[derive(Default, Serialize, Deserialize, Document)]
pub struct MetricSettings {
    /// gRPC endpoint to send metrics to. Omit to disable opentelemetry metrics.
    #[doku(example = "http://localhost:4318/v1/metrics")]
    pub endpoint: Option<String>,
}

/// Settings for Logging
#[derive(Default, Serialize, Deserialize, Document)]
pub struct LogSettings {
    /// log level used when filtering console logs. Uses env-logger style syntax. Set to "off" to disable console logging.
    #[doku(example = "debug,yourcrate=trace")]
    pub console_level: String,

    /// log level used when filtering opentelemetry logs. Uses env-logger style syntax.
    #[doku(example = "warn,yourcrate=debug")]
    pub otel_level: String,

    /// gRPC endpoint to send the opentelemetry logs. Omit to disable opentelemetry logs, will not disable console logs.
    #[doku(example = "http://localhost:4317")]
    pub endpoint: Option<String>,
}

/// Settings for opentelemetry traces
#[derive(Default, Serialize, Deserialize, Document)]
pub struct TraceSettings {
    /// gRPC endpoint to send opentelemetry traces to, omit to disable.
    #[doku(example = "http://localhost:4317")]
    pub endpoint: Option<String>,
}

/**
Use TelemetrySettings as a member in your own Settings object.

```rust
use doku::Document;
use serde::Deserialize;

#[derive(Deserialize, Document)]
/// Top level Settings
struct Settings {
    /// Application Settings
    pub application: Application,
    // Telemetry settings.
    pub telemetry: byre::telemetry::TelemetrySettings,
}

#[derive(Deserialize, Document)]
struct Application {
    // .. your app settings here
}
```
*/

/// Settings for tracing, logging, and metrics.
#[derive(Default, Serialize, Deserialize, Document)]
pub struct TelemetrySettings {
    /// Settings for tracing
    pub trace: TraceSettings,
    /// Settings for logging
    pub log: LogSettings,
    /// Settings for metrics
    pub metric: MetricSettings,
}

/// Telemetry initializes tracing, metrics, and logging.
#[derive(Debug)]
pub struct Telemetry {
    meter_provider: Option<SdkMeterProvider>,
    tracer_provider: Option<sdktrace::TracerProvider>,
    logger_provider: Option<LoggerProvider>,
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Some(tracer_provider) = self.tracer_provider.take() {
            match tracer_provider.shutdown() {
                Err(err) => {
                    eprintln!("Error shutting down Telemetry tracer provider: {err}");
                }
                _ => (),
            }
        }
        if let Some(logger_provider) = self.logger_provider.take() {
            match logger_provider.shutdown() {
                Err(err) => {
                    eprintln!("Error shutting down Telemetry logger provider: {err}");
                }
                _ => (),
            }
        }
        match self.meter_provider.take() {
            Some(meter_provider) => match meter_provider.shutdown() {
                Err(err) => {
                    eprintln!("Error shutting down Telemetry meter provider: {err}");
                }
                _ => (),
            },
            _ => (),
        }
    }
}

fn init_traces(
    service_info: &ServiceInfo,
    settings: &TraceSettings,
) -> Result<Option<sdktrace::TracerProvider>, TraceError> {
    match &settings.endpoint {
        Some(endpoint) => {
            let exporter = SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .build()?;

            let resource = Resource::new(vec![KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                service_info.name_in_metrics.clone(),
            )]);

            Ok(Some(
                sdktrace::TracerProvider::builder()
                    .with_resource(resource)
                    .with_batch_exporter(exporter, TokioCurrentThread)
                    .build(),
            ))
        }
        None => Ok(None),
    }
}

fn init_metrics(
    service_info: &ServiceInfo,
    setting: &MetricSettings,
) -> Result<Option<opentelemetry_sdk::metrics::SdkMeterProvider>, MetricError> {
    match &setting.endpoint {
        Some(endpoint) => {
            let exporter = MetricExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .build()?;
            let reader = PeriodicReader::builder(exporter, TokioCurrentThread).build();

            let resource = Resource::new(vec![KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                service_info.name_in_metrics.clone(),
            )]);

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
        Option<opentelemetry_sdk::logs::LoggerProvider>,
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
            let builder = LoggerProvider::builder();

            let exporter = LogExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .build()
                .with_context(|_| InitLogSnafu {})?;

            let resource = Resource::new(vec![KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                service_info.name_in_metrics.clone(),
            )]);

            let builder = builder
                .with_resource(resource)
                .with_batch_exporter(exporter, TokioCurrentThread);

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

fn init_logs(
    service_info: &ServiceInfo,
    settings: &LogSettings,
) -> Result<Option<opentelemetry_sdk::logs::LoggerProvider>, Error> {
    let (logger_provider, otel_layer) = init_otel_logs(service_info, settings)?;

    // Create a new tracing::Fmt layer to print the logs to stdout. It has a
    // default filter of `info` level and above, and `debug` and above for logs
    // from OpenTelemetry crates. The filter levels can be customized as needed.
    let filter_fmt = EnvFilter::new(&settings.console_level);
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_thread_names(true)
        .with_filter(filter_fmt);

    // Initialize the tracing subscriber with the OpenTelemetry layer and the
    // Fmt layer.
    tracing_subscriber::registry()
        .with(otel_layer)
        .with(fmt_layer)
        .init();

    Ok(logger_provider)
}

/// Starts the telemetry backend
///
/// Uses `service_info` to configure the SERVICE_NAME of the telemetry client.
/// If you would like to disable sending any of the metrics, tracing, or logging to the OpenTelemetry set the respective endpoint to `None`.
pub fn init(service_info: &ServiceInfo, settings: &TelemetrySettings) -> Result<Telemetry, Error> {
    let logger_provider = init_logs(service_info, &settings.log)?;

    let tracer_provider =
        init_traces(service_info, &settings.trace).with_context(|_| InitTraceSnafu {})?;
    if let Some(tracer_provider) = &tracer_provider {
        global::set_tracer_provider(tracer_provider.clone());
    }

    let meter_provider =
        init_metrics(service_info, &settings.metric).with_context(|_| InitMetricSnafu {})?;
    if let Some(meter_provider) = &meter_provider {
        global::set_meter_provider(meter_provider.clone());
    }

    Ok(Telemetry {
        meter_provider,
        tracer_provider,
        logger_provider,
    })
}
