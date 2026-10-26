all: check build embedded test check-fmt clippy embedded-clippy docs

check:
  cargo check
  cargo check -p example-std --no-default-features

build:
  cargo build

test:
  cargo nextest run --all-features
  cargo test --doc --all-features

# Writes an HTML coverage report of satrs to target/llvm-cov/html. Pass --open to open it.
coverage *args:
  cargo llvm-cov -p satrs --html {{args}}

embedded: embedded-stm32h7

[working-directory:"examples/embedded"]
embedded-stm32h7:
  cargo build --target=thumbv7em-none-eabihf --release

clean:
  cargo clean
  cargo clean --manifest-path examples/embedded/Cargo.toml

check-fmt:
  cargo fmt --all -- --check
  cargo fmt --all --manifest-path examples/embedded/Cargo.toml -- --check

fmt:
  cargo fmt --all
  cargo fmt --all --manifest-path examples/embedded/Cargo.toml

clippy:
  cargo clippy -- -D warnings

# Runs in the embedded directory, so the embedded target from its cargo config is used.
[working-directory:"examples/embedded"]
embedded-clippy:
  cargo clippy -- -D warnings

docs-satrs:
  RUSTDOCFLAGS="--cfg docsrs --generate-link-to-definition -Z unstable-options" cargo +nightly doc -p satrs --all-features

docs: docs-satrs

[working-directory:"satrs-book"]
book *args:
  mdbook build {{args}}

# Pass --dry-run to preview the changes first.
[working-directory:"satrs-book"]
deploy-book *args: book
  rsync -avz --delete {{args}} book/html/ numalfix@documentation.irs.uni-stuttgart.de:/home/numalfix/www/projects/sat-rs/book/
