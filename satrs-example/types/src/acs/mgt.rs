use crate::DeviceMode;

/// Commanded magnetic dipole. Simple model using raw values per axis.
#[derive(serde::Serialize, serde::Deserialize, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dipole {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}

impl Dipole {
    pub const LEN: usize = 6;

    pub fn to_be_bytes(&self) -> [u8; Self::LEN] {
        let [x0, x1] = self.x.to_be_bytes();
        let [y0, y1] = self.y.to_be_bytes();
        let [z0, z1] = self.z.to_be_bytes();
        [x0, x1, y0, y1, z0, z1]
    }

    pub fn from_be_bytes(bytes: &[u8; Self::LEN]) -> Self {
        let [x0, x1, y0, y1, z0, z1] = *bytes;
        Self {
            x: i16::from_be_bytes([x0, x1]),
            y: i16::from_be_bytes([y0, y1]),
            z: i16::from_be_bytes([z0, z1]),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HkSet {
    pub valid: bool,
    pub dipole: Dipole,
    pub torquing: bool,
}

pub mod request {
    use crate::{DeviceMode, HealthRequest, HkRequestType, Message};

    use super::Dipole;

    #[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ModeRequest {
        SetMode(DeviceMode),
        ReadMode,
    }

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
    pub enum Request {
        Ping,
        Hk(HkRequestType),
        Mode(ModeRequest),
        /// Only accepted in normal mode.
        ApplyTorque {
            dipole: Dipole,
            duration: core::time::Duration,
        },
        Health(HealthRequest),
    }

    impl Message for Request {
        fn message_type(&self) -> crate::MessageType {
            match self {
                Request::Ping => crate::MessageType::Verification,
                Request::Hk(_) => crate::MessageType::Hk,
                Request::Mode(_) => crate::MessageType::Mode,
                Request::ApplyTorque { .. } => crate::MessageType::Action,
                Request::Health(_) => crate::MessageType::Health,
            }
        }
    }
}

#[derive(strum::EnumDiscriminants, serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
#[strum_discriminants(derive(num_enum::IntoPrimitive))]
#[repr(u16)]
pub enum Event {
    /// A commanded mode transition completed.
    ModeChanged(DeviceMode),
    /// Too many requests were not answered correctly. Followed by a recovery event.
    ReplyFaultThresholdExceeded,
    Recovery(satrs::fdir::RecoveryEvent),
}

impl crate::Message for Event {
    fn message_type(&self) -> crate::MessageType {
        crate::MessageType::Event
    }
}

impl crate::EventId for Event {
    fn event_id(&self) -> u16 {
        match self {
            Event::Recovery(event) => crate::recovery_event_id(*event),
            _ => EventDiscriminants::from(self).into(),
        }
    }
}

pub mod response {
    use crate::{DeviceMode, Message};

    use super::HkSet;

    #[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ModeResponse {
        /// New mode has been set.
        Mode(DeviceMode),
        /// Setting a mode timed out.
        SetModeTimeout,
    }

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Response {
        Ok,
        Hk(HkSet),
        Mode(ModeResponse),
        /// The command requires the device to be in normal mode.
        NotInNormalMode,
        /// The device did not answer the command with a valid reply in time.
        ReplyTimeout,
    }

    impl Message for Response {
        fn message_type(&self) -> crate::MessageType {
            match self {
                Response::Ok | Response::NotInNormalMode | Response::ReplyTimeout => {
                    crate::MessageType::Verification
                }
                Response::Hk(_) => crate::MessageType::Hk,
                Response::Mode(_) => crate::MessageType::Mode,
            }
        }
    }
}
