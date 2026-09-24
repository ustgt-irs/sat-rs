use std::str::FromStr as _;

pub fn setup_logger() -> Result<(), fern::InitError> {
    // Read the RUST_LOG environment variable and parse it.
    // If the variable is not set or is invalid, default to Debug.
    let mut log_level = log::LevelFilter::Info;
    if let Ok(log_level_str) = std::env::var("RUST_LOG") {
        log_level = log::LevelFilter::from_str(&log_level_str).unwrap_or(log::LevelFilter::Info);
    }

    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "{}[{}][{}] {}",
                chrono::Local::now().format("[%Y-%m-%d][%H:%M:%S]"),
                std::thread::current().name().expect("unnamed_thread"),
                record.level(),
                message
            ))
        })
        .level(log_level)
        .chain(std::io::stdout())
        .chain(fern::log_file("output.log")?)
        .apply()?;
    Ok(())
}
