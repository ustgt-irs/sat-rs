use strum::IntoEnumIterator as _;

#[bitbybit::bitfield(u16, debug, default = 0x0)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SwitchesBitfield {
    #[bit(2, rw)]
    magnetorquer: bool,
    #[bit(1, rw)]
    mgm1: bool,
    #[bit(0, rw)]
    mgm0: bool,
}

#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    serde::Serialize,
    serde::Deserialize,
    Hash,
    strum::EnumIter,
    num_enum::IntoPrimitive,
    num_enum::TryFromPrimitive,
)]
#[repr(u16)]
pub enum SwitchId {
    Mgm0 = 0,
    Mgm1 = 1,
    Mgt = 2,
}

#[derive(Debug, Eq, PartialEq, Copy, Clone, serde::Serialize, serde::Deserialize)]
pub enum SwitchState {
    Off = 0,
    On = 1,
    Unknown = 2,
    Faulty = 3,
}

impl From<SwitchStateBinary> for SwitchState {
    fn from(value: SwitchStateBinary) -> Self {
        match value {
            SwitchStateBinary::Off => SwitchState::Off,
            SwitchStateBinary::On => SwitchState::On,
        }
    }
}

#[derive(Debug, Eq, PartialEq, Copy, Clone, serde::Serialize, serde::Deserialize)]
pub enum SwitchStateBinary {
    Off = 0,
    On = 1,
}

#[derive(strum::EnumDiscriminants, serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
#[strum_discriminants(derive(num_enum::IntoPrimitive))]
#[repr(u16)]
pub enum Event {
    /// Sending a request to the PCDU over the serial/simulator link failed.
    SerialCommError,
}

impl crate::Message for Event {
    fn message_type(&self) -> crate::MessageType {
        crate::MessageType::Event
    }
}

impl crate::EventId for Event {
    fn event_id(&self) -> u16 {
        EventDiscriminants::from(self).into()
    }
}

pub const MAX_NUM_OF_SWITCHES: usize = 16;

pub type SwitchMapBinary =
    heapless::index_map::FnvIndexMap<SwitchId, SwitchStateBinary, MAX_NUM_OF_SWITCHES>;

pub struct SwitchMapBinaryWrapper(pub SwitchMapBinary);

impl Default for SwitchMapBinaryWrapper {
    fn default() -> Self {
        let mut switch_map = SwitchMapBinary::default();
        for entry in SwitchId::iter() {
            switch_map
                .insert(entry, SwitchStateBinary::Off)
                .expect("switch map capacity exceeded");
        }
        Self(switch_map)
    }
}

pub struct SwitchRequest {
    pub switch_id: SwitchId,
    pub target_state: SwitchStateBinary,
}

impl SwitchRequest {
    pub fn new(switch_id: SwitchId, target_state: SwitchStateBinary) -> Self {
        Self {
            switch_id,
            target_state,
        }
    }

    pub fn switch_id(&self) -> SwitchId {
        self.switch_id
    }

    pub fn target_state(&self) -> SwitchStateBinary {
        self.target_state
    }
}

pub mod request {
    use crate::{DeviceMode, Message};

    use super::*;

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
    pub enum Request {
        Mode(DeviceMode),
        Ping,
        GetSwitches,
        EnableSwitches(SwitchesBitfield),
        DisableSwitches(SwitchesBitfield),
    }

    impl Request {
        pub fn message_type(&self) -> crate::MessageType {
            match self {
                Request::Mode(_mode) => crate::MessageType::Mode,
                Request::Ping => crate::MessageType::Verification,
                Request::GetSwitches => crate::MessageType::Action,
                Request::EnableSwitches(_switches) | Request::DisableSwitches(_switches) => {
                    crate::MessageType::Action
                }
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
    use super::*;
    use crate::Message;

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
    pub enum Response {
        Ok,
        Switches(SwitchesBitfield),
    }

    impl Message for Response {
        fn message_type(&self) -> crate::MessageType {
            match self {
                Response::Ok => crate::MessageType::Verification,
                Response::Switches(_switches) => crate::MessageType::Action,
            }
        }
    }
}
