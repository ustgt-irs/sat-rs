pub mod request {
    use crate::ComponentId;

    /// Controls which events are converted to TM. Component and event filters are independent:
    /// re-enabling a component does not re-enable its individually disabled events.
    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Request {
        EnableComponent(ComponentId),
        DisableComponent(ComponentId),
        EnableEvent {
            sender_id: ComponentId,
            event_id: u16,
        },
        DisableEvent {
            sender_id: ComponentId,
            event_id: u16,
        },
    }
}

pub mod response {
    use crate::Message;

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Response {
        Ok,
    }

    impl Message for Response {
        fn message_type(&self) -> crate::MessageType {
            crate::MessageType::Verification
        }
    }
}
