//! Panic hook utilities for capturing detailed crash information
//!
//! Provides a standardized panic hook that logs panic location, message,
//! and full backtrace before the process terminates. This is essential for
//! debugging production crashes.

/// Install a panic hook that captures and logs detailed panic information.
///
/// This hook will:
/// - Log the panic location (file, line, column)
/// - Log the panic message
/// - Capture and log a full backtrace
///
/// The hook writes to stderr, so output can be captured by systemd, Docker logs,
/// or file redirection.
///
/// # Example
///
/// ```no_run
/// # use stratum_apps::panic_hook::install_panic_hook;
/// install_panic_hook();
/// // ... rest of your application
/// ```
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let location = panic_info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());

        let message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "Box<dyn Any>".to_string()
        };

        eprintln!("╔════════════════════════════════════════════════════════════════╗");
        eprintln!("║                         PANIC OCCURRED                         ║");
        eprintln!("╠════════════════════════════════════════════════════════════════╣");
        eprintln!("║ Location: {:<52} ║", location);
        eprintln!("║ Message:  {:<52} ║", message);
        eprintln!("╠════════════════════════════════════════════════════════════════╣");
        eprintln!("║ Backtrace:                                                     ║");
        eprintln!("╚════════════════════════════════════════════════════════════════╝");
        eprintln!("{:?}", std::backtrace::Backtrace::force_capture());
        eprintln!("╚════════════════════════════════════════════════════════════════╝");
    }));
}
