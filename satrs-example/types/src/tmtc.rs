use crate::{ComponentId, Message};

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
pub enum Event {
    /// A received CCSDS packet failed CRC or basic format validation.
    InvalidTcPacket,
    /// The CCSDS packet was valid, but its embedded TC header could not be decoded.
    InvalidTcHeader,
    /// The TC header decoded fine, but no handler is registered for its target ID.
    UnknownTargetId(ComponentId),
}

impl Message for Event {
    fn message_type(&self) -> crate::MessageType {
        crate::MessageType::Event
    }
}
