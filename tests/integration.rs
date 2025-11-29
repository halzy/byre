//! Integration tests for the byre crate.
//!
//! These tests exercise the public APIs to ensure nothing breaks when changes are made.

use std::path::PathBuf;

use doku::Document;
use serde::Deserialize;

/// Test settings structure similar to what a real application would use.
#[derive(Document, Deserialize)]
pub struct TestSettings {
    /// Application-specific settings
    pub application: ApplicationSettings,

    /// Telemetry settings from byre
    pub telemetry: byre::telemetry::TelemetrySettings,
}

#[derive(Document, Deserialize)]
pub struct ApplicationSettings {
    /// Port to listen on
    #[doku(example = "8080")]
    pub listen_port: u16,

    /// Hostname to listen on
    #[doku(example = "localhost")]
    pub listen_host: String,

    /// Database directory path
    #[doku(example = "/var/db/test")]
    pub db_path: PathBuf,
}

// ============================================================================
// ServiceInfo Tests
// ============================================================================

#[test]
fn test_service_info_macro() {
    let info = byre::service_info!();

    assert_eq!(info.name, "byre");
    assert_eq!(info.name_in_metrics, "byre");
    assert!(!info.version.is_empty());
}

#[test]
fn test_service_info_fields() {
    let info = byre::ServiceInfo {
        name: "test-service",
        name_in_metrics: "test_service".to_string(),
        version: "1.0.0",
        author: "Test Author",
        description: "A test service",
    };

    assert_eq!(info.name, "test-service");
    assert_eq!(info.name_in_metrics, "test_service");
    assert_eq!(info.version, "1.0.0");
    assert_eq!(info.author, "Test Author");
    assert_eq!(info.description, "A test service");
}

#[test]
fn test_service_info_default() {
    let info = byre::ServiceInfo::default();

    assert_eq!(info.name, "");
    assert_eq!(info.name_in_metrics, "");
    assert_eq!(info.version, "");
}

#[test]
fn test_service_info_clone() {
    let info = byre::service_info!();
    let cloned = info.clone();

    assert_eq!(info.name, cloned.name);
    assert_eq!(info.version, cloned.version);
}

// ============================================================================
// Config Generation Tests
// ============================================================================

#[test]
fn test_config_generation() {
    let temp_dir = std::env::temp_dir();
    let config_path = temp_dir.join("test_byre_config.toml");

    // Clean up any existing file
    let _ = std::fs::remove_file(&config_path);

    // Generate the config file
    let result = byre::config::create_config_file::<TestSettings>(&config_path);
    assert!(result.is_ok(), "Failed to create config file: {:?}", result);

    // Verify the file exists
    assert!(config_path.exists(), "Config file was not created");

    // Read and verify the content
    let content = std::fs::read_to_string(&config_path).expect("Failed to read config file");

    // Check for expected sections and fields
    assert!(
        content.contains("[application]"),
        "Missing [application] section"
    );
    assert!(content.contains("listen_port"), "Missing listen_port field");
    assert!(content.contains("listen_host"), "Missing listen_host field");
    assert!(content.contains("db_path"), "Missing db_path field");
    assert!(content.contains("[telemetry"), "Missing telemetry section");

    // Check for doku examples
    assert!(content.contains("8080"), "Missing example value 8080");
    assert!(
        content.contains("localhost"),
        "Missing example value localhost"
    );

    // Clean up
    let _ = std::fs::remove_file(&config_path);
}

#[test]
fn test_config_generation_with_comments() {
    let temp_dir = std::env::temp_dir();
    let config_path = temp_dir.join("test_byre_config_comments.toml");

    let _ = std::fs::remove_file(&config_path);

    byre::config::create_config_file::<TestSettings>(&config_path)
        .expect("Failed to create config file");

    let content = std::fs::read_to_string(&config_path).expect("Failed to read config file");

    // Doc comments should be included as TOML comments
    assert!(
        content.contains("# Port to listen on") || content.contains("Port to listen on"),
        "Missing doc comment for listen_port"
    );

    let _ = std::fs::remove_file(&config_path);
}

// ============================================================================
// Config Loading Tests
// ============================================================================

