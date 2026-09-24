use heck::{ToShoutySnakeCase, ToSnakeCase};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Write},
};

use toml::{Value, map::Map};

fn main() -> io::Result<()> {
    // Read the configuration file
    let config_str = fs::read_to_string("components.toml").expect("Unable to read file");
    let config: Value = toml::from_str(&config_str).expect("Unable to parse TOML");

    let mut output = File::create("../../examples/obsw-std/src/ids.rs")?;

    generate_rust_code(&config, &mut output);
    Ok(())
}

fn sort_enum_table(table_map: &Map<String, Value>) -> BTreeMap<u64, &str> {
    // Collect entries into a BTreeMap to sort them by key
    let mut sorted_entries: BTreeMap<u64, &str> = BTreeMap::new();

    for (key, value) in table_map {
        if let Some(value) = value.as_integer() {
            if !(0..=0x7FF).contains(&value) {
                panic!("Invalid APID value: {}", value);
            }
            sorted_entries.insert(value as u64, key);
        }
    }
    sorted_entries
}

fn generate_rust_code(config: &Value, writer: &mut impl Write) {
    writeln!(
        writer,
        "//! This is an auto-generated configuration module."
    )
    .unwrap();
    writeln!(writer, "use satrs::request::UniqueApidTargetId;").unwrap();
    writeln!(writer).unwrap();

    // Generate the main module
    writeln!(
        writer,
        "#[derive(Debug, Copy, Clone, PartialEq, Eq, strum::EnumIter)]"
    )
    .unwrap();
    writeln!(writer, "pub enum Apid {{").unwrap();

    // Generate constants for the main module
    if let Some(apid_table) = config.get("apid").and_then(Value::as_table) {
        let sorted_entries = sort_enum_table(apid_table);
        // Write the sorted entries to the writer
        for (value, key) in sorted_entries {
            writeln!(writer, "    {} = {},", key, value).unwrap();
        }
    }
    writeln!(writer, "}}").unwrap();

    // Generate ID tables.
    if let Some(id_tables) = config.get("ids").and_then(Value::as_table) {
        for (mod_name, table) in id_tables {
            let mod_name_as_snake = mod_name.to_snake_case();
            writeln!(writer).unwrap();
            writeln!(writer, "pub mod {} {{", mod_name_as_snake).unwrap();
            let sorted_entries = sort_enum_table(table.as_table().unwrap());
            writeln!(writer, "    #[derive(Debug, Copy, Clone, PartialEq, Eq)]").unwrap();
            writeln!(writer, "    pub enum Id {{").unwrap();
            // Write the sorted entries to the writer
            for (value, key) in &sorted_entries {
                writeln!(writer, "        {} = {},", key, value).unwrap();
            }
            writeln!(writer, "    }}").unwrap();
            writeln!(writer).unwrap();

            for (_value, key) in sorted_entries {
                let key_shouting = key.to_shouty_snake_case();
                writeln!(
                    writer,
                    "    pub const {}: super::UniqueApidTargetId = super::UniqueApidTargetId::new(super::Apid::{} as u16, Id::{} as u32);",
                key_shouting, mod_name, key
                ).unwrap();
            }

            writeln!(writer, "}}").unwrap();
        }
    }
}
