use std::path::{Path, PathBuf};

use clap::{Arg, ArgAction, ArgMatches, Command, Parser};
use figment::{
    providers::{Env, Format as _, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};
use snafu::{ResultExt, Snafu};

use crate::ServiceInfo;

const GENERATE_CONFIG_OPT_ID: &str = "generate";
const USE_CONFIG_OPT_ID: &str = "config";

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Could not load application configuration: {source}"))]
    FigmentError { source: figment::Error },
}

#[derive(clap::Parser, Serialize, Deserialize)]
pub struct NoArguments {}

// Wish List:
//  * Easily
//    * parse a config file
//    * generate a config file
//    * override config from environment
//  * Generating documentation should exit the software
//  * Escape hatches
pub struct Cli<C, A = NoArguments> {
    pub args: A,
    pub config: C,
}

impl<'a, C, A> Cli<C, A>
where
    A: Parser + Serialize + Deserialize<'a>,
    C: Deserialize<'a> + doku::Document,
{
    pub fn new(service_info: &ServiceInfo, env_prefix: impl AsRef<str>) -> Self
where {
        // What about the service info? generating config file examples
        let arg_command = A::command();

        let cmd = Command::new(service_info.name)
            .version(service_info.version)
            .author(service_info.author)
            .about(
                arg_command
                    .get_about()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| service_info.description.to_owned()),
            )
            .args(arg_command.get_arguments())
            .arg(
                Arg::new("config")
                    .required_unless_present(GENERATE_CONFIG_OPT_ID)
                    .action(ArgAction::Set)
                    .long("config")
                    .short('c')
                    .help("Specifies the config to run the service with"),
            )
            .arg(
                Arg::new(GENERATE_CONFIG_OPT_ID)
                    .action(ArgAction::Set)
                    .long("generate")
                    .short('g')
                    .help("Generates a new default config for the service"),
            );

        let mut arg_matches = cmd.get_matches();
        if let Some(config_file_path_str) = arg_matches.remove_one::<String>(GENERATE_CONFIG_OPT_ID)
        {
            let config_file_path = PathBuf::from(config_file_path_str);
            let config_contents = doku::to_toml::<C>();
            if let Err(err) = std::fs::write(&config_file_path, config_contents) {
                eprintln!(
                    "Could not write to {}: {err}",
                    config_file_path.to_string_lossy()
                );
            };
            std::process::exit(0);
        }

        let config_path_str = arg_matches
            .remove_one::<String>(USE_CONFIG_OPT_ID)
            .expect("clap should have required config");

        let res = A::from_arg_matches_mut(&mut arg_matches); //.map_err(format_error::<Self>);
        let args = match res {
            Ok(s) => s,
            Err(e) => {
                eprintln!("There was an error parsing arg matches!");
                // Since this is more of a development-time error, we aren't doing as fancy of a quit
                // as `get_matches`
                e.exit()
            }
        };

        // Load information from the command line
        let env_prefix = env_prefix.as_ref();
        let f = Figment::new()
            .merge(Serialized::defaults(&args))
            // from the config file
            .merge(Toml::file(config_path_str))
            // and from the environment
            .merge(Env::prefixed(env_prefix));

        let config = match f.extract().with_context(|_| FigmentSnafu {}) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("{}", err.to_string());
                std::process::exit(1);
            }
        };

        Self { args, config }
    }
}
