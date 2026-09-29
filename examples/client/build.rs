use std::path::PathBuf;
use std::{env, fs};

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let config = manifest_dir.join("config.toml");
    let config_template = manifest_dir.join("config.toml.template");

    if !config.exists() {
        fs::copy(&config_template, &config).unwrap();
    }
    println!("cargo::rerun-if-changed=config.toml.template");
}
