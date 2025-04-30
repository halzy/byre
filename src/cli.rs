//! # Command Line Interface Tools
//!
//! This module provides utilities for building command-line applications with:
//!
//! - Command-line argument parsing based on `clap`
//! - TOML configuration file generation and loading
//! - Environment variable overrides for configuration values
//!
//! The design goal is to simplify the common CLI application pattern of:
//! 1. Parsing command-line arguments
//! 2. Loading configuration from files
//! 3. Overriding configuration with environment variables

use clap::{Arg, ArgAction, Command, Parser};
use serde::{Deserialize, Serialize};

use crate::{config::Config, ServiceInfo};

const GENERATE_CONFIG_OPT_ID: &str = "generate";
const USE_CONFIG_OPT_ID: &str = "config";

/// An empty arguments structure for use when no custom CLI arguments are needed.
///
/// This type implements the `clap::Parser` trait and can be used as the default
/// argument type for the `Cli` struct when you only need the built-in config file
/// functionality without additional command-line options.
#[derive(clap::Parser, Serialize, Deserialize)]
pub struct NoArguments {}

/// Main CLI handler that combines command-line arguments, configuration files, and environment variables.
///
/// This struct serves as the primary interface for CLI applications, providing:
///
/// - Type-safe access to command-line arguments via the `args` field
/// - Access to the loaded and merged configuration via the `config` field
/// - Automatic handling of config file generation and loading
/// - Application of configuration overrides from environment variables
///
/// The generic parameters control the behavior:
/// - `C`: The configuration structure type (must implement `Deserialize` and `doku::Document`)
/// - `A`: The arguments structure type (defaults to `NoArguments` if custom arguments aren't needed)
pub struct Cli<C, A = NoArguments> {
    /// Parsed command-line arguments from the user.
    ///
    /// These are the validated command-line arguments that were passed to the application
    /// according to the structure defined by type `A`.
    pub args: A,

    /// Application configuration loaded from the TOML config file and environment variables.
    ///
    /// This is the fully processed configuration that combines:
    /// 1. Default values defined in the `C` structure
    /// 2. Values from the specified configuration file
    /// 3. Overrides from environment variables (using the prefix specified in `new()`)
    pub config: C,
}

impl<'a, C, A> Cli<C, A>
where
    A: Parser + Serialize + Deserialize<'a>,
    C: Deserialize<'a> + doku::Document,
{
    /// Creates a new CLI instance by parsing arguments and loading configuration.
    ///
    /// This method:
    /// 1. Builds a command-line parser with your application info and arguments from type `A`
    /// 2. Adds the built-in `--config` and `--generate` options
    /// 3. Parses the command line
    /// 4. If `--generate` is specified, creates a sample config file and exits
    /// 5. If `--config` is specified, loads and parses the configuration file
    /// 6. Applies any environment variable overrides using the specified prefix
    /// 7. Returns a `Cli` instance with the parsed arguments and configuration
    ///
    /// # Arguments
    ///
    /// * `service_info` - Service information including name, version, and description
    /// * `env_prefix` - Prefix for environment variables that override config values
    ///
    /// # Panics
    ///
    /// This method will call `std::process::exit()` if:
    /// - A config generation is requested (after generating the file)
    /// - Command-line argument parsing fails
    /// - Configuration loading or parsing fails
    pub fn new(service_info: &ServiceInfo, env_prefix: impl AsRef<str>) -> Self {
        // What about the service info? generating config file examples
        let arg_command = A::command();

        let cmd = Command::new(service_info.name)
            .version(service_info.version)
            .author(service_info.author)
            .about(
                arg_command
                    .get_about()
                    .map_or_else(|| service_info.description.to_owned(), ToString::to_string),
            )
            .args(arg_command.get_arguments())
            .arg(
                Arg::new("config")
                    .required_unless_present(GENERATE_CONFIG_OPT_ID)
                    .action(ArgAction::Set)
                    .long(USE_CONFIG_OPT_ID)
                    .short('c')
                    .help("Specifies the toml config file to run the service with"),
            )
            .arg(
                Arg::new(GENERATE_CONFIG_OPT_ID)
                    .action(ArgAction::Set)
                    .long(GENERATE_CONFIG_OPT_ID)
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

        let Some(config_path_str) = arg_matches.remove_one::<String>(USE_CONFIG_OPT_ID) else {
            unreachable!()
        };

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
                eprintln!("{err}");
                std::process::exit(1);
            }
        };

        Self { args, config }
    }
}
