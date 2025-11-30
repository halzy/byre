use std::path::PathBuf;
use std::time::Duration;

use clap::Parser;
use doku::Document;
use opentelemetry::metrics::{Counter, Histogram, UpDownCounter};
use opentelemetry::{global, KeyValue};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info, info_span, instrument, warn, Instrument};

/// Top Level Settings
#[derive(Document, Deserialize)]
pub struct Settings {
    /// App Settings
    pub application: Application,

    /// Telemetry settings.
    pub telemetry: byre::telemetry::TelemetrySettings,
}

#[derive(Document, Deserialize)]
pub struct Application {
    /// Port to listen on
    #[doku(example = "8080")]
    pub listen_port: u16,

    /// Hostname to listen to
    #[doku(example = "localhost")]
    pub listen_host: String,

    /// Directory where the application databases are located
    #[doku(example = "/var/db/my_databases")]
    pub application_db_dir: PathBuf,
}

#[derive(Parser, Deserialize, Serialize)]
/// Example application demonstrating byre telemetry integration
pub struct Arguments {
    /// Number of iterations to run (0 = run forever)
    #[arg(short, long, default_value = "10")]
    pub iterations: u32,

    /// Delay between iterations in milliseconds
    #[arg(short, long, default_value = "1000")]
    pub delay_ms: u64,

    /// Simulate errors every N iterations (0 = no errors)
    #[arg(short, long, default_value = "5")]
    pub error_every: u32,
}

/// Metrics for the example application
struct AppMetrics {
    request_counter: Counter<u64>,
    active_operations: UpDownCounter<i64>,
    operation_duration: Histogram<f64>,
}

impl AppMetrics {
    fn new() -> Self {
        let meter = global::meter("example_app");

        Self {
            request_counter: meter
                .u64_counter("example.requests")
                .with_description("Total number of requests processed")
                .with_unit("requests")
                .build(),
            active_operations: meter
                .i64_up_down_counter("example.active_operations")
                .with_description("Number of currently active operations")
                .build(),
            operation_duration: meter
                .f64_histogram("example.operation.duration")
                .with_description("Duration of operations in seconds")
                .with_unit("s")
                .build(),
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service_info = byre::service_info!();
    let cli = byre::cli::Cli::<Settings, Arguments>::new(&service_info, "MYAPP_");

    let _telemetry = byre::telemetry::init(&service_info, &cli.config.telemetry)?;

    info!(
        service = %service_info.name,
        version = %service_info.version,
        "Starting example application"
    );

    let metrics = AppMetrics::new();

    // Run the main loop
    run_telemetry_demo(&cli.args, &metrics).await;

    info!("Example application completed");
    Ok(())
}

async fn run_telemetry_demo(args: &Arguments, metrics: &AppMetrics) {
    let iterations = if args.iterations == 0 {
        u32::MAX
    } else {
        args.iterations
    };

    for i in 1..=iterations {
        let span = info_span!("iteration", number = i);
        async {
            info!(iteration = i, "Starting iteration");

            // Track active operations
            metrics.active_operations.add(1, &[]);

            let start = std::time::Instant::now();

            // Simulate different types of operations
            let should_error = args.error_every > 0 && i % args.error_every == 0;

            if should_error {
                simulate_failed_operation(i).await;
                metrics.request_counter.add(
                    1,
                    &[
                        KeyValue::new("status", "error"),
                        KeyValue::new("operation", "simulated_failure"),
                    ],
                );
            } else {
                simulate_successful_operation(i).await;
                metrics.request_counter.add(
                    1,
                    &[
                        KeyValue::new("status", "success"),
                        KeyValue::new("operation", "simulated_work"),
                    ],
                );
            }

            let duration = start.elapsed().as_secs_f64();
            metrics.operation_duration.record(
                duration,
                &[KeyValue::new(
                    "iteration_type",
                    if should_error { "error" } else { "success" },
                )],
            );

            metrics.active_operations.add(-1, &[]);

            debug!(
                iteration = i,
                duration_ms = duration * 1000.0,
                "Iteration completed"
            );
        }
        .instrument(span)
        .await;

        if i < iterations {
            tokio::time::sleep(Duration::from_millis(args.delay_ms)).await;
        }
    }
}

#[instrument(level = "info")]
async fn simulate_successful_operation(iteration: u32) {
    info!("Performing database lookup");
    database_query("users", iteration).await;

    info!("Processing data");
    process_data(iteration).await;

    info!("Sending notification");
    send_notification(iteration).await;
}

#[instrument(level = "warn")]
async fn simulate_failed_operation(iteration: u32) {
    warn!(iteration, "Simulating a failed operation");

    database_query("orders", iteration).await;

    error!(
        iteration,
        error_code = "E_SIMULATED",
        "Simulated error occurred during processing"
    );
}

#[instrument(level = "debug", fields(table = %table))]
async fn database_query(table: &str, iteration: u32) {
    // Simulate variable query times
    let delay = 10 + (iteration % 50);
    tokio::time::sleep(Duration::from_millis(delay as u64)).await;
    debug!(table, rows_returned = iteration % 100, "Query completed");
}

#[instrument(level = "debug")]
async fn process_data(iteration: u32) {
    let delay = 20 + (iteration % 30);
    tokio::time::sleep(Duration::from_millis(delay as u64)).await;
    debug!(
        records_processed = iteration * 10,
        "Data processing completed"
    );
}

#[instrument(level = "debug")]
async fn send_notification(iteration: u32) {
    tokio::time::sleep(Duration::from_millis(5)).await;
    debug!(notification_id = iteration, "Notification sent");
}
