extern crate alloc;
use core::str::FromStr;

use spacepackets::{
    CcsdsPacketIdAndPsc,
    time::cds::{CdsTime, MIN_CDS_FIELD_LEN},
};

pub mod acs;
pub mod ccsds;
pub mod control;
pub mod pcdu;
pub mod tmtc;

#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    num_enum::TryFromPrimitive,
    num_enum::IntoPrimitive,
)]
#[repr(u32)]
pub enum ComponentId {
    Controller,

    AcsSubsystem,
    AcsMgmAssembly,
    AcsController,
    AcsMgm0,
    AcsMgm1,
    AcsMgt,

    EpsSubsystem,
    EpsPcdu,

    UdpServer,
    TcpServer,
    EventManager,

    Ground,
}

#[derive(Debug, PartialEq, Eq, strum::EnumIter)]
#[bitbybit::bitenum(u11)]
pub enum Apid {
    Tmtc = 1,
    Cfdp = 2,

    Acs = 3,
    Eps = 6,
}

#[derive(Debug, Copy, Clone, serde::Serialize, serde::Deserialize)]
pub enum Event {
    ControllerEvent(control::Event),
}

impl Message for Event {
    fn message_type(&self) -> MessageType {
        MessageType::Event
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct TmHeader {
    pub sender_id: ComponentId,
    pub target_id: ComponentId,
    pub message_type: MessageType,
    /// Telemetry can either be sent unsolicited, or as a response to telecommands.
    pub tc_id: Option<CcsdsPacketIdAndPsc>,
    /// Raw CDS short timestamp.
    pub timestamp: Option<[u8; 7]>,
}

impl TmHeader {
    pub fn new(
        sender_id: ComponentId,
        target_id: ComponentId,
        message_type: MessageType,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        cds_timestamp: &CdsTime,
    ) -> Self {
        // Can not fail, CDS short always requires 7 bytes.
        let mut stamp_buf: [u8; MIN_CDS_FIELD_LEN] = [0; MIN_CDS_FIELD_LEN];
        cds_timestamp.write_to_bytes(&mut stamp_buf).unwrap();
        Self {
            sender_id,
            target_id,
            tc_id,
            message_type,
            timestamp: Some(stamp_buf),
        }
    }
    pub fn new_for_unsolicited_tm(
        sender_id: ComponentId,
        target_id: ComponentId,
        message_type: MessageType,
        cds_timestamp: &CdsTime,
    ) -> Self {
        Self::new(sender_id, target_id, message_type, None, cds_timestamp)
    }

    pub fn new_for_tc_response(
        sender_id: ComponentId,
        target_id: ComponentId,
        message_type: MessageType,
        tc_id: CcsdsPacketIdAndPsc,
        cds_timestamp: &CdsTime,
    ) -> Self {
        Self::new(
            sender_id,
            target_id,
            message_type,
            Some(tc_id),
            cds_timestamp,
        )
    }

    pub fn from_bytes_postcard(data: &[u8]) -> Result<(Self, &[u8]), postcard::Error> {
        postcard::take_from_bytes::<TmHeader>(data)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct TcHeader {
    pub target_id: ComponentId,
    pub request_type: MessageType,
}

impl TcHeader {
    pub fn new(target_id: ComponentId, request_type: MessageType) -> Self {
        Self {
            target_id,
            request_type,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MessageType {
    Ping,
    Mode,
    Hk,
    Action,
    Event,
    Verification,
    Health,
}

pub trait Message {
    fn message_type(&self) -> MessageType;
}

/// Generic device mode which covers the requirements of most devices.
///
/// The states are related both to the physical and the logical state of the device. Some
/// device handlers control the power supply of their own device and an off state might also
/// mean that the device is physically off.
#[derive(
    serde::Serialize,
    serde::Deserialize,
    Debug,
    PartialEq,
    Eq,
    Copy,
    Clone,
    num_enum::IntoPrimitive,
    num_enum::TryFromPrimitive,
)]
#[repr(u32)]
pub enum DeviceMode {
    Off = 0,
    On = 1,
    /// Normal operation mode where periodic polling might be done as well.
    Normal = 2,
}

impl FromStr for DeviceMode {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" => Ok(DeviceMode::Off),
            "on" => Ok(DeviceMode::On),
            "normal" => Ok(DeviceMode::Normal),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum HkRequestType {
    OneShot,
    /// Enable periodic HK generation with a specified frequency.
    EnablePeriodic(core::time::Duration),
    DisablePeriodic,
    /// Modify periodic HK generation interval.
    ModifyInterval(core::time::Duration),
}

#[cfg(test)]
mod tests {}
