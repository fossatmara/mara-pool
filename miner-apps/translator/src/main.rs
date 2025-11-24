mod args;
use stratum_apps::config_helpers::logging::init_logging;
use stratum_apps::panic_hook::install_panic_hook;
pub use translator_sv2::{config, error, status, sv1, sv2, TranslatorSv2};

use crate::args::process_cli_args;

/// Entrypoint for the Translator binary.
///
/// Loads the configuration from TOML and initializes the main runtime
/// defined in `translator_sv2::TranslatorSv2`. Errors during startup are logged.
#[tokio::main]
async fn main() {
    let proxy_config = process_cli_args().unwrap_or_else(|e| {
        eprintln!("Translator proxy config error: {e}");
        std::process::exit(1);
    });

    init_logging(proxy_config.log_dir());

    // Install panic hook to log panics before crash
    install_panic_hook();

    TranslatorSv2::new(proxy_config).start().await;
}
