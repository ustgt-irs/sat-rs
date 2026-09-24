#![no_std]
extern crate alloc;

use alloc::vec::Vec;
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
    Mgm {
        id: mgm::Id,
        request: mgm::Request,
    },
    /// Raw frame of the MGT serial protocol.
    Mgt(Vec<u8>),
    Pcdu(PcduRequest),
}

impl From<SimCtrlRequest> for SimRequest {
    fn from(request: SimCtrlRequest) -> Self {
        Self::SimCtrl(request)
    }
}

impl From<mgt::Request> for SimRequest {
    fn from(request: mgt::Request) -> Self {
        Self::Mgt(request.to_frame())
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
    Mgm {
        id: mgm::Id,
        reply: mgm::Reply,
    },
    /// Raw frame of the MGT serial protocol.
    Mgt(Vec<u8>),
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
        Self::Mgt(reply.to_frame())
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

    /// Simple serial protocol of the magnetorquer.
    ///
    /// The first byte of each frame is the packet ID. The high bit of the ID is set for replies.
    /// All fields are big endian. Every command is answered with exactly one reply, but only
    /// if the device is powered. The device drops invalid frames.
    ///
    /// A data link layer is deliberately skipped for simplicity. A real serial link would need
    /// framing and error detection, for example COBS encoding and a CRC. Here, the transport
    /// always delivers complete and intact frames.
    pub mod mgt {
        use alloc::{vec, vec::Vec};
        use core::time::Duration;

        use num_enum::{IntoPrimitive, TryFromPrimitive};
        pub use types::acs::mgt::Dipole;

        #[derive(Debug, Copy, Clone, PartialEq, Eq, TryFromPrimitive, IntoPrimitive)]
        #[repr(u8)]
        pub enum RequestId {
            RequestHk = 0x01,
            /// Payload: dipole (3 x i16), duration in milliseconds (u32).
            ApplyTorque = 0x02,
        }

        #[derive(Debug, Copy, Clone, PartialEq, Eq, TryFromPrimitive, IntoPrimitive)]
        #[repr(u8)]
        pub enum ReplyId {
            /// Payload: dipole (3 x i16), torquing flag (u8).
            Hk = 0x81,
            /// Reply to [RequestId::ApplyTorque].
            Ack = 0x82,
        }

        #[derive(Debug, Copy, Clone, PartialEq, Eq, thiserror::Error)]
        pub enum FrameError {
            #[error("empty frame")]
            Empty,
            #[error("unknown packet ID {0:#04x}")]
            UnknownPacketId(u8),
            #[error("invalid length {len} for packet ID {id:#04x}")]
            InvalidLength { id: u8, len: usize },
        }

        fn split_packet_id(frame: &[u8]) -> Result<(u8, &[u8]), FrameError> {
            let (&id, payload) = frame.split_first().ok_or(FrameError::Empty)?;
            Ok((id, payload))
        }

        fn payload_array<const N: usize>(id: u8, payload: &[u8]) -> Result<&[u8; N], FrameError> {
            payload.try_into().map_err(|_| FrameError::InvalidLength {
                id,
                len: payload.len() + 1,
            })
        }

        #[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub enum Request {
            /// The duration has millisecond resolution on the wire.
            ApplyTorque {
                duration: Duration,
                dipole: Dipole,
            },
            RequestHk,
        }

        impl Request {
            pub fn to_frame(&self) -> Vec<u8> {
                match self {
                    Request::RequestHk => vec![RequestId::RequestHk.into()],
                    Request::ApplyTorque { duration, dipole } => {
                        let mut frame = vec![RequestId::ApplyTorque.into()];
                        frame.extend_from_slice(&dipole.to_be_bytes());
                        let duration_ms = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
                        frame.extend_from_slice(&duration_ms.to_be_bytes());
                        frame
                    }
                }
            }

            pub fn from_frame(frame: &[u8]) -> Result<Self, FrameError> {
                let (id, payload) = split_packet_id(frame)?;
                match RequestId::try_from(id).map_err(|_| FrameError::UnknownPacketId(id))? {
                    RequestId::RequestHk => {
                        payload_array::<0>(id, payload)?;
                        Ok(Request::RequestHk)
                    }
                    RequestId::ApplyTorque => {
                        let payload: &[u8; Dipole::LEN + 4] = payload_array(id, payload)?;
                        let [dipole @ .., d0, d1, d2, d3] = *payload;
                        Ok(Request::ApplyTorque {
                            duration: Duration::from_millis(
                                u32::from_be_bytes([d0, d1, d2, d3]).into(),
                            ),
                            dipole: Dipole::from_be_bytes(&dipole),
                        })
                    }
                }
            }
        }

        #[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub struct HkSet {
            pub dipole: Dipole,
            pub torquing: bool,
        }

