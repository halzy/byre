Byre
====
Byre is a toolbox for bootstrapping services quickly. Providing tracing, logging, configuration file generation and loading, and command line argument parsing.

Byre leans on tracing, opentelemetry, clap, doku, tokio and others.

Byre is a rough fork of [Cloudflare Foundations](https://docs.rs/foundations) with some bits copied directly and others used as inspiration. Byre differs from Foundations in the ecosystems that it draws from.

[![Build status](https://img.shields.io/docsrs/byre)](https://docs.rs/crate/byre/latest/builds)
[![Crates.io](https://img.shields.io/crates/v/byre.svg)](https://crates.io/crates/byre)
[![Docs.rs](https://img.shields.io/docsrs/byre)](https://docs.rs/byre)

### Quick Start

Add byre and tokio to your `Cargo.toml`:

```toml
[dependencies]
byre = "0.3"
doku = "0.21"
serde = "1"
tokio = "1"
clap = { version = "4", features = ["derive"] }  # Only if defining custom CLI Arguments
```

### Application Config Generation

Byre is opinionated about application configuration. Configuration generation MUST be available by running the application. The `clap` crate is used for the cli argument parsing and help display:

```sh
❯ ./data_archive --help
database archive service

Usage: data_archive [OPTIONS]

Options:
  -c, --config <config>      Specifies the toml config file to run the service with
  -g, --generate <generate>  Generates a new default toml config file for the service
  -h, --help                 Print help
  -V, --version              Print version
```

Config generation uses [`Doku`](https://docs.rs/doku/latest/doku/). The following `Settings` struct will produce the toml below.

```rust
use std::path::PathBuf;

use doku::Document;
use serde::Deserialize;

/// Data Archive Settings
#[derive(Document, Deserialize)]
pub struct Settings {
    /// App Settings
    pub application: Application,

    /// Telemetry settings.
    pub telemetry: byre::telemetry::TelemetrySettings,
}

#[derive(Document, Deserialize)]
pub struct Application {
    /// Port to listen on for gRPC status requests
    #[doku(example = "8080")]
    pub listen_port: u16,

    /// Hostname to listen on for gRPC status requests
    #[doku(example = "localhost")]
    pub listen_host: String,

    /// Directory where the application databases are located
    #[doku(example = "/var/db/reverb")]
    pub application_db_dir: PathBuf,
}
```

The generated toml config file:
```toml
# App Settings
[application]
# Port to listen on for gRPC status requests
listen_port = 8080

# Hostname to listen on for gRPC status requests
listen_host = "localhost"

# Directory where the application databases are located
application_db_dir = "/var/db/reverb"

[telemetry.trace]
# Optional
endpoint = "http://localhost:4317"

[telemetry.log]
console_level = "debug,yourcrate=trace"
otel_level = "warn,yourcrate=debug"
# Optional
endpoint = "http://localhost:4317"

[telemetry.metric]
# Optional
endpoint = "http://localhost:4318/v1/metrics"
```

As you can see, the doc comments are written into the config, and the Doku `example` becomes the value.

### Application start-up

Parsing CLI arguments, loading app config file, and setting up telemetry is done in approximately 4 lines (excluding the structs for the config), making it simple and consistent to use.

```rust
use doku::Document;
use serde::Deserialize;

mod settings {
    use super::*;
    use std::path::PathBuf;

    #[derive(Document, Deserialize)]
    pub struct Settings {
        pub application: Application,
        pub telemetry: byre::telemetry::TelemetrySettings,
    }

    #[derive(Document, Deserialize)]
    pub struct Application {
        #[doku(example = "8080")]
        pub listen_port: u16,
        #[doku(example = "localhost")]
        pub listen_host: String,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let service_info = byre::service_info!();
    let Some(cli) = byre::cli::Cli::<settings::Settings>::try_new(&service_info, "APP_")? else {
        // Config file was generated, exit successfully
        return Ok(());
    };

    let _telemetry = byre::telemetry::init(&service_info, &cli.config.telemetry)?;

    // Find the SocketAddr that we should bind to
    let listen_port = cli.config.application.listen_port;
    let listen_hostname = &cli.config.application.listen_host;

    // ...

    Ok(())
}
```

### Config overrides from the environment

Environment variables override the values parsed from the config file. In this example `"APP_"` is the common prefix. If you do not want a prefix, pass an empty string (`""`).

```rust
let Some(cli) = byre::cli::Cli::<settings::Settings>::try_new(&service_info, "APP_")? else {
    return Ok(()); // Config was generated
};
```

Overriding values in a nested structure is possible. For example, if we wanted to override the `application.listen_port` you would set an environment variable `APP_APPLICATION__LISTEN_PORT`. Notice the double underscore (`__`), it is used in place of a period (`.`).

### OpenTelemetry

Setting up the connection to OpenTelemetry systems is done by calling `init`:
```rust
let _telemetry = byre::telemetry::init(&service_info, &cli.config.telemetry)?;
```

Logging, Tracing, and Metrics are available. To disable sending traces, logs, or metrics you can remove the optional `endpoint`. If you want to disable console logs set `console_level` to `"off"`.

```toml
[telemetry.trace]
# Optional
endpoint = "http://localhost:4317"

[telemetry.log]
console_level = "debug,yourcrate=trace"
otel_level = "warn,yourcrate=debug"
# Optional
endpoint = "http://localhost:4317"

[telemetry.metric]
# Optional
endpoint = "http://localhost:4318/v1/metrics"
```

#### Log Level Filtering

The `otel_level` filter is applied first to the tracing subscriber, then `console_level` filters what gets printed to the console. This means `console_level` can only show logs that pass through `otel_level`. For example, if `otel_level` is `warn`, then `console_level` can only display `warn`, `error`, or be set to `off`.

To prevent infinite telemetry loops, logs from the following crates are automatically filtered out and will not be sent to OpenTelemetry endpoints: `hyper`, `opentelemetry`, `tonic`, `h2`, and `reqwest`.

### Examples

See the [full example](https://github.com/halzy/byre/tree/main/examples/full.rs) in the source tree for a complete working application.
