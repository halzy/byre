//! # Configuration File Handling
//!
//! This module provides functionality for:
//!
//! - Loading configuration from TOML files
//! - Generating sample configuration files with documentation
//! - Overriding configuration values with environment variables
//! - Expanding environment variable references in config values (`${VAR}` syntax)
//!
//! The implementation uses [figment](https://docs.rs/figment) for configuration loading and
//! [doku](https://docs.rs/doku) for generating documented sample configuration files.

use std::path::{Path, PathBuf};

use figment::{
    providers::{Env, Format as _, Toml},
    value::{Dict, Map, Value},
    Figment, Metadata, Profile, Provider,
};
use serde::Deserialize;
use snafu::ResultExt as _;

use crate::{ConfigFileWriteSnafu, Error};

/// Expand environment variable references in a string value.
///
/// Supports two syntaxes:
/// - `${VAR}` - expands to the value of environment variable VAR
/// - `$VAR` - expands to the value of environment variable VAR
///
/// If the environment variable is not set, the original value is returned unchanged.
/// Values that don't start with `$` are returned as-is.
///
/// # Examples
///
/// ```
/// use byre::config::expand_env_var;
///
/// // In dev mode (literal values in config):
/// assert_eq!(expand_env_var("dev-1"), "dev-1");
///
/// // Environment variable expansion:
/// std::env::set_var("MY_TEST_VAR", "expanded-value");
/// assert_eq!(expand_env_var("${MY_TEST_VAR}"), "expanded-value");
/// std::env::remove_var("MY_TEST_VAR");
/// ```
pub fn expand_env_var(value: &str) -> String {
    if let Some(var_name) = value.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        std::env::var(var_name).unwrap_or_else(|_| value.to_string())
    } else if let Some(var_name) = value.strip_prefix('$') {
        std::env::var(var_name).unwrap_or_else(|_| value.to_string())
    } else {
        value.to_string()
    }
}

/// Recursively expand environment variable references in a configuration value.
fn expand_value(value: Value) -> Value {
    match value {
        Value::String(tag, s) => {
            let expanded = expand_env_var(&s);
            Value::String(tag, expanded)
        }
        Value::Dict(tag, dict) => Value::Dict(tag, expand_dict(dict)),
        Value::Array(tag, arr) => {
            Value::Array(tag, arr.into_iter().map(expand_value).collect())
        }
        other => other,
    }
}

/// Recursively expand environment variable references in a dictionary.
fn expand_dict(dict: Dict) -> Dict {
    dict.into_iter()
        .map(|(k, v)| (k, expand_value(v)))
        .collect()
}

/// A Figment provider that expands environment variable references in string values.
///
/// This provider wraps another provider's data and expands `${VAR}` and `$VAR`
/// patterns in all string values to their corresponding environment variable values.
struct EnvExpander {
    data: Map<Profile, Dict>,
}

impl EnvExpander {
    /// Create a new EnvExpander from a Figment's merged data.
    fn from_figment(figment: &Figment) -> Result<Self, figment::Error> {
        let data = figment.data()?;
        let expanded_data = data
            .into_iter()
            .map(|(profile, dict)| (profile, expand_dict(dict)))
            .collect();
        Ok(Self {
            data: expanded_data,
        })
    }
}

impl Provider for EnvExpander {
    fn metadata(&self) -> Metadata {
        Metadata::named("env-expander")
    }

    fn data(&self) -> Result<Map<Profile, Dict>, figment::Error> {
        Ok(self.data.clone())
    }
}

/// Generates a documented configuration file at the specified path.
///
/// This function uses the [doku](https://docs.rs/doku) library to extract documentation
/// from a type that implements `doku::Document` and generate a TOML file with
/// commented examples. This is particularly useful for helping users understand
/// the available configuration options and their purpose.
///
/// This function can be used directly when the `Cli` struct is not appropriate
/// for your use case.
///
/// # Arguments
///
/// * `config_path` - Path where the configuration file should be created
///
/// # Type Parameters
///
/// * `C` - The configuration type that implements `doku::Document`
///
/// # Errors
/// - `ConfigFileWrite` if the config file cannot be written.
pub fn create_config_file<C>(config_path: impl Into<PathBuf>) -> Result<(), Error>
where
    C: doku::Document,
{
    let path = config_path.into();
    let config_contents = doku::to_toml::<C>();
    std::fs::write(&path, config_contents).with_context(|_| ConfigFileWriteSnafu { path })?;
    Ok(())
}