        #[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        pub enum Reply {
            Hk(HkSet),
            Ack,
        }

        impl Reply {
            pub fn to_frame(&self) -> Vec<u8> {
                match self {
                    Reply::Hk(hk) => {
                        let mut frame = vec![ReplyId::Hk.into()];
                        frame.extend_from_slice(&hk.dipole.to_be_bytes());
                        frame.push(hk.torquing as u8);
                        frame
                    }
                    Reply::Ack => vec![ReplyId::Ack.into()],
                }
            }

            pub fn from_frame(frame: &[u8]) -> Result<Self, FrameError> {
                let (id, payload) = split_packet_id(frame)?;
                match ReplyId::try_from(id).map_err(|_| FrameError::UnknownPacketId(id))? {
                    ReplyId::Hk => {
                        let payload: &[u8; Dipole::LEN + 1] = payload_array(id, payload)?;
                        let [dipole @ .., torquing] = *payload;
                        Ok(Reply::Hk(HkSet {
                            dipole: Dipole::from_be_bytes(&dipole),
                            torquing: torquing != 0,
                        }))
                    }
                    ReplyId::Ack => {
                        payload_array::<0>(id, payload)?;
                        Ok(Reply::Ack)
                    }
                }
            }
        }

        #[cfg(test)]
        mod tests {
            use super::*;

            #[test]
            fn test_apply_torque_frame() {
                let request = Request::ApplyTorque {
                    duration: Duration::from_millis(0x0102_0304),
                    dipole: Dipole {
                        x: -2,
                        y: 0x0506,
                        z: 0x0708,
                    },
                };
                let frame = request.to_frame();
                assert_eq!(
                    frame,
                    [
                        0x02, 0xff, 0xfe, 0x05, 0x06, 0x07, 0x08, 0x01, 0x02, 0x03, 0x04
                    ]
                );
                assert_eq!(Request::from_frame(&frame), Ok(request));
            }

            #[test]
            fn test_request_hk_frame() {
                assert_eq!(Request::RequestHk.to_frame(), [0x01]);
                assert_eq!(Request::from_frame(&[0x01]), Ok(Request::RequestHk));
            }

            #[test]
            fn test_reply_frames() {
                let hk = Reply::Hk(HkSet {
                    dipole: Dipole { x: 1, y: 2, z: 3 },
                    torquing: true,
                });
                let frame = hk.to_frame();
                assert_eq!(frame, [0x81, 0, 1, 0, 2, 0, 3, 1]);
                assert_eq!(Reply::from_frame(&frame), Ok(hk));
                assert_eq!(Reply::Ack.to_frame(), [0x82]);
                assert_eq!(Reply::from_frame(&[0x82]), Ok(Reply::Ack));
            }

            #[test]
            fn test_invalid_frames() {
                assert_eq!(Request::from_frame(&[]), Err(FrameError::Empty));
                assert_eq!(
                    Request::from_frame(&[0x81]),
                    Err(FrameError::UnknownPacketId(0x81))
                );
                assert_eq!(
                    Request::from_frame(&[0x01, 0x00]),
                    Err(FrameError::InvalidLength { id: 0x01, len: 2 })
                );
                assert_eq!(
                    Reply::from_frame(&[0x81, 0, 1]),
                    Err(FrameError::InvalidLength { id: 0x81, len: 3 })
                );
            }
        }
    }
}

pub mod udp {
    pub const SIM_CTRL_PORT: u16 = 7303;
}

#[cfg(test)]
mod tests {
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
