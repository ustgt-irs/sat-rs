#![no_std]

use serde::{Deserialize, Serialize};
use tai_time::MonotonicTime;

use crate::{
    acs::{mgm, mgt},
    eps::{PcduReply, PcduRequest},
};

/// Used by clients to route replies to the component handling them.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum ComponentId {
    SimCtrl,
    Mgm0Lis3Mdl,
    Mgm1Lis3Mdl,
    Mgt,
    Pcdu,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SimRequest {
    SimCtrl(SimCtrlRequest),
    Mgm { id: mgm::Id, request: mgm::Request },
    Mgt(mgt::Request),
    Pcdu(PcduRequest),
}

impl From<SimCtrlRequest> for SimRequest {
    fn from(request: SimCtrlRequest) -> Self {
        Self::SimCtrl(request)
    }
}

impl From<mgt::Request> for SimRequest {
    fn from(request: mgt::Request) -> Self {
        Self::Mgt(request)
    }
}

impl From<PcduRequest> for SimRequest {
    fn from(request: PcduRequest) -> Self {
        Self::Pcdu(request)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimRequestWithTime {
    pub request: SimRequest,
    pub timestamp: MonotonicTime,
}

impl SimRequestWithTime {
    pub fn new(request: impl Into<SimRequest>, timestamp: MonotonicTime) -> Self {
        Self {
            request: request.into(),
            timestamp,
        }
    }

    pub fn new_with_epoch_time(request: impl Into<SimRequest>) -> Self {
        Self::new(request, MonotonicTime::EPOCH)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SimReply {
    SimCtrl(SimCtrlReply),
    Mgm { id: mgm::Id, reply: mgm::Reply },
    Mgt(mgt::Reply),
    Pcdu(PcduReply),
}

impl SimReply {
    pub fn component(&self) -> ComponentId {
        match self {
            SimReply::SimCtrl(_) => ComponentId::SimCtrl,
            SimReply::Mgm { id, .. } => id.sim_component(),
            SimReply::Mgt(_) => ComponentId::Mgt,
            SimReply::Pcdu(_) => ComponentId::Pcdu,
        }
    }
}

impl From<SimCtrlReply> for SimReply {
    fn from(reply: SimCtrlReply) -> Self {
        Self::SimCtrl(reply)
    }
}

impl From<mgt::Reply> for SimReply {
    fn from(reply: mgt::Reply) -> Self {
        Self::Mgt(reply)
    }
}

impl From<PcduReply> for SimReply {
    fn from(reply: PcduReply) -> Self {
        Self::Pcdu(reply)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimCtrlRequest {
    Ping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimCtrlReply {
    Pong,
}

pub mod eps {
    use super::*;
    use types::pcdu::{SwitchId, SwitchMapBinary, SwitchStateBinary};

    #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub enum PcduRequest {
        SwitchDevice {
            switch: SwitchId,
            state: SwitchStateBinary,
        },
        RequestSwitchInfo,
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    pub enum PcduReply {
        SwitchInfo(SwitchMapBinary),
    }
}

pub mod acs {
    /// MGM module strongly based on the LIS3MDL device.
    pub mod mgm {
        use serde::{Deserialize, Serialize};
        use types::pcdu::SwitchStateBinary;

        use crate::ComponentId;

        /// Fault mode injected on the simulated SPI bus, independent of the switch state.
        ///
        /// Models the classic symptom of a stuck SPI bus: an undriven MISO line commonly reads
        /// back as all-1s, a shorted/grounded one as all-0s.
        #[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
        pub enum SpiFaultMode {
            #[default]
            None,
            AllZeros,
            AllOnes,
        }

        #[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
        pub struct SpiFault {
            pub mode: SpiFaultMode,
            /// The fault is cleared when the device is switched off, so a power cycle recovers
            /// from it.
            pub cleared_by_power_cycle: bool,
        }

        // Normally, small magnetometers generate their output as a signed 16 bit raw format or something
        // similar which needs to be converted to a signed float value with physical units. We will
        // simplify this now and generate the signed float values directly. The unit is micro tesla.
        #[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
        pub struct SensorValuesMicroTesla {
            pub x: f32,
            pub y: f32,
            pub z: f32,
        }

        pub const MGT_GEN_MAGNETIC_FIELD: SensorValuesMicroTesla = SensorValuesMicroTesla {
            x: 30.0,
            y: -30.0,
            z: 30.0,
        };
        pub const ALL_ONES_SENSOR_VAL: i16 = 0xffff_u16 as i16;
        pub const ALL_ZEROS_SENSOR_VAL: i16 = 0;

        // Field data register scaling
        pub const GAUSS_TO_MICROTESLA_FACTOR: u32 = 100;
        pub const FIELD_LSB_PER_GAUSS_4_SENS: f32 = 1.0 / 6842.0;

        #[derive(Default, Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
        pub struct RawValues {
            pub x: i16,
            pub y: i16,
            pub z: i16,
        }
        #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
        pub enum Request {
            RequestSensorData,
            /// Force the raw register reply into a stuck-bus pattern, regardless of switch state.
            /// Used to test FDIR handling of SPI bus faults.
            SetSpiFault(SpiFault),
        }

        #[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
        pub struct Reply {
            pub switch_state: SwitchStateBinary,
            pub sensor_values: SensorValuesMicroTesla,
            // Raw sensor values which are transmitted by the LIS3 device in little-endian
            // order.
            pub raw: RawValues,
        }

        #[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
        pub enum Id {
            Mgm0,
            Mgm1,
        }

        impl Id {
            pub const fn sim_component(&self) -> ComponentId {
                match self {
                    Id::Mgm0 => ComponentId::Mgm0Lis3Mdl,
                    Id::Mgm1 => ComponentId::Mgm1Lis3Mdl,
                }
            }
        }

        impl RawValues {
            pub const fn splat(value: i16) -> Self {
                Self {
                    x: value,
                    y: value,
                    z: value,
                }
            }
        }
    }

    pub mod mgt {
        use core::time::Duration;

        use serde::{Deserialize, Serialize};

        // Simple model using i16 values.
        #[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
        pub struct Dipole {
            pub x: i16,
            pub y: i16,
            pub z: i16,
        }

        #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
        pub enum Request {
            ApplyTorque { duration: Duration, dipole: Dipole },
            RequestHk,
        }

        #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
        pub struct HkSet {
            pub dipole: Dipole,
            pub torquing: bool,
        }

        #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
        pub enum Reply {
            Hk(HkSet),
        }
    }
}

pub mod udp {
    pub const SIM_CTRL_PORT: u16 = 7303;
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;

    #[test]
    fn test_request_serde_roundtrip() {
        let sim_request = SimRequestWithTime::new_with_epoch_time(SimCtrlRequest::Ping);
        let bytes = postcard::to_allocvec(&sim_request).unwrap();
        let deserialized: SimRequestWithTime = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(deserialized, sim_request);
    }

    #[test]
    fn test_reply_serde_roundtrip() {
        let sim_reply = SimReply::from(SimCtrlReply::Pong);
        assert_eq!(sim_reply.component(), ComponentId::SimCtrl);
        let bytes = postcard::to_allocvec(&sim_reply).unwrap();
        let deserialized: SimReply = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(deserialized, sim_reply);
    }
}
