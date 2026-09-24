sat-rs example for the STM32H753ZI-Nucleo board
=======

This example application shows how the [sat-rs library](https://egit.irs.uni-stuttgart.de/rust/sat-rs)
can be used on an embedded target.
It also shows how a relatively simple OBSW could be built when no standard runtime is available.
It uses [RTIC](https://rtic.rs/2/book/en/) as the concurrency framework and the
[defmt](https://defmt.ferrous-systems.com/) framework for logging.

The STM32H753ZIT device was picked because it is one of the more powerful Cortex-M based STM32
devices. It has more RAM available and allows commanding via Ethernet. The example is written for
the NUCLEO-H753ZI board, which uses the MB1364 Nucleo-144 board layout.

## Pre-Requisites

Make sure the following tools are installed:

1. [`probe-rs`](https://probe.rs/): Application used to flash and debug the MCU.
2. Optional and recommended: [VS Code](https://code.visualstudio.com/) with
   [probe-rs plugin](https://marketplace.visualstudio.com/items?itemName=probe-rs.probe-rs-debugger)
   for debugging.

## Preparing Rust and the repository

Building an application requires the `thumbv7em-none-eabihf` cross-compiler toolchain.
If you have not installed it yet, you can do so with

```sh
rustup target add thumbv7em-none-eabihf
```

A default `.cargo` config file is provided as `.cargo/config.toml.template`. The build script
copies it to `.cargo/config.toml` if that file does not exist yet. The copy is not tracked by git,
so you can change settings like the runner for your setup.

Cargo reads the configuration before the build script runs, so the very first build on a fresh
checkout does not use it yet and might fail. Simply run the build again, or copy the file
manually beforehand:

```sh
cp .cargo/config.toml.template .cargo/config.toml
```

The configuration file also sets the target so it does not always have to be specified with
the `--target` argument.

## Building

After that, assuming that you have a `.cargo/config.toml` setting the correct build target,
you can simply build the application with

```sh
cargo build
```

## Flashing from the command line

You can flash the application from the command line using `probe-rs`:

```sh
probe-rs run --chip STM32H753ZITx
```

## Debugging with VS Code

The Nucleo board comes with an on-board ST-Link so all that is required to flash and debug
the board is a USB cable. The code in this repository was debugged using [`probe-rs`](https://probe.rs/docs/tools/debuggerA)
and the VS Code [`probe-rs` plugin](https://marketplace.visualstudio.com/items?itemName=probe-rs.probe-rs-debugger).
Make sure to install this plugin first.

Sample configuration files are provided inside the `vscode` folder.
Use `cp vscode .vscode -r` to use them for your project.

Some sample configuration files for VS Code were provided as well. You can simply use `Run` and `Debug`
to automatically rebuild and flash your application.

The `tasks.json` and `launch.json` files are generic and you can use them immediately by opening
the folder in VS code or adding it to a workspace.

## Commanding the board

The board is commanded via UDP on port 7301. It gets its IP address via DHCP, so it needs to be
connected to a network with a DHCP server. The network configuration including the IP address is
logged after startup. According to the board user manual UM2407, jumper JP6 and solder bridge SB72
must be ON when using Ethernet. The telecommands are CCSDS space packets with a
[`postcard`](https://docs.rs/postcard) serialized payload.

The [`embedded-client`](../embedded-client) application is used to command the board. Set the
address of the board inside `embedded-client/config.toml`, for example:

```toml
[interface]
udp_addr = "192.168.1.50:7301"
```

Then run the client from inside the `embedded-client` directory. For example, you can send a ping
to the MCU using

```sh
cargo run --bin stm32h7-client -- --ping
```

and set the LED blink frequency to 500 ms using

```sh
cargo run --bin stm32h7-client -- --set-led-frequency 500
```

You can also pass the board address with `--udp-addr` instead of setting it inside the
configuration file.

## Resources

- [STM32H743ZI Ethernet link checker example](https://github.com/stm32-rs/stm32h7xx-hal/blob/master/examples/ethernet-nucleo-h743zi2.rs)
- [smoltcp DHCP client](https://github.com/smoltcp-rs/smoltcp/blob/main/examples/dhcp_client.rs)
