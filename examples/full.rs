use std::path::PathBuf;

use clap::Parser;
use doku::Document;
use serde::{Deserialize, Serialize};

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
/// A NEW description, not using the one from Cargo.toml!
pub struct Arguments {
    /// world peace, careful, has consequences
    #[arg(short, long)]
    pub enable_world_peace: bool,

    /// This value will be overridden by
    #[arg(short, long)]
    pub override_me: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments. Add additional command line option that allows checking
    // the config without running the server.
    let service_info = byre::service_info!();
    let cli = byre::cli::Cli::<Settings, Arguments>::new(&service_info, "MYAPP_");

    let _telemetry = byre::telemetry::init(&service_info, &cli.config.telemetry)?;

    // Find the SocketAddr that we should bind to
    let _listen_port = cli.config.application.listen_port;
    let _listen_hostname = cli.config.application.listen_host;

    // Check if world peace has been enabled
    if cli.args.enable_world_peace {
        // ...
    }

    Ok(())
}
