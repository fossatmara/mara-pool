use pool_sv2::PoolSv2;
use stratum_apps::config_helpers::logging::init_logging;
use stratum_apps::panic_hook::install_panic_hook;

use crate::args::process_cli_args;

mod args;

#[tokio::main]
async fn main() {
    let config = process_cli_args();
    init_logging(config.log_dir());
    install_panic_hook();
    if let Err(e) = PoolSv2::new(config).start().await {
        tracing::error!("Pool Error'ed out: {e}");
    };
}
