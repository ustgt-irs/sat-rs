use core::str::FromStr;

use num_enum::TryFromPrimitive as _;
use satrs::mode::ModeRaw;

use crate::DeviceMode;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The assembly mode ressembles the modes of the devices it controls. It also tries to keep
    /// the children in the correct mode by re-commanding them into the correct mode.
    Device(DeviceMode),
    /// Mode keeping disabled.
    NoModeKeeping,
}

impl From<Mode> for ModeRaw {
    fn from(value: Mode) -> Self {
        match value {
            Mode::Device(device_mode) => device_mode.into(),
            Mode::NoModeKeeping => 5,
        }
    }
}

impl TryFrom<ModeRaw> for Mode {
    type Error = ();

    fn try_from(value: ModeRaw) -> Result<Self, Self::Error> {
        match DeviceMode::try_from_primitive(value) {
            Ok(val) => Ok(Mode::Device(val)),
            Err(_) => {
                if value == 5 {
                    Ok(Mode::NoModeKeeping)
                } else {
                    Err(())
                }
            }
        }
    }
}

impl FromStr for Mode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" => Ok(Mode::Device(DeviceMode::Off)),
            "on" => Ok(Mode::Device(DeviceMode::On)),
            "normal" => Ok(Mode::Device(DeviceMode::Normal)),
            "no_mode_keeping" => Ok(Mode::NoModeKeeping),
            _ => Err(()),
        }
    }
}

pub mod request {
    use crate::{HkRequestType, Message, acs::mgm_assembly::Mode};

    #[derive(Debug, PartialEq, Eq, Clone, Copy, serde::Serialize, serde::Deserialize)]
    pub enum HkId {
        Sensor,
    }

    #[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum ModeRequest {
        SetMode(Mode),
        ReadMode,
    }

    #[derive(Debug, PartialEq, Eq, Clone, Copy, serde::Serialize, serde::Deserialize)]
    pub struct HkRequest {
        pub id: HkId,
        pub req_type: HkRequestType,
    }

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
    pub enum Request {
        Ping,
        Mode(ModeRequest),
    }

    impl Request {
        fn message_type(&self) -> crate::MessageType {
            match self {
                Request::Ping => crate::MessageType::Verification,
                Request::Mode(_mode) => crate::MessageType::Mode,
            }
        }
    }
    impl Message for Request {
        fn message_type(&self) -> crate::MessageType {
            self.message_type()
        }
    }
}

pub mod response {
    use crate::{DeviceMode, Message};

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ModeCommandFailure {
        Timeout,
    }

    #[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub enum ModeResponse {
        /// Mode of the assembly.
        Mode(super::Mode),
        /// Timeout failure setting the children modes.
        SetModeTimeout([Option<DeviceMode>; 2]),
        /// Children are in wrong mode after commanding.
        WrongMode([Option<DeviceMode>; 2]),
        /// An assembly tried modekeeping but can not keep its mode.
        CanNotKeepMode([Option<DeviceMode>; 2]),
    }

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Response {
        Ok,
        Mode(ModeResponse),
    }

    impl Response {
        fn message_type(&self) -> crate::MessageType {
            match self {
                Response::Ok => crate::MessageType::Verification,
                Response::Mode(_mode_report) => crate::MessageType::Mode,
            }
        }
    }

    impl Message for Response {
        fn message_type(&self) -> crate::MessageType {
            self.message_type()
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
pub enum Event {
    /// A commanded or autonomous mode transition completed.
    ModeChanged(Mode),
}

impl crate::Message for Event {
    fn message_type(&self) -> crate::MessageType {
        crate::MessageType::Event
    }
}
