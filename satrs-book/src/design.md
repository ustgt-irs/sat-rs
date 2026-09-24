# Library Design

Satellites and space systems in general are complex systems with a wide range of requirements for
both the hardware and the software. Consequently, the general design of the library is centered
around many light-weight components and a toolbox principle where you assemble everything
you need instead of plugging something into a larger framework. This approach allows the largest
amount of flexibility, including the operating system and platform choice. For example, `sat-rs`
can be used both in `async` and regular synchronous platforms.

There are still a lot of common patterns and architectures across these systems where guidance
of how to solve a problem and a common structure would still be extremely useful to avoid pitfalls
which were already solved and to avoid boilerplate code. This library tries to provide this
structure and guidance the following way:

1. Providing this book which explains the architecture and design patterns with respect to common
   issues and requirements of space systems.
2. Providing an example application. Space systems still commonly have large monolithic
   primary On-Board Software, so the choice was made to provide one example software which
   contains the various features provided by sat-rs.
3. Providing a good test suite. This includes both unit tests and integration tests. The integration
   tests can also serve as smaller usage examples than the large `satrs-example` application.

This library has special support for standards used in the space industry. The recommended
standards are provided by the Consultative Committee for Space Data Systems (CCSDS):

- The CCSDS Space Packet Protocol as the basic packet format for telecommands and telemetry.
- The CCSDS File Delivery Protocol (CFDP) for file transfers.

The library does not enforce using any of those standards, but it is always recommended to use
some sort of standard for interoperability.

A lot of the modules and design considerations are based on the Flight Software Framework (FSFW).
The FSFW has its own [documentation](https://documentation.irs.uni-stuttgart.de/fsfw/), which
will be referred to when applicable. The FSFW was developed over a period of 10 years for the
Flying Laptop Project by the University of Stuttgart with Airbus Defence and Space GmbH.
It has flight heritage through the 2 missions [FLP](https://www.irs.uni-stuttgart.de/en/research/satellitetechnology-and-instruments/smallsatelliteprogram/flying-laptop/)
and [EIVE](https://www.irs.uni-stuttgart.de/en/research/satellitetechnology-and-instruments/smallsatelliteprogram/EIVE/).
Therefore, a lot of the design concepts were ported more or less unchanged to the `sat-rs`
library.
FLP is a medium-size small satellite with a higher budget and longer development time than EIVE,
which allowed building a highly reliable system while EIVE is a smaller 6U+ cubesat which had a
shorter development cycle and was built using cheaper COTS components. This library also tries
to accumulate the knowledge of developing the OBSW and operating the satellite for both these
different systems and provide a solution for a wider range of small satellite systems.

`sat-rs` can be seen as a modern port of the FSFW which uses common principles of software
engineering to provide a reliable and robust basis for space On-Board Software. The choice
of using the Rust programming language was made for the following reasons:

1. Rust has safety guarantees which are a perfect fit for space systems which generally have high
   robustness and reliability guarantees.
2. Rust is suitable for embedded systems. It can also be run on smaller embedded systems like the
   STM32 which have also become common in the space sector. All space systems are embedded systems,
   which makes using large languages like Python challenging even for OBCs with more performance.
3. Rust has support for linking C APIs through its excellent FFI support. This is especially
   important because many vendor provided libraries are still C based.
4. Modern tooling like a package manager and various development helpers, which can further reduce
   development cycles for space systems. `cargo` provides tools like auto-formatters and linters
   which can immediately ensure a high software quality throughout each development cycle.
5. A large ecosystem with excellent libraries which also leverages the excellent tooling provided
   previously. Integrating these libraries is a lot easier compared to languages like C/C++ where
   there is still no standardized way to use packages.
