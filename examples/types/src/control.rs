use crate::{EventId, Message};

#[derive(strum::EnumDiscriminants, serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
#[strum_discriminants(derive(num_enum::IntoPrimitive))]
#[repr(u16)]
pub enum Event {
    TestEvent,
}

impl Message for Event {
    fn message_type(&self) -> crate::MessageType {
        crate::MessageType::Event
    }
}

impl EventId for Event {
    fn event_id(&self) -> u16 {
        EventDiscriminants::from(self).into()
    }
}

pub mod request {
    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
    pub enum Request {
        Ping,
        TestEvent,
    }
}

pub mod response {
    use crate::Message;

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
    pub enum Response {
        Ok,
        Event(super::Event),
    }

    impl Message for Response {
        fn message_type(&self) -> crate::MessageType {
            match self {
                Response::Ok => crate::MessageType::Verification,
                Response::Event(_event) => crate::MessageType::Event,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_id() {
        assert_eq!(Event::TestEvent.event_id(), 0);
    }
}
