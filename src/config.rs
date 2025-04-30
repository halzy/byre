//! Config file tools.
use std::path::{Path, PathBuf};

use figment::{
    providers::{Env, Format as _, Toml},
    Figment,
};
use serde::Deserialize;
use snafu::ResultExt as _;

use crate::{ConfigFileWriteSnafu, Error};

/// Uses Doku to create a config file at the given path
///
/// This is usedful if the `Cli` struct cannot be used.
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

/// Loads the config file and applies overrides
pub struct Config<C> {
    /// an instance of the loaded config file
    pub config: C,
}

impl<'a, C> Config<C>
where
    C: Deserialize<'a> + doku::Document,
{
    /// Loads the config file and overrides
    ///
    /// # Errors
    /// - `ConfigLoad` if the config file cannot be loaded.
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