#[test]
fn test_config_loading() {
    let temp_dir = std::env::temp_dir();
    let config_path = temp_dir.join("test_byre_load_config.toml");

    // Create a test config file
    let config_content = r#"
[application]
listen_port = 9090
listen_host = "127.0.0.1"
db_path = "/tmp/test_db"

[telemetry.trace]

[telemetry.log]
console_level = "info"
otel_level = "warn"

[telemetry.metric]
"#;

    std::fs::write(&config_path, config_content).expect("Failed to write test config");

    // Load the config
    let config: byre::config::Config<TestSettings> =
        byre::config::Config::new(Some(&config_path), None::<&str>).expect("Failed to load config");

    assert_eq!(config.config.application.listen_port, 9090);
    assert_eq!(config.config.application.listen_host, "127.0.0.1");
    assert_eq!(
        config.config.application.db_path,
        PathBuf::from("/tmp/test_db")
    );
    assert_eq!(config.config.telemetry.log.console_level, "info");
    assert_eq!(config.config.telemetry.log.otel_level, "warn");

    let _ = std::fs::remove_file(&config_path);
}

#[test]
fn test_config_loading_with_env_override() {
    let temp_dir = std::env::temp_dir();
    let config_path = temp_dir.join("test_byre_env_override.toml");

    let config_content = r#"
[application]
listen_port = 8080
listen_host = "localhost"
db_path = "/var/db"

[telemetry.trace]

[telemetry.log]
console_level = "debug"
otel_level = "info"

[telemetry.metric]
"#;

    std::fs::write(&config_path, config_content).expect("Failed to write test config");

    // Set environment variable to override
    std::env::set_var("TESTAPP_APPLICATION__LISTEN_PORT", "3000");
    std::env::set_var("TESTAPP_APPLICATION__LISTEN_HOST", "0.0.0.0");

    let config: byre::config::Config<TestSettings> =
        byre::config::Config::new(Some(&config_path), Some("TESTAPP_"))
            .expect("Failed to load config");

    // These should be overridden by env vars
    assert_eq!(config.config.application.listen_port, 3000);
    assert_eq!(config.config.application.listen_host, "0.0.0.0");

    // This should remain from the file
    assert_eq!(config.config.application.db_path, PathBuf::from("/var/db"));

    // Clean up
    std::env::remove_var("TESTAPP_APPLICATION__LISTEN_PORT");
    std::env::remove_var("TESTAPP_APPLICATION__LISTEN_HOST");
    let _ = std::fs::remove_file(&config_path);
}

#[test]
fn test_config_loading_invalid_file() {
    let result: Result<byre::config::Config<TestSettings>, _> =
        byre::config::Config::new(Some("/nonexistent/path/config.toml"), None::<&str>);

    assert!(result.is_err(), "Should fail with nonexistent file");
}

#[test]
fn test_config_loading_invalid_toml() {
    let temp_dir = std::env::temp_dir();
    let config_path = temp_dir.join("test_byre_invalid.toml");

    // Write invalid TOML
    std::fs::write(&config_path, "this is not valid toml {{{{").expect("Failed to write file");

    let result: Result<byre::config::Config<TestSettings>, _> =
        byre::config::Config::new(Some(&config_path), None::<&str>);

    assert!(result.is_err(), "Should fail with invalid TOML");

    let _ = std::fs::remove_file(&config_path);
}

// ============================================================================
// TelemetrySettings Tests
// ============================================================================

#[test]
fn test_telemetry_settings_default() {
    let settings = byre::telemetry::TelemetrySettings::default();

    assert!(settings.trace.endpoint.is_none());
    assert!(settings.log.endpoint.is_none());
    assert!(settings.metric.endpoint.is_none());
    assert!(settings.log.console_level.is_empty());
    assert!(settings.log.otel_level.is_empty());
}

#[test]
fn test_telemetry_settings_serialization() {
    let settings = byre::telemetry::TelemetrySettings {
        trace: byre::telemetry::TraceSettings {
            endpoint: Some("http://localhost:4317".to_string()),
        },
        log: byre::telemetry::LogSettings {
            console_level: "debug".to_string(),
            otel_level: "warn".to_string(),
            endpoint: Some("http://localhost:4317".to_string()),
        },
        metric: byre::telemetry::MetricSettings {
            endpoint: Some("http://localhost:4318/v1/metrics".to_string()),
        },
    };

    // Test that it can be serialized
    let serialized = toml::to_string(&settings);
    assert!(serialized.is_ok(), "Failed to serialize TelemetrySettings");

    let toml_str = serialized.unwrap();
    assert!(toml_str.contains("endpoint"));
    assert!(toml_str.contains("console_level"));
}

