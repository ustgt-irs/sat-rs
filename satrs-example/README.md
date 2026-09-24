sat-rs example
======

This crate contains an example application which simulates an on-board software.
It uses various components provided by the sat-rs framework to do this. As such, it shows how
a more complex real on-board software could be built from these components. It is recommended to
read the dedicated
[example chapters](https://documentation.irs.uni-stuttgart.de/projects/sat-rs/book/example.html) inside
the sat-rs book.

The application opens a UDP and a TCP server on port 7301 to receive telecommands.

You can run the application using `cargo run`.

# Features

The example has the `heap_tmtc` feature which is enabled by default. With this feature enabled,
TMTC packets are exchanged using the heap as the backing memory instead of pre-allocated static
stores.

You can run the application without this feature using

```sh
cargo run --no-default-features
```

# Interacting with the sat-rs example

The `client` crate is a command line client which sends telecommands to the example application
and prints the received telemetry. For example, you can ping the application or switch MGM 0
to normal mode like this:

```sh
cargo run -p client -- --ping
cargo run -p client -- mgm0 -m normal
```

Use `cargo run -p client -- --help` to list all available commands.

## Adding the mini simulator application

This example application features a few device handlers. The
[`satrs-minisim`](https://egit.irs.uni-stuttgart.de/rust/sat-rs/src/branch/main/satrs-example/minisim)
can be used to simulate the physical devices managed by these device handlers.

The example application will attempt communication with the mini simulator on UDP port 7303.
If this works, the device handlers will use communication interfaces dedicated to the communication
with the mini simulator. Otherwise, they will be replaced by dummy interfaces which either
return constant values or behave like ideal devices.

In summary, you can use the following command command to run the mini-simulator first:

```sh
cargo run -p satrs-minisim
```

and then start the example using `cargo run -p satrs-example`.
