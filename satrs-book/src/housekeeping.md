# Housekeeping Data

If you have not read [the TMTC modelling chapter](./tmtc-modelling.md) yet, it is recommended to
do that first.

Remote systems like satellites and rovers oftentimes generate data autonomously and periodically.
An example for this could be temperature or attitude data. Data like this is commonly
referred to as housekeeping data, and is usually one of the most important and most resource heavy
data sources received from a satellite.

First, we are going to list some assumptions and requirements about Housekeeping (HK) data:

1. HK data is generated periodically by various system components throughout the
   system.
2. An autonomous and periodic sampling of that HK data to be stored and sent to Ground is generally
   required. A minimum interface consists of requesting a one-shot sample of HK, enabling and
   disabling the periodic autonomous generation of samples and modifying the collection interval
   of the periodic autonomous generation.
3. HK data often needs to be shared with other software components. For example, a thermal controller
   wants to read the data samples of all sensor components.

## Modelling our data

Generally, it makes sense to model the data with Rust data structures for various reasons. For
example, the sensor data received from a 3-axis magnetometer might be modelled like this:

```rust
#[derive(Debug, Copy, Clone, serde::Serialize, serde::Deserialize)]
pub struct MgmData {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}
```

You can then re-use this data structure for various purposes. Also note the `serde` implementations,
which are useful for generating the housekeeping data sent to ground.

We can model the housekeeping requests for a handler with a single data set like this:

```rust
#[derive(Debug, Copy, Clone, serde::Serialize, serde::Deserialize)]
pub enum HkRequest {
    OneShot,
    EnablePeriodic,
    DisablePeriodic,
    ModifyInterval(core::time::Duration)

}
```

which might then be a part of a top level request type, e.g.

```rust
#[derive(Debug, Copy, Clone, serde::Serialize, serde::Deserialize)]
pub enum Request {
    Ping,
    Hk(HkRequest)
}
```

A corresponding `Response` type might just include a HK data variant:

```rust
#[derive(Debug, Copy, Clone, serde::Serialize, serde::Deserialize)]
pub enum Response {
    Ok,
    Hk(MgmData)
}
```

If the software object managed multiple data sets, you could model it like this:

```rust
/// Example set ID.
#[derive(Debug, Copy, Clone, serde::Serialize, serde::Deserialize)]
pub enum SetId {
    Data,
    Config
}

#[derive(Debug, Copy, Clone, serde::Serialize, serde::Deserialize)]
pub enum Request {
    Ping,
    Hk {
        set_id: SetId,
        request: HkRequest
    }
}
```

Sometimes, you need to share the generated data as well. Furthermore, it might make sense to
decouple the HK generation from the data acquisition and only return the latest snapshot
of the data. In this case, you can put the `MgmData` inside an appropriate lock structure for your
platform/runtime to share it safely with other software components. For example, in a `std` system,
you might simply use an `Arc<Mutex<MgmData>>` or an `Arc<RwLock<MgmData>>` for this.

Now, you can update that shared data structure when acquiring new data, and other software objects
or the HK generation routine can safely read from it.

## Helper components

You need some application logic to track whether periodic data generation is enabled, what
the current generation interval is and whether a HK set needs to be generated if the interval
period has elapsed.

`sat-rs` provides some simple helper components for this inside the [`hk`](https://docs.rs/satrs/latest/satrs/hk/index.html) module. The module documentation contains more information.
