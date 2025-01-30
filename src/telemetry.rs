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
use tracing_subscriber::prelude::*;
use tracing_subscriber::EnvFilter;

use crate::ServiceInfo;

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Could not initialize logging: {source}"))]
    InitLogError { source: LogError },

    #[snafu(display("Could not initialize metrics: {source}"))]
    InitMetricError { source: MetricError },

    #[snafu(display("Could not initialize tracing: {source}"))]
    InitTraceError { source: TraceError },
}

#[derive(Default, Serialize, Deserialize, Document)]
pub struct MetricSettings {
    #[doku(example = "http://localhost:4318/v1/metrics")]
    pub endpoint: Option<String>,
}

#[derive(Default, Serialize, Deserialize, Document)]
pub struct LogSettings {
    #[doku(example = "debug,yourcrate=trace")]
    pub console_level: String,
    #[doku(example = "warn,yourcrate=debug")]
    pub otel_level: String,
    #[doku(example = "http://localhost:4317")]
    pub endpoint: Option<String>,
}
#[derive(Default, Serialize, Deserialize, Document)]
pub struct TraceSettings {
    #[doku(example = "http://localhost:4317")]
    pub endpoint: Option<String>,
}
#[derive(Default, Serialize, Deserialize, Document)]
pub struct TelemetrySettings {
    pub trace: TraceSettings,
    pub log: LogSettings,
    pub metric: MetricSettings,
}

pub struct Telemetry {
    meter_provider: Option<SdkMeterProvider>,
    tracer_provider: Option<sdktrace::TracerProvider>,
    logger_provider: LoggerProvider,
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Some(tracer_provider) = self.tracer_provider.take() {
            if let Err(err) = tracer_provider.shutdown() {
                eprintln!("Error shutting down Telemetry tracer provider: {err}");
            }
        }
        if let Some(meter_provider) = self.meter_provider.take() {
            if let Err(err) = meter_provider.shutdown() {
                eprintln!("Error shutting down Telemetry meter provider: {err}");
            }
        }
        if let Err(err) = self.logger_provider.shutdown() {
            eprintln!("Error shutting down Telemetry logger provider: {err}");
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

fn init_logs(
    service_info: &ServiceInfo,
    settings: &LogSettings,
) -> Result<opentelemetry_sdk::logs::LoggerProvider, LogError> {
    let builder = LoggerProvider::builder();

    let builder = match &settings.endpoint {
        Some(endpoint) => {
            let exporter = LogExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint)
                .build()?;

            let resource = Resource::new(vec![KeyValue::new(
                opentelemetry_semantic_conventions::resource::SERVICE_NAME,
                service_info.name_in_metrics.clone(),
            )]);

            builder
                .with_resource(resource)
                .with_batch_exporter(exporter, TokioCurrentThread)
        }
        None => builder,
    };

    let logger_provider = builder.build();

    let otel_layer = settings.endpoint.as_ref().map(|_| {
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

        otel_layer
    });

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

pub fn init(service_info: &ServiceInfo, settings: &TelemetrySettings) -> Result<Telemetry, Error> {
    let logger_provider =
        init_logs(service_info, &settings.log).with_context(|_| InitLogSnafu {})?;

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

// #[cfg(test)]
// mod tests {
//     use crate::init_tracing;

//     #[tokio::test(flavor = "multi_thread")]
//     async fn test_connection() {
//         init_tracing().unwrap();
//         eprintln!("END");
//     }
// }
