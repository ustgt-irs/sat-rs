use std::path::PathBuf;
use std::{env, fs};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let cargo_dir = manifest_dir.join(".cargo");
    let config = cargo_dir.join("config.toml");
    let config_template = cargo_dir.join("config.toml.template");

    if !config.exists() && config_template.exists() {
        fs::create_dir_all(&cargo_dir).unwrap();
        fs::copy(&config_template, &config).unwrap();
    }
}
