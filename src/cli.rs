//! Command line argument and config file tools.

use clap::{Arg, ArgAction, Command, Parser};
use serde::{Deserialize, Serialize};

use crate::{config::Config, ServiceInfo};

const GENERATE_CONFIG_OPT_ID: &str = "generate";
const USE_CONFIG_OPT_ID: &str = "config";

/// Default generic argument for `Cli` to be used when you do not need custom CLI arguments.  
#[derive(clap::Parser, Serialize, Deserialize)]
pub struct NoArguments {}

/// Cli is used to parse command line arguments, generate and load config files.
pub struct Cli<C, A = NoArguments> {
    /// parsed command line arguments
    pub args: A,

    /// parsed TOML config file with the structure of `C`
    pub config: C,
}

impl<'a, C, A> Cli<C, A>
where
    A: Parser + Serialize + Deserialize<'a>,
    C: Deserialize<'a> + doku::Document,
{
    /// Parse command line arguments, generate or load the config file, and apply config overrides from environment variables with `env_prefix`
    pub fn new(service_info: &ServiceInfo, env_prefix: impl AsRef<str>) -> Self {
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
                    .help("Specifies the toml config file to run the service with"),
            )
            .arg(
                Arg::new(GENERATE_CONFIG_OPT_ID)
                    .action(ArgAction::Set)
                    .long("generate")
                    .short('g')
                    .help("Generates a new default toml config file for the service"),
            );

        let mut arg_matches = cmd.get_matches();
        if let Some(config_file_path_str) = arg_matches.remove_one::<String>(GENERATE_CONFIG_OPT_ID)
        {
            if let Err(err) = crate::config::create_config_file::<C>(config_file_path_str) {
                eprintln!("{err}",);
            }

            std::process::exit(0);
        }

        let config_path_str = arg_matches
            .remove_one::<String>(USE_CONFIG_OPT_ID)
            .expect("clap should have required config");

        let res = A::from_arg_matches_mut(&mut arg_matches);
        let args = match res {
            Ok(s) => s,
            Err(e) => {
                eprintln!("There was an error parsing arg matches!");
                // Since this is more of a development-time error, we aren't doing as fancy of a quit
                // as `get_matches`
                e.exit();
            }
        };

        let env_prefix = env_prefix.as_ref();
        let config_result = Config::new(Some(config_path_str), Some(env_prefix));

        let config = match config_result {
            Ok(config) => config.config,
            Err(err) => {
                eprintln!("{}", err.to_string());
                std::process::exit(1);
            }
        };

        Self { args, config }
    }
}
