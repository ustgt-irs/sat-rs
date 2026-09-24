use std::{fs::File, io::Read as _, net::SocketAddr, path::Path, time::SystemTime};

#[derive(Debug, serde::Deserialize)]
pub struct Config {
    pub interface: Interface,
}

#[derive(Debug, serde::Deserialize)]
pub struct Interface {
    pub serial_port: Option<String>,
    pub udp_addr: Option<SocketAddr>,
}

impl Config {
    pub fn new_from_file() -> Self {
        let mut config_file =
            File::open(Path::new("config.toml")).expect("opening config.toml file failed");
        let mut toml_str = String::new();
        config_file
            .read_to_string(&mut toml_str)
            .expect("reading config.toml file failed");
        let config: Config = toml::from_str(&toml_str).expect("parsing config.toml file failed");
        config
    }
}

pub fn setup_logger() -> Result<(), fern::InitError> {
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{} {} {}] {}",
                humantime::format_rfc3339_seconds(SystemTime::now()),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(log::LevelFilter::Info)
        .chain(std::io::stdout())
        .chain(fern::log_file("output.log")?)
        .apply()?;
    Ok(())
}
