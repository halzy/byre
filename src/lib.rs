/*!
Byre is opinionated. It is a shallow wrapper around collection of crates.

Use byre as a means to avoid bikeshedding and bootstrap programs more quickly.

It provides:
 * command line parsing (via clap)
 * config file generation and loading (via Doku & Figment)
 * environment variable overrides for configs (via Doku)
 * logging & tracing & metrics (via tracing & opentelemetry)

### Tutorial

1. Start by adding the *latest* version of byre, doku, serde, and tokio to your Cargo.toml dependencies. (**NOTE:** check for *latest* versions)

```toml
[dependencies]
byre = "0.2"
doku = "0.21"
serde = "1"
tokio = "1"
```

2. Create a Settings struct that will be used to hold your application settings and the telemetry settings.
```rust
use doku::Document;
use serde::Deserialize;

/// Settings container
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
    #[doku(example = "/var/db/app_dbs")]
    pub application_db_dir: std::path::PathBuf,
}
```

3. Have `byre` handle the CLI argument parsing, config, and env overrides:

```rust,no_run
# use doku::Document;
# use serde::Deserialize;
# #[derive(Document, Deserialize)]
# pub struct Settings {}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments. Add additional command line option that allows checking
    // the config without running the server.
    let service_info = byre::service_info!();
    let cli = byre::cli::Cli::<Settings>::new(&service_info, "MYAPP_");

    // ...

    Ok(())
}
```

4. Initialize the `byre` telemetry

```rust,no_run
# use doku::Document;
# use serde::Deserialize;
# #[derive(Document, Deserialize)]
# pub struct Settings {
#     /// Telemetry settings.
#     pub telemetry: byre::telemetry::TelemetrySettings,
# }
# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
#     // Parse command line arguments. Add additional command line option that allows checking
#     // the config without running the server.
#     let service_info = byre::service_info!();
    let cli = byre::cli::Cli::<Settings>::new(&service_info, "MYAPP_");
    let _telemetry = byre::telemetry::init(&service_info, &cli.config.telemetry)?;
#
#     // ...
#
#     Ok(())
# }
```

### Override config value via environment values

Environment variables can be used to override a setting from a config file.

Overrides can be nested. For example, to override the `application.listen_port` one would set an environment value like so, replacing the dot (`.`) with double underscores (`__`):
```sh
MYAPP_APPLICATION__LISTEN_PORT=8080 ./test_app --config ./test_app.toml
```

### Additional Command line arguments

Create a struct that represents the Arguments you want to check for. `clap` will need to be added to your Cargo.toml dependencies since `clap::Parser` is used.


```rust,no_run
# use doku::Document;
# #[derive(Document, Deserialize)]
# pub struct Settings {
#     /// Telemetry settings.
#     pub telemetry: byre::telemetry::TelemetrySettings,
# }
use clap::Parser;
use serde::{Deserialize, Serialize};

#[derive(Parser, Deserialize, Serialize)]
/// A NEW description, not using the one from Cargo.toml!
pub struct Arguments {
    /// world peace, careful, has consequences
    #[arg(short, long)]
    pub enable_world_peace: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments. Add additional command line option that allows checking
    // the config without running the server.
    let service_info = byre::service_info!();
    let cli = byre::cli::Cli::<Settings, Arguments>::new(&service_info, "MYAPP_");
    let _telemetry = byre::telemetry::init(&service_info, &cli.config.telemetry)?;

    // Check if world peace has been enabled
    if cli.args.enable_world_peace {
        // yay!
    }

    Ok(())
}
```

Notice that the description is overridden and there is an option to enable world peace.
```sh
❯ test_app --help
A NEW description, not using the one from Cargo.toml!

Usage: test_app [OPTIONS]

Options:
  -e, --enable-world-peace   world peace, careful, has consequences
  -c, --config <config>      Specifies the toml config file to run the service with
  -g, --generate <generate>  Generates a new default toml config file for the service
  -h, --help                 Print help
  -V, --version              Print version
```

### Examples

There is a `full` example in the [source tree](https://github.com/halzy/byre/tree/main/examples).

*/
#![deny(
    future_incompatible,
    deprecated_safe,
    rust_2018_compatibility,
    rust_2018_idioms,
    rust_2021_compatibility,
    rust_2024_compatibility
)]
// Document ALL THE THINGS!
#![deny(missing_docs)]

