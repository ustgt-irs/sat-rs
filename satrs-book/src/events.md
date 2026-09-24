# Events

Events are an important mechanism used for remote systems to monitor unexpected
or expected anomalies and events occurring on these systems. 
They can improve the observability of a system significantly and provide a
"paper trail" of what is happening or has happened on a satellite where regular
housekeeping packets might not be sufficient. They can also be used for fault
detection, isolation and recovery (FDIR) purposes. For example, higher level
system objects can listen on certain high criticality events to initiate
custom system responses.


## Event Severity

Generally, it also makes sense to classify events according to a severity system
so operators can quickly judge the importance of an event. `sat-rs` does not
constrain the severity classes or enforce their usage, but a severity
classification like this can make sense:

- INFO
- LOW ERROR
- MEDIUM ERROR
- HIGH ERROR

## Modelling Events with Rust

Usually, events will be associated with certain software objects or handlers.
Oftentimes, developers and operators want to supply parameters or metadata
associated with an event. This can all be done using the Rust `enum` type. 

Let's start with an example: a camera device
handler might have the following events:

- Image taken event
- Communication error event including an error classifier
- Communication timeout event with the configured timeout
- Overheating event

You can model these events using the following data structure, also including
a `severity` method.

```rust
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
pub enum Event {
    ImageTaken,
    CommunicationError(ErrorType),
    CommunicationTimeout(core::time::Duration),
    Overheating
}

impl Event {
    pub fn severity(&self) -> Severity {
        match self {
            Event::ImageTaken => Severity::Info,
            Event::CommunicationError(_) => Severity::Low,
            Event::CommunicationTimeout(_) => Severity::Low,
            Event::Overheating => Severity::High,
        } 
    }
}
```

Depending on the requirements of your system, you might want to filter which
events are packaged and sent as telemetry. This requires an identification
system. A simple scheme would be to add something like this:

```rust
impl Event {
    pub fn id(&self) -> u32 {
        match self {
            Event::ImageTaken => 0,
            Event::CommunicationError(_) => 1,
            Event::CommunicationTimeout(_) => 2,
            Event::Overheating => 3,
        } 
    }
}

```

## Handling events

When an event occurs in the system, you want to trigger the event.
This usually includes sending the event to a centralized event funnel. The funnel
takes care of packing the event into a telemetry packet as well as forwarding
the event to any other objects which are interested in the event. A message
queue system is the best solution for this. For example, on an embedded Linux
system, you might have an event sender handle like this inside your camera
device handler:

```rust
pub struct CameraHandler {
    // (...)
    event_sender: std::sync::mpsc::SyncSender<Event>
}
```
