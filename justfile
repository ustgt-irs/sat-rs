all: check build embedded test check-fmt clippy docs

check:
  cargo check
  cargo check -p satrs-example --no-default-features

build:
  cargo build

test:
  cargo nextest run --all-features
  cargo test --doc --all-features

embedded: embedded-stm32h7 embedded-stm32f3
  cargo check -p satrs --target=thumbv7em-none-eabihf --no-default-features

[working-directory:"embedded-examples/stm32h7-nucleo-rtic"]
embedded-stm32h7:
  cargo build --target=thumbv7em-none-eabihf --release

[working-directory:"embedded-examples/stm32f3-disco-rtic"]
embedded-stm32f3:
  cargo build --target=thumbv7em-none-eabihf --release

check-fmt:
  cargo fmt --all -- --check

fmt:
  cargo fmt --all

clippy:
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
