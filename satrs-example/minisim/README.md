sat-rs minisim
======

This crate contains a mini-simulator based on the open-source discrete-event simulation framework
[nexosim](https://github.com/asynchronics/nexosim).

Right now, this crate is primarily used together with the
[`satrs-example` application](https://egit.irs.uni-stuttgart.de/rust/sat-rs/src/branch/main/satrs-example)
to simulate the devices connected to the example application.

You can simply run this application using

```sh
cargo run
```

or

```sh
cargo run -p satrs-minisim
```

in the workspace. The mini simulator uses the UDP port 7303 to exchange simulation requests and
simulation replies with any other application.

The simulator was designed in a modular way to be scalable and adaptable to other communication
schemes. This might allow it to serve a mini-simulator for other example applications which
still have similar device handlers.

The following graph shows the high-level architecture of the mini-simulator.

<img src="../../images/minisim-arch/minisim-arch.png" alt="Mini simulator architecture" width="500" class="center"/>