pub mod cli;
pub mod config;
pub mod telemetry;

/// Cli initialization related errors
#[derive(Debug, snafu::Snafu)]
pub enum Error {
    /// Figment could not extract a config from the file with env overrides
    #[snafu(display("Could not load application configuration: {source}"))]
    ConfigLoadError {
        /// The source figment error
        source: figment::Error,
    },

    /// Writing to the config file was not possible
    #[snafu(display("Could not write to the config file at {path:?}: {source}"))]
    ConfigFileWriteError {
        /// path where the config file was trying to be written to
        path: std::path::PathBuf,
        /// the IO error that occurred
        source: std::io::Error,
    },
}

/// Global memory allocator backed by [jemalloc].
///
/// This static variable is exposed solely for the documentation purposes and don't need to be used
/// directly. If **jemalloc** feature is enabled then the service will use jemalloc for all the
/// memory allocations implicitly.
///
/// If no Foundations API is being used by your project, you will need to explicitly link foundations crate
/// to your project by adding `extern crate foundations;` to your `main.rs` or `lib.rs`, for jemalloc to
/// be embedded in your binary.
///
/// [jemalloc]: https://github.com/jemalloc/jemalloc
#[cfg(feature = "jemalloc")]
#[global_allocator]
pub static JEMALLOC_MEMORY_ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// Service information collected from the build.
#[derive(Clone, Debug, Default)]
pub struct ServiceInfo {
    /// The name of the service.
    pub name: &'static str,

    /// The service identifier as used in metrics.
    ///
    /// Usually the same as [`ServiceInfo::name`], with hyphens (`-`) replaced by underscores `_`.
    pub name_in_metrics: String,

    /// The version of the service.
    pub version: &'static str,

    /// Service author.
    pub author: &'static str,
    /// The description of the service.
    pub description: &'static str,
}

// # #[tokio::main] async fn main() -> anyhow::Result<()> {
//
/**
Creates [`ServiceInfo`] from the information in `Cargo.toml` manifest of the service.

ServiceInfo is used to populate the client name for Telemetry and the CLI help.
```rust,no_run
use doku::Document;
use serde::Deserialize;

#[derive(Deserialize, Document)]
/// Data Archive Settings
pub(crate) struct Settings {
    /// Server Settings
    pub(crate) application: Application,
    // Telemetry settings.
    pub(crate) telemetry: byre::telemetry::TelemetrySettings,
}

#[derive(Deserialize, Document)]
pub(crate) struct Application {
    /// DNS resolve this host and bind to its IP, ie: localhost
    #[doku(example = "localhost")]
    pub(crate) listen_host: String,

    /// port to bind to
    #[doku(example = "8080")]
    pub(crate) listen_port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service_info = byre::service_info!();
    let cli = byre::cli::Cli::<Settings>::new(&service_info, "MYAPP_");
    let _telemetry = byre::telemetry::init(&service_info, &cli.config.telemetry)?;

    // ...

    Ok(())
}
```

[`ServiceInfo::name_in_metrics`] is the same as the package name, with hyphens (`-`) replaced
by underscores (`_`).
*/
#[macro_export]
macro_rules! service_info {
    () => {
        $crate::ServiceInfo {
            name: env!("CARGO_PKG_NAME"),
            name_in_metrics: env!("CARGO_PKG_NAME").replace("-", "_"),
            version: env!("CARGO_PKG_VERSION"),
            author: env!("CARGO_PKG_AUTHORS"),
            description: env!("CARGO_PKG_DESCRIPTION"),
        }
    };
}
