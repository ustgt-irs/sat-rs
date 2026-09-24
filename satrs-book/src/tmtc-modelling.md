# TMTC modelling using Rust

Before we talk about how to model telecommand and telemetry data using Rust, we are going
to present some basic concepts and useful libraries first.

## Serialization

Serialization and deserialization is the process of converting (Rust) data structures into
some format which can be stored or transmitted. We can use this system for generating the payload
of our telecommand and telemetry packets. This allows us to model our payloads with Rust data
structures, fits perfectly into the data-driven approach that Rust programs tend to favor and
allows us to use the excellent type system.

The Rust ecosystem provides the [`serde`](https://serde.rs/) library for this task. The library
makes it trivial to add serialization support to custom data structures by providing a
[`derive`](https://serde.rs/derive.html) macro. In almost all cases, you can just add this derive
macro to a data structure to make it serializable with any `serde` compatible serializer.

There are various serializers available which are well suited to the requirements of space systems.

- Generally, we try to minimize the payload size to save data bandwidth.
- The data does not necessarily have to be human-readable

We recommend the [`postcard`](https://github.com/jamesmunns/postcard) serializer, which fulfills
these requirements and also works well for embedded systems.

## Modelling telecommands and telemetry

Using a serializer library like `serde` allows us to do some interesting things. For example,
let's assume you have a `Camera` object in software that you want to send some commands to.
This object should have the following capabilities:

- Process a ping command
- Capture an image
- Send back configuration data

You can now model a request to your `Camera` object using the following data structure

```rust
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum CameraRequest {
    Ping,
    CaptureImage,
    RequestConfig,
}
```

This data structure models all the requests that the `Camera` provides.
On the telemetry side, you would have a similar object

```rust
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub enum CameraResponse {
    Ok,
    Config(ConfigStructure)
}
```

where `ConfigStructure` would be some other wrapped configuration structure, and the `Ok` response
would be the reply for successful execution for all other commands which do not have additional
telemetry information.

Rust makes it trivial to move components into a new shared library. You can now put these data
structures in a shared `types` or `data` library which can be re-used by both a ground system
library and by the on-board software.

On the ground system, you could use a function like [`postcard::to_allocvec`](https://docs.rs/postcard/latest/postcard/fn.to_allocvec.html)
to generate the byte representation of a `CameraRequest`, which is then sent as the payload
inside a CCSDS space packet. On the on-board software side, you can use
[`postcard::from_bytes`](https://docs.rs/postcard/latest/postcard/fn.from_bytes.html) to deserialize
the `CameraRequest` from the raw payload bytes. In both cases, you do not need to hand-write
the serialization and deserialization code anymore. The only trade-off is that you need a Rust
conversion layer if you want to create your telecommands in another language like Python.

Using Rust structures like this also has other advantages. Once you have the `CameraRequest`
structure, you can `match` on it to cover **all** commands that the device handler needs to cover.
If you add a new variant, you have to handle it as well and you can not forget to handle a
variant.

One trade-off to keep in mind is that a Rust `enum` will always have the size of its largest variant
in memory. If you need to send large payloads to and from the on-board software, you can also
add this data as a secondary data blob behind the primary `serde` payload, and still send something
like small metadata as part of the payload. `postcard` can tell you the size of the deserialized
payload which helps with determining the size of any additional payload data.

We recommend this approach for all TMTC definitions where you control all sides of the communication.
