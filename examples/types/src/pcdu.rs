use strum::{EnumCount as _, IntoEnumIterator as _};

#[bitbybit::bitfield(u16, debug, default = 0x0, defmt_bitfields(feature = "defmt"))]
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
    Debug, Copy, Clone, PartialEq, Eq, num_enum::TryFromPrimitive, num_enum::IntoPrimitive,
)]
#[repr(u32)]
pub enum SetId {
    SwitcherSet = 0,
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
    strum::EnumCount,
    num_enum::IntoPrimitive,
    num_enum::TryFromPrimitive,
)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
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
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
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

pub type SwitchMapBinary = heapless::LinearMap<SwitchId, SwitchStateBinary, { SwitchId::COUNT }>;

#[derive(Debug)]
pub struct SwitchMapBinaryWrapper(pub SwitchMapBinary);

#[cfg(feature = "defmt")]
impl defmt::Format for SwitchMapBinaryWrapper {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "{{");
        for (i, (id, state)) in self.0.iter().enumerate() {
            if i > 0 {
                defmt::write!(fmt, ", ");
            }
            defmt::write!(fmt, "{}: {}", id, state);
        }
        defmt::write!(fmt, "}}");
    }
}

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

pub type SwitchMap = heapless::LinearMap<SwitchId, SwitchState, { SwitchId::COUNT }>;

pub struct SwitchMapWrapper(pub SwitchMap);

impl Default for SwitchMapWrapper {
    fn default() -> Self {
        Self::new_with_all_switches(SwitchState::Unknown)
    }
}

impl SwitchMapWrapper {
    pub fn new_with_init_switches_off() -> Self {
        Self::new_with_all_switches(SwitchState::Off)
    }

    fn new_with_all_switches(state: SwitchState) -> Self {
        let mut switch_map = SwitchMap::default();
        for entry in SwitchId::iter() {
            switch_map
                .insert(entry, state)
                .expect("switch map capacity exceeded");
        }
        Self(switch_map)
    }

    pub fn from_binary_switch_map_ref(switch_map: &SwitchMapBinary) -> Self {
        Self(
            switch_map
                .iter()
                .map(|(key, value)| (*key, SwitchState::from(*value)))
                .collect(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SwitchSet {
    pub valid: bool,
    pub switch_map: SwitchMap,
}

impl SwitchSet {
    pub fn new(switch_map: SwitchMap) -> Self {
        Self {
            valid: true,
            switch_map,
        }
    }

    /// Invalid until the PCDU reported the switch states for the first time.
    pub fn new_with_init_switches_unknown() -> Self {
        Self {
            valid: false,
            switch_map: SwitchMapWrapper::default().0,
        }
    }

    /// Builds the set from a PCDU report, together with the bitfield for TM. Returns [None] if a
    /// switch is missing, so an incomplete report is never used.
    pub fn from_binary_switch_map(
        switch_map: &SwitchMapBinary,
    ) -> Option<(Self, SwitchesBitfield)> {
        let switch_set = Self::new(SwitchMapWrapper::from_binary_switch_map_ref(switch_map).0);
        let switches = switch_set.as_bitfield()?;
        Some((switch_set, switches))
    }

    /// Returns [None] if a switch is missing in the map.
    pub fn as_bitfield(&self) -> Option<SwitchesBitfield> {
        let is_on = |id| {
            self.switch_map
                .get(&id)
                .map(|state| *state == SwitchState::On)
        };
        Some(
            SwitchesBitfield::builder()
                .with_magnetorquer(is_on(SwitchId::Mgt)?)
                .with_mgm1(is_on(SwitchId::Mgm1)?)
                .with_mgm0(is_on(SwitchId::Mgm0)?)
                .build(),
        )
    }

    /// Returns false if the switch is not in the map.
    pub fn set_switch_state(&mut self, switch_id: SwitchId, state: SwitchState) -> bool {
        match self.switch_map.get_mut(&switch_id) {
            Some(entry) => {
                *entry = state;
                true
            }
            None => false,
        }
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
    use crate::Message;

    use super::*;

    #[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug)]
    #[cfg_attr(feature = "defmt", derive(defmt::Format))]
    pub enum Request {
        Ping,
        GetSwitches,
        EnableSwitches(SwitchesBitfield),
        DisableSwitches(SwitchesBitfield),
    }

    impl Request {
        pub fn message_type(&self) -> crate::MessageType {
            match self {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_switch_set_from_binary_switch_map() {
        let mut switch_map = SwitchMapBinaryWrapper::default().0;
        *switch_map.get_mut(&SwitchId::Mgm0).unwrap() = SwitchStateBinary::On;
        let (switch_set, switches) =
            SwitchSet::from_binary_switch_map(&switch_map).expect("switch map is complete");
        assert!(switch_set.valid);
        assert_eq!(
            switch_set.switch_map.get(&SwitchId::Mgm0),
            Some(&SwitchState::On)
        );
        assert!(switches.mgm0());
        assert!(!switches.mgm1());
        assert!(!switches.magnetorquer());
    }

    #[test]
    fn test_switch_set_from_incomplete_binary_switch_map() {
        let mut switch_map = SwitchMapBinaryWrapper::default().0;
        switch_map.remove(&SwitchId::Mgt);
        assert!(SwitchSet::from_binary_switch_map(&switch_map).is_none());
    }
}
