//! LEDs of the embedded examples.
//!
//! One LED blinks periodically as a heartbeat. The red and the orange LED are controlled with
//! the [Mode].

use core::time::Duration;

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    AllOff,
    RedOn,
    OrangeOn,
    /// The red and the orange LED toggle in turns with the given period.
    AlternatingToggle(Duration),
    /// The red and the orange LED toggle together with the given period.
    UnifiedToggle(Duration),
}

pub mod request {
    use crate::{Message, MessageType};

    use super::Mode;

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Request {
        Ping,
        SetMode(Mode),
    }

    impl Message for Request {
        fn message_type(&self) -> MessageType {
            match self {
                Request::Ping => MessageType::Ping,
                Request::SetMode(_) => MessageType::Mode,
            }
        }
    }
}

pub mod response {
    use crate::{Message, MessageType};

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Response {
        Ok,
    }

    impl Message for Response {
        fn message_type(&self) -> MessageType {
            MessageType::Verification
        }
    }
}
