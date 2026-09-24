The sat-rs book
======

This book is the primary information resource for the [sat-rs library](https://egit.irs.uni-stuttgart.de/rust/sat-rs)
in addition to the regular API documentation. It contains the following resources:

1. Architecture information and considerations which would exceed the scope of the regular API.
2. General information on how to build on-board software and how `sat-rs` can help to fulfill
   the unique requirements of writing software for remote systems.

# Introduction

The primary goal of the sat-rs library is to provide re-usable components
to write on-board software for remote systems like rovers or satellites. It is specifically written
for the special requirements for these systems.

Some architecture and general design considerations are based on the
[FSFW](https://egit.irs.uni-stuttgart.de/fsfw/fsfw) C++ framework which has flight heritage
through the 2 missions [FLP](https://www.irs.uni-stuttgart.de/en/research/satellitetechnology-and-instruments/smallsatelliteprogram/flying-laptop/)
and [EIVE](https://www.irs.uni-stuttgart.de/en/research/satellitetechnology-and-instruments/smallsatelliteprogram/EIVE/).

However, `sat-rs` has a significantly reduced scope compared to those frameworks. Rust provides
a great ecosystem and a powerful standard library which reduces the need for large and complex
frameworks.

# Getting started with the example

The [`satrs-example`](https://egit.irs.uni-stuttgart.de/rust/sat-rs/src/branch/main/examples/obsw-std)
provides various practical usage examples of the `sat-rs` framework. If you are more interested in
the practical application of `sat-rs` inside an application, it is recommended to have a look at
the example application. The [`satrs-minisim`](https://egit.irs.uni-stuttgart.de/rust/sat-rs/src/branch/main/examples/minisim)
application complements the example application and can be used to simulate some physical devices
for the `satrs-example` device handlers.

# Flight Heritage

There is an active and continuous effort to get early flight heritage for the sat-rs library.
Currently this library has the following flight heritage:

- Submission as an [OPS-SAT experiment](https://www.esa.int/Enabling_Support/Operations/OPS-SAT)
  which has also
  [flown on the satellite](https://blogs.esa.int/rocketscience/2024/05/21/ops-sat-reentry-tomorrow-final-experiments-continue/).
  The application is strongly based on the sat-rs example application. You can find the repository
  of the experiment [here](https://egit.irs.uni-stuttgart.de/rust/ops-sat-rs).
- Development and use of a sat-rs-based [demonstration on-board software](https://egit.irs.uni-stuttgart.de/rust/eurosim-obsw)
  alongside a Flight System Simulator in the context of a
  [Bachelor's thesis](https://www.researchgate.net/publication/380785984_Design_and_Development_of_a_Hardware-in-the-Loop_EuroSim_Demonstrator)
  at [Airbus Netherlands](https://www.airbusdefenceandspacenetherlands.nl/).
