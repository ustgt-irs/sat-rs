sat-rs examples for the STM32H753ZI-Nucleo board
=======

These example applications show how the [sat-rs library](https://egit.irs.uni-stuttgart.de/rust/sat-rs)
can be used on an embedded target.
They also show how a relatively simple OBSW could be built when no standard runtime is available.
Both use the [defmt](https://defmt.ferrous-systems.com/) framework for logging and provide the
same functionality with a different concurrency framework:

- [`stm32h7-nucleo-embassy`](./stm32h7-nucleo-embassy) uses the [embassy](https://embassy.dev/)
  executor.
- [`stm32h7-nucleo-rtic`](./stm32h7-nucleo-rtic) uses [RTIC](https://rtic.rs/2/book/en/).

The application code is shared inside the [`shared-embedded`](./shared-embedded) crate.
Each application only initializes the hardware and wraps the shared code inside its own tasks.

The STM32H753ZIT device was picked because it is one of the more powerful Cortex-M based STM32
devices. It has more RAM available and allows commanding via Ethernet. The examples are written
for the NUCLEO-H753ZI board, which uses the MB1364 Nucleo-144 board layout.

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

All crates in this directory form a separate workspace, because they are built for a different
target than the rest of the repository. They share one `.cargo` config file. A default is
provided as `.cargo/config.toml.template`. The build scripts copy it to `.cargo/config.toml`
if that file does not exist yet. The copy is not tracked by git, so you can change settings like
the runner for your setup.

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
you can build all applications from this directory with

```sh
cargo build
```

or a single application with `cargo build -p stm32h7-nucleo-embassy`, for example.

## Flashing from the command line

The configuration file sets `probe-rs` as the runner, so you can flash and run an application
with

```sh
cargo run -p stm32h7-nucleo-embassy
```

## Debugging with VS Code

The Nucleo board comes with an on-board ST-Link so all that is required to flash and debug
the board is a USB cable. The code in this repository was debugged using [`probe-rs`](https://probe.rs/docs/tools/debuggerA)
and the VS Code [`probe-rs` plugin](https://marketplace.visualstudio.com/items?itemName=probe-rs.probe-rs-debugger).
Make sure to install this plugin first.

## Commanding the board

The board is commanded via UDP on port 7301. It gets its IP address via DHCP, so it needs to be
connected to a network with a DHCP server. The network configuration including the IP address is
logged after startup. According to the board user manual UM2407, jumper JP6 and solder bridge SB72
must be ON when using Ethernet. The board uses the same TMTC protocol as the
[`example-std`](../example-std) application, which is defined inside the [`types`](../types)
crate.

The [`client`](../client) application is used to command the board. Set the address of the board
inside `client/config.toml`, which is created from `client/config.toml.template` on the first
build:

```toml
[interface]
udp_addr = "192.168.1.50:7301"
```

For example, you can then send a ping to the MCU using

```sh
cargo run -p client -- --ping
```

Like the `example-std` application, the board has a controller component which handles pings and
test events. A test event can be triggered with `--test-event`.

The green LED blinks every 0.5 seconds as a heartbeat. The red and the orange LED are controlled
with a mode. For example, you can let both toggle together every 200 ms using

```sh
cargo run -p client -- led --mode unified-toggle --toggle-period-ms 200
```

Use `cargo run -p client -- led --help` to list all modes.

You can also pass the board address with `--udp-addr` instead of setting it inside the
configuration file.

## Connecting to the mini simulator

The firmware can connect to the [`minisim`](../minisim), which simulates the devices of the OBSW.
The simulator address can be passed with a sim connect request:

```sh
cargo run -p client -- sim connect
```

Without an IP address, the firmware uses the sender address of the request, which fits the
common setup where the client and the simulator run on the same host. Otherwise, pass the IP
address of the simulator host, for example `sim connect 192.168.1.10`.

The simulator address can also be set at build time inside `.cargo/config.toml`, so the firmware
connects on its own after startup:

```toml
[env]
SIM_IP_ADDR = "192.168.1.10"
```

The firmware pings the simulator on UDP port 7303 and logs the result. It reconnects after a
network link loss. A new sim connect request, for example after restarting the simulator,
triggers a new connection attempt.
Like the `example-std` application, the device handlers use dummy interfaces if no simulator
address is known or the simulator does not reply. The simulator sends its replies to the
last client which contacted it, so only one application can use it at a time.

## Resources

- [STM32H743ZI Ethernet link checker example](https://github.com/stm32-rs/stm32h7xx-hal/blob/master/examples/ethernet-nucleo-h743zi2.rs)
- [smoltcp DHCP client](https://github.com/smoltcp-rs/smoltcp/blob/main/examples/dhcp_client.rs)