/// Container for loaded and merged configuration.
///
/// This struct loads configuration from multiple sources and makes it available
/// through the `config` field. The loading order (from lowest to highest precedence) is:
///
/// 1. Default values defined in the configuration struct
/// 2. Values from the TOML configuration file
/// 3. Values from environment variables with the specified prefix
///
/// Environment variables override configuration using double underscores (`__`) to
/// represent nesting. For example, `APP__DATABASE__PORT=5432` would override
/// the `port` field in the `database` section of the configuration.
pub struct Config<C> {
    /// The fully loaded and merged configuration instance.
    ///
    /// This contains the final configuration after applying all defaults,
    /// file-based configuration values, and environment variable overrides.
    pub config: C,
}

impl<'a, C> Config<C>
where
    C: Deserialize<'a> + doku::Document,
{
    /// Creates a new `Config` instance by loading and merging configuration from multiple sources.
    ///
    /// This method loads configuration in the following order (from lowest to highest precedence):
    ///
    /// 1. Default values defined in the configuration struct
    /// 2. Values from the TOML configuration file (if provided)
    /// 3. Values from environment variables with the specified prefix (if provided)
    ///
    /// # Arguments
    ///
    /// * `config_path` - Optional path to a TOML configuration file
    /// * `env_prefix` - Optional prefix for environment variables that should override configuration values
    ///
    /// # Type Parameters
    ///
    /// * `P` - Type that can be converted to a path
    /// * `E` - Type that can be converted to a string for the environment prefix
    ///
    /// # Errors
    /// - `ConfigLoad` if the config file cannot be loaded or parsed.
    pub fn new<P, E>(config_path: Option<P>, env_prefix: Option<E>) -> Result<Self, Error>
    where
        P: AsRef<Path>,
        E: AsRef<str>,
    {
        // Load information from the command line
        let f = Figment::new();

        // from the config file
        let f = match config_path {
            Some(config_file) => f.merge(Toml::file(config_file)),
            None => f,
        };

        // and from the environment
        let f = match env_prefix {
            Some(env_prefix) => {
                let env_prefix = env_prefix.as_ref();
                f.merge(Env::prefixed(env_prefix).split("__"))
            }
            None => f,
        };

        // Expand environment variable references in string values (${VAR} and $VAR syntax)
        let expander =
            EnvExpander::from_figment(&f).map_err(|err| super::Error::ConfigLoad {
                source: Box::new(err),
            })?;
        let f = Figment::from(expander);

        let config = f.extract().map_err(|err| super::Error::ConfigLoad {
            source: Box::new(err),
        })?;

        Ok(Self { config })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_env_var_literal_value() {
        // Literal values (no $ prefix) are returned unchanged
        assert_eq!(expand_env_var("dev-1"), "dev-1");
        assert_eq!(expand_env_var("my-node"), "my-node");
        assert_eq!(expand_env_var(""), "");
        assert_eq!(expand_env_var("plain-text"), "plain-text");
    }

    #[test]
    fn expand_env_var_with_braces() {
        // Set a test env var
        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::set_var("BYRE_TEST_EXPAND_VAR_BRACES", "expanded-value");
        }

        // ${VAR} syntax should expand
        assert_eq!(
            expand_env_var("${BYRE_TEST_EXPAND_VAR_BRACES}"),
            "expanded-value"
        );

        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::remove_var("BYRE_TEST_EXPAND_VAR_BRACES");
        }
    }

    #[test]
    fn expand_env_var_without_braces() {
        // Set a test env var
        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::set_var("BYRE_TEST_EXPAND_VAR_NO_BRACES", "expanded-value");
        }

        // $VAR syntax should expand
        assert_eq!(
            expand_env_var("$BYRE_TEST_EXPAND_VAR_NO_BRACES"),
            "expanded-value"
        );

        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::remove_var("BYRE_TEST_EXPAND_VAR_NO_BRACES");
        }
    }

    #[test]
    fn expand_env_var_missing_returns_original() {
        // Missing env var returns the original value unchanged
        let original = "${BYRE_DEFINITELY_NOT_SET_12345}";
        assert_eq!(expand_env_var(original), original);

        let original_no_braces = "$BYRE_DEFINITELY_NOT_SET_12345";
        assert_eq!(expand_env_var(original_no_braces), original_no_braces);
    }

    #[test]
    fn expand_value_handles_strings() {
        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::set_var("BYRE_TEST_VALUE_STRING", "test-value");
        }

        let value = Value::String(Default::default(), "${BYRE_TEST_VALUE_STRING}".to_string());
        let expanded = expand_value(value);
        match expanded {
            Value::String(_, s) => assert_eq!(s, "test-value"),
            _ => panic!("Expected String value"),
        }

        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::remove_var("BYRE_TEST_VALUE_STRING");
        }
    }

    #[test]
    fn expand_value_handles_non_strings() {
        // Non-string values should pass through unchanged
        let num_value = Value::from(42i64);
        let expanded = expand_value(num_value.clone());
        assert_eq!(format!("{:?}", expanded), format!("{:?}", num_value));

        let bool_value = Value::from(true);
        let expanded = expand_value(bool_value.clone());
        assert_eq!(format!("{:?}", expanded), format!("{:?}", bool_value));
    }

    #[test]
    fn expand_value_handles_arrays() {
        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::set_var("BYRE_TEST_ARRAY_VAR", "array-value");
        }

        let arr = Value::Array(
            Default::default(),
            vec![
                Value::String(Default::default(), "${BYRE_TEST_ARRAY_VAR}".to_string()),
                Value::String(Default::default(), "literal".to_string()),
            ],
        );
        let expanded = expand_value(arr);
        match expanded {
            Value::Array(_, items) => {
                assert_eq!(items.len(), 2);
                match &items[0] {
                    Value::String(_, s) => assert_eq!(s, "array-value"),
                    _ => panic!("Expected String value"),
                }
                match &items[1] {
                    Value::String(_, s) => assert_eq!(s, "literal"),
                    _ => panic!("Expected String value"),
                }
            }
            _ => panic!("Expected Array value"),
        }

        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::remove_var("BYRE_TEST_ARRAY_VAR");
        }
    }

    #[test]
    fn expand_dict_handles_nested_values() {
        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::set_var("BYRE_TEST_DICT_VAR", "dict-value");
        }

        let mut dict = Dict::new();
        dict.insert(
            "key1".to_string(),
            Value::String(Default::default(), "${BYRE_TEST_DICT_VAR}".to_string()),
        );
        dict.insert(
            "key2".to_string(),
            Value::String(Default::default(), "literal".to_string()),
        );

        let expanded = expand_dict(dict);
        match expanded.get("key1") {
            Some(Value::String(_, s)) => assert_eq!(s, "dict-value"),
            _ => panic!("Expected String value for key1"),
        }
        match expanded.get("key2") {
            Some(Value::String(_, s)) => assert_eq!(s, "literal"),
            _ => panic!("Expected String value for key2"),
        }

        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::remove_var("BYRE_TEST_DICT_VAR");
        }
    }

    #[test]
    fn expand_dict_handles_nested_dicts() {
        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::set_var("BYRE_TEST_NESTED_VAR", "nested-value");
        }

        let mut inner_dict = Dict::new();
        inner_dict.insert(
            "nested_key".to_string(),
            Value::String(Default::default(), "${BYRE_TEST_NESTED_VAR}".to_string()),
        );

        let mut outer_dict = Dict::new();
        outer_dict.insert(
            "outer".to_string(),
            Value::Dict(Default::default(), inner_dict),
        );

        let expanded = expand_dict(outer_dict);
        match expanded.get("outer") {
            Some(Value::Dict(_, inner)) => match inner.get("nested_key") {
                Some(Value::String(_, s)) => assert_eq!(s, "nested-value"),
                _ => panic!("Expected String value for nested_key"),
            },
            _ => panic!("Expected Dict value for outer"),
        }

        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::remove_var("BYRE_TEST_NESTED_VAR");
        }
    }

    #[test]
    fn env_expander_creates_from_figment() {
        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::set_var("BYRE_TEST_FIGMENT_VAR", "figment-value");
        }

        // Create a minimal figment with raw data
        let figment = Figment::new().merge(("node_id", "${BYRE_TEST_FIGMENT_VAR}"));

        let expander = EnvExpander::from_figment(&figment).unwrap();
        let data = expander.data().unwrap();

        // Check that the value was expanded - find any profile that has the data
        let mut found = false;
        for (_profile, profile_data) in data.iter() {
            if let Some(Value::String(_, s)) = profile_data.get("node_id") {
                assert_eq!(s, "figment-value");
                found = true;
                break;
            }
        }
        assert!(found, "Expected to find node_id in some profile");

        // SAFETY: Test runs in a single thread, no concurrent env access
        unsafe {
            std::env::remove_var("BYRE_TEST_FIGMENT_VAR");
        }
    }
}
