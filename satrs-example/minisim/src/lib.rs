use nexosim::time::MonotonicTime;
use serde::{Deserialize, Serialize};

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

            pub fn from_microtesla(values: SensorValuesMicroTesla) -> Self {
                let to_raw = |microtesla: f32| {
                    (microtesla / (GAUSS_TO_MICROTESLA_FACTOR as f32 * FIELD_LSB_PER_GAUSS_4_SENS))
                        .round() as i16
                };
                Self {
                    x: to_raw(values.x),
                    y: to_raw(values.y),
                    z: to_raw(values.z),
                }
            }
        }

        impl Reply {
            pub fn new(
                switch_state: SwitchStateBinary,
                sensor_values: SensorValuesMicroTesla,
                fault_mode: SpiFaultMode,
            ) -> Self {
                // An injected fault always wins. A switched off device reads back like an
                // undriven bus.
                let raw = match (fault_mode, switch_state) {
                    (SpiFaultMode::AllZeros, _) => RawValues::splat(ALL_ZEROS_SENSOR_VAL),
                    (SpiFaultMode::AllOnes, _) | (SpiFaultMode::None, SwitchStateBinary::Off) => {
                        RawValues::splat(ALL_ONES_SENSOR_VAL)
                    }
                    (SpiFaultMode::None, SwitchStateBinary::On) => {
                        RawValues::from_microtesla(sensor_values)
                    }
                };
                Self {
                    switch_state,
                    sensor_values,
                    raw,
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
        use std::time::Duration;

        use serde::{Deserialize, Serialize};

        pub mod packet_id {
            pub const REQUEST_HK: u8 = 0x01;
            /// Payload: dipole (3 x i16), duration in milliseconds (u32).
            pub const APPLY_TORQUE: u8 = 0x02;
            /// Payload: dipole (3 x i16), torquing flag (u8).
            pub const HK: u8 = 0x81;
            /// Reply to [APPLY_TORQUE].
            pub const ACK: u8 = 0x82;
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

        // Simple model using i16 values.
        #[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
        pub struct Dipole {
            pub x: i16,
            pub y: i16,
            pub z: i16,
        }

        impl Dipole {
            const LEN: usize = 6;

            fn write_to(&self, frame: &mut Vec<u8>) {
                frame.extend_from_slice(&self.x.to_be_bytes());
                frame.extend_from_slice(&self.y.to_be_bytes());
                frame.extend_from_slice(&self.z.to_be_bytes());
            }

            fn read_from(buf: &[u8]) -> Self {
                Self {
                    x: i16::from_be_bytes([buf[0], buf[1]]),
                    y: i16::from_be_bytes([buf[2], buf[3]]),
                    z: i16::from_be_bytes([buf[4], buf[5]]),
                }
            }
        }

        /// Checks the frame length and returns the packet ID and the payload.
        fn split_frame(
            frame: &[u8],
            payload_len: impl Fn(u8) -> Option<usize>,
        ) -> Result<(u8, &[u8]), FrameError> {
            let (&id, payload) = frame.split_first().ok_or(FrameError::Empty)?;
            let expected_len = payload_len(id).ok_or(FrameError::UnknownPacketId(id))?;
            if payload.len() != expected_len {
                return Err(FrameError::InvalidLength {
                    id,
                    len: frame.len(),
                });
            }
            Ok((id, payload))
        }

        #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
                    Request::RequestHk => vec![packet_id::REQUEST_HK],
                    Request::ApplyTorque { duration, dipole } => {
                        let mut frame = vec![packet_id::APPLY_TORQUE];
                        dipole.write_to(&mut frame);
                        let duration_ms = u32::try_from(duration.as_millis()).unwrap_or(u32::MAX);
                        frame.extend_from_slice(&duration_ms.to_be_bytes());
                        frame
                    }
                }
            }

            pub fn from_frame(frame: &[u8]) -> Result<Self, FrameError> {
                let (id, payload) = split_frame(frame, |id| match id {
                    packet_id::REQUEST_HK => Some(0),
                    packet_id::APPLY_TORQUE => Some(Dipole::LEN + 4),
                    _ => None,
                })?;
                Ok(match id {
                    packet_id::REQUEST_HK => Request::RequestHk,
                    _ => {
                        let duration_ms =
                            u32::from_be_bytes(payload[Dipole::LEN..].try_into().unwrap());
                        Request::ApplyTorque {
                            duration: Duration::from_millis(duration_ms.into()),
                            dipole: Dipole::read_from(payload),
                        }
                    }
                })
            }
        }

        #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
        pub struct HkSet {
            pub dipole: Dipole,
            pub torquing: bool,
        }

        #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
        pub enum Reply {
            Hk(HkSet),
            Ack,
        }

        impl Reply {
            pub fn to_frame(&self) -> Vec<u8> {
                match self {
                    Reply::Hk(hk) => {
                        let mut frame = vec![packet_id::HK];
                        hk.dipole.write_to(&mut frame);
                        frame.push(hk.torquing as u8);
                        frame
                    }
                    Reply::Ack => vec![packet_id::ACK],
                }
            }

            pub fn from_frame(frame: &[u8]) -> Result<Self, FrameError> {
                let (id, payload) = split_frame(frame, |id| match id {
                    packet_id::HK => Some(Dipole::LEN + 1),
                    packet_id::ACK => Some(0),
                    _ => None,
                })?;
                Ok(match id {
                    packet_id::ACK => Reply::Ack,
                    _ => Reply::Hk(HkSet {
                        dipole: Dipole::read_from(payload),
                        torquing: payload[Dipole::LEN] != 0,
                    }),
                })
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
                    [0x02, 0xff, 0xfe, 0x05, 0x06, 0x07, 0x08, 0x01, 0x02, 0x03, 0x04]
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
pub mod tests {
    use super::*;

    #[test]
    fn test_request_serde_roundtrip() {
        let sim_request = SimRequestWithTime::new_with_epoch_time(SimCtrlRequest::Ping);
        let json = serde_json::to_string(&sim_request).unwrap();
        let deserialized: SimRequestWithTime = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, sim_request);
    }

    #[test]
    fn test_reply_serde_roundtrip() {
        let sim_reply = SimReply::from(SimCtrlReply::Pong);
        assert_eq!(sim_reply.component(), ComponentId::SimCtrl);
        let json = serde_json::to_string(&sim_reply).unwrap();
        let deserialized: SimReply = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, sim_reply);
    }
}
