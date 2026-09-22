use crate::{ComponentId, EventId, Message};

#[derive(strum::EnumDiscriminants, serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
#[strum_discriminants(derive(num_enum::IntoPrimitive))]
#[repr(u16)]
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

impl EventId for Event {
    fn event_id(&self) -> u16 {
        EventDiscriminants::from(self).into()
    }
}
