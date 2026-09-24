use crate::DeviceMode;

/// Commanded magnetic dipole. Simple model using raw values per axis.
#[derive(serde::Serialize, serde::Deserialize, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dipole {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HkSet {
    pub valid: bool,
    pub dipole: Dipole,
    pub torquing: bool,
}

pub mod request {
    use crate::{DeviceMode, HkRequestType, Message};

    use super::Dipole;

    #[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ModeRequest {
        SetMode(DeviceMode),
        ReadMode,
    }

    #[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
    pub enum HealthRequest {
        /// Overrides the device's autonomous FDIR health state, for example to clear a `Faulty`
        /// state set by the handler after ground has fixed or worked around the underlying issue.
        SetHealth(satrs::health::HealthState),
    }

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
    pub enum Request {
        Ping,
        Hk(HkRequestType),
        Mode(ModeRequest),
        Health(HealthRequest),
        /// Only accepted in normal mode.
        ApplyTorque {
            dipole: Dipole,
            duration: core::time::Duration,
        },
    }

    impl Message for Request {
        fn message_type(&self) -> crate::MessageType {
            match self {
                Request::Ping => crate::MessageType::Verification,
                Request::Hk(_) => crate::MessageType::Hk,
                Request::Mode(_) => crate::MessageType::Mode,
                Request::Health(_) => crate::MessageType::Health,
                Request::ApplyTorque { .. } => crate::MessageType::Action,
            }
        }
    }
}

#[derive(strum::EnumDiscriminants, serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
#[strum_discriminants(derive(num_enum::IntoPrimitive))]
#[repr(u16)]
pub enum Event {
    /// The communication fault counter exceeded its threshold. Followed by a recovery event.
    CommFaultThresholdExceeded,
    /// A commanded or autonomous mode transition completed.
    ModeChanged(DeviceMode),
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
    }

    impl Message for Response {
        fn message_type(&self) -> crate::MessageType {
            match self {
                Response::Ok | Response::NotInNormalMode => crate::MessageType::Verification,
                Response::Hk(_) => crate::MessageType::Hk,
                Response::Mode(_) => crate::MessageType::Mode,
            }
        }
    }
}