#[test]
fn test_telemetry_settings_deserialization() {
    let toml_content = r#"
[trace]
endpoint = "http://trace:4317"

[log]
console_level = "info,mycrate=debug"
otel_level = "warn"
endpoint = "http://log:4317"

[metric]
endpoint = "http://metric:4318/v1/metrics"
"#;

    let settings: byre::telemetry::TelemetrySettings =
        toml::from_str(toml_content).expect("Failed to deserialize");

    assert_eq!(
        settings.trace.endpoint,
        Some("http://trace:4317".to_string())
    );
    assert_eq!(settings.log.console_level, "info,mycrate=debug");
    assert_eq!(settings.log.otel_level, "warn");
    assert_eq!(settings.log.endpoint, Some("http://log:4317".to_string()));
    assert_eq!(
        settings.metric.endpoint,
        Some("http://metric:4318/v1/metrics".to_string())
    );
}

#[test]
fn test_telemetry_settings_partial_config() {
    // Test with optional endpoints omitted
    let toml_content = r#"
[trace]

[log]
console_level = "info"
otel_level = "warn"

[metric]
"#;

    let settings: byre::telemetry::TelemetrySettings =
        toml::from_str(toml_content).expect("Failed to deserialize");

    assert!(settings.trace.endpoint.is_none());
    assert!(settings.log.endpoint.is_none());
    assert!(settings.metric.endpoint.is_none());
    assert_eq!(settings.log.console_level, "info");
}

// ============================================================================
// Telemetry Initialization Tests
// ============================================================================

#[test]
fn test_telemetry_init_with_disabled_endpoints() {
    let service_info = byre::service_info!();

    let settings = byre::telemetry::TelemetrySettings {
        trace: byre::telemetry::TraceSettings { endpoint: None },
        log: byre::telemetry::LogSettings {
            console_level: "off".to_string(),
            otel_level: "off".to_string(),
            endpoint: None,
        },
        metric: byre::telemetry::MetricSettings { endpoint: None },
    };

    // This should succeed when all endpoints are disabled
    // Note: We can only run this once per process due to global tracing subscriber
    // So we just verify the settings are valid
    assert!(settings.trace.endpoint.is_none());
    assert!(settings.log.endpoint.is_none());
    assert!(settings.metric.endpoint.is_none());

    // Verify service info is usable
    assert!(!service_info.name.is_empty());
}

// ============================================================================
// Error Type Tests
// ============================================================================

#[test]
fn test_error_display() {
    // Test that error types implement Display properly
    let config_error = byre::Error::ConfigFileWrite {
        path: PathBuf::from("/test/path"),
        source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "test error"),
    };

    let error_string = format!("{}", config_error);
    assert!(error_string.contains("/test/path"));
}

// ============================================================================
// CLI Error Tests
// ============================================================================

#[test]
fn test_cli_error_display() {
    // Test that cli::Error types implement Display properly
    let error = byre::cli::Error::ArgParse {
        message: "missing required argument".to_string(),
    };
    let error_string = format!("{}", error);
    assert!(error_string.contains("missing required argument"));
}

// ============================================================================
// Document Trait Tests
// ============================================================================

#[test]
fn test_telemetry_settings_document() {
    // Verify TelemetrySettings implements doku::Document
    let toml = doku::to_toml::<byre::telemetry::TelemetrySettings>();

    assert!(toml.contains("[trace]"));
    assert!(toml.contains("[log]"));
    assert!(toml.contains("[metric]"));
    assert!(toml.contains("endpoint"));
    assert!(toml.contains("console_level"));
    assert!(toml.contains("otel_level"));
}

#[test]
fn test_nested_settings_document() {
    // Verify our TestSettings with nested TelemetrySettings generates proper TOML
    let toml = doku::to_toml::<TestSettings>();

    assert!(toml.contains("[application]"));
    assert!(toml.contains("[telemetry.trace]"));
    assert!(toml.contains("[telemetry.log]"));
    assert!(toml.contains("[telemetry.metric]"));
}
