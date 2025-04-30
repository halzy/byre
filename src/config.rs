//! # Configuration File Handling
//!
//! This module provides functionality for:
//!
//! - Loading configuration from TOML files
//! - Generating sample configuration files with documentation
//! - Overriding configuration values with environment variables
//!
//! The implementation uses [figment](https://docs.rs/figment) for configuration loading and
//! [doku](https://docs.rs/doku) for generating documented sample configuration files.

use std::path::{Path, PathBuf};

use figment::{
    providers::{Env, Format as _, Toml},
    Figment,
};
use serde::Deserialize;
use snafu::ResultExt as _;

use crate::{ConfigFileWriteSnafu, Error};

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

        let config = f.extract().map_err(|err| super::Error::ConfigLoad {
            source: Box::new(err),
        })?;

        Ok(Self { config })
    }
}
