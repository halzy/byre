use std::{
    marker::PhantomData,
    path::{Path, PathBuf},
};

use figment::{
    providers::{Env, Format as _, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use snafu::ResultExt as _;

#[derive(Debug, snafu::Snafu)]
pub enum Error {
    #[snafu(display("Could not load application configuration: {source}"))]
    FigmentError { source: figment::Error },

    #[snafu(display("Could not write to the config file at {path:?}: {source}"))]
    ConfigFileWriteError {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub fn create_config_file<C>(config_path: impl Into<PathBuf>) -> Result<(), Error>
where
    C: doku::Document,
{
    let path = config_path.into();
    let config_contents = doku::to_toml::<C>();
    std::fs::write(&path, config_contents).with_context(|_| ConfigFileWriteSnafu { path })?;
    Ok(())
}

#[derive(Default, Serialize)]
pub struct NoDefaults {}

pub struct Config<C, D = NoDefaults> {
    pub config: C,
    defaults: PhantomData<D>,
}
impl<'a, C, D> Config<C, D>
where
    C: Deserialize<'a> + doku::Document,
    D: Default + Serialize,
{
    ///
    pub fn new<P, E>(config_path: Option<P>, env_prefix: Option<E>) -> Result<Self, Error>
    where
        P: AsRef<Path>,
        E: AsRef<str>,
    {
        let defaults = D::default();
        let defaults = Serialized::defaults(&defaults);

        Self::new_with_default_values(defaults, config_path, env_prefix)
    }
}

impl<'a, C, D> Config<C, D>
where
    C: Deserialize<'a> + doku::Document,
    D: Serialize,
{
    pub(crate) fn new_with_default_values<P, E>(
        defaults: Serialized<&D>,
        config_path: Option<P>,
        env_prefix: Option<E>,
    ) -> Result<Self, Error>
    where
        P: AsRef<Path>,
        E: AsRef<str>,
    {
        // Load information from the command line
        let f = Figment::new().merge(defaults);

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

        let config = f.extract().with_context(|_| FigmentSnafu {})?;

        Ok(Self {
            config,
            defaults: PhantomData,
        })
    }
}
