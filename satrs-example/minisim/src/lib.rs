use nexosim::time::MonotonicTime;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum SimComponent {
    SimCtrl,
    Mgm0Lis3Mdl,
    Mgm1Lis3Mdl,
    Mgt,
    Pcdu,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimMessage {
    pub target: SimComponent,
    pub payload: String,
}

/// A generic simulation request type. Right now, the payload data is expected to be
/// JSON, which might be changed in the future.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimRequest {
    inner: SimMessage,
    pub timestamp: MonotonicTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimMessageType {
    Request,
    Reply,
}

/// Generic trait implemented by simulation request or reply payloads. It ties the request or
/// reply to a specific target and provides an API which does boilerplate tasks like checking the
/// validity of the target.
pub trait SerializableSimMsgPayload<P: SimMessageProvider>:
    Serialize + DeserializeOwned + Sized
{
    const TARGET: SimComponent;

    fn from_sim_message(sim_message: &P) -> Result<Self, SimMessageError<P>> {
        if sim_message.component() == Self::TARGET {
            return Ok(serde_json::from_str(sim_message.payload())?);
        }
        Err(SimMessageError::TargetRequestMissmatch(sim_message.clone()))
    }
}

pub trait SimMessageProvider: Serialize + DeserializeOwned + Clone + Sized {
    fn msg_type(&self) -> SimMessageType;
    fn component(&self) -> SimComponent;
    fn payload(&self) -> &String;
    fn from_raw_data(data: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(data)
    }
}

impl SimRequest {
    pub fn new_with_epoch_time<T: SerializableSimMsgPayload<SimRequest>>(
        serializable_request: T,
    ) -> Self {
        Self::new(serializable_request, MonotonicTime::EPOCH)
    }

    pub fn new<T: SerializableSimMsgPayload<SimRequest>>(
        serializable_request: T,
        timestamp: MonotonicTime,
    ) -> Self {
        Self {
            inner: SimMessage {
                target: T::TARGET,
                payload: serde_json::to_string(&serializable_request).unwrap(),
            },
            timestamp,
        }
    }
}

impl SimMessageProvider for SimRequest {
    fn component(&self) -> SimComponent {
        self.inner.target
    }
    fn payload(&self) -> &String {
        &self.inner.payload
    }

    fn msg_type(&self) -> SimMessageType {
        SimMessageType::Request
    }
}

/// A generic simulation reply type. Right now, the payload data is expected to be
/// JSON, which might be changed in the future.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimReply {
    inner: SimMessage,
}

impl SimReply {
    pub fn new<T: SerializableSimMsgPayload<SimReply>>(serializable_reply: &T) -> Self {
        Self {
            inner: SimMessage {
                target: T::TARGET,
                payload: serde_json::to_string(serializable_reply).unwrap(),
            },
        }
    }

    /// For payloads where the target is only known at runtime.
    pub fn new_with_target<T: Serialize>(target: SimComponent, reply: &T) -> Self {
        Self {
            inner: SimMessage {
                target,
                payload: serde_json::to_string(reply).unwrap(),
            },
        }
    }
}

impl SimMessageProvider for SimReply {
    fn component(&self) -> SimComponent {
        self.inner.target
    }
    fn payload(&self) -> &String {
        &self.inner.payload
    }
    fn msg_type(&self) -> SimMessageType {
        SimMessageType::Reply
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimCtrlRequest {
    Ping,
}

impl SerializableSimMsgPayload<SimRequest> for SimCtrlRequest {
    const TARGET: SimComponent = SimComponent::SimCtrl;
}

pub type SimReplyError = SimMessageError<SimReply>;
pub type SimRequestError = SimMessageError<SimRequest>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimMessageError<P> {
    SerdeJson(String),
    TargetRequestMissmatch(P),
}

impl<P> From<serde_json::Error> for SimMessageError<P> {
    fn from(error: serde_json::Error) -> SimMessageError<P> {
        SimMessageError::SerdeJson(error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimCtrlReply {
    Pong,
    InvalidRequest(SimRequestError),
}

impl SerializableSimMsgPayload<SimReply> for SimCtrlReply {
    const TARGET: SimComponent = SimComponent::SimCtrl;
}

impl From<SimRequestError> for SimCtrlReply {
    fn from(error: SimRequestError) -> Self {
        SimCtrlReply::InvalidRequest(error)
    }
}

pub mod eps {
    use super::*;
    use types::pcdu::{SwitchId, SwitchMapBinary, SwitchStateBinary};

    #[derive(Debug, Copy, Clone)]
    #[repr(u8)]
    pub enum PcduRequestId {
        SwitchDevice = 0,
        RequestSwitchInfo = 1,
    }

    #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub enum PcduRequest {
        SwitchDevice {
            switch: SwitchId,
            state: SwitchStateBinary,
        },
        RequestSwitchInfo,
    }

    impl SerializableSimMsgPayload<SimRequest> for PcduRequest {
        const TARGET: SimComponent = SimComponent::Pcdu;
    }

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    pub enum PcduReply {
        // Ack,
        SwitchInfo(SwitchMapBinary),
    }

    impl SerializableSimMsgPayload<SimReply> for PcduReply {
        const TARGET: SimComponent = SimComponent::Pcdu;
    }
}

pub mod acs {
    use std::time::Duration;

    use types::pcdu::SwitchStateBinary;

    use super::*;

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

    #[derive(Debug, Copy, Clone, Serialize, Deserialize)]
    pub enum MgmRequestLis3Mdl {
        RequestSensorData,
        /// Force the raw register reply into a stuck-bus pattern, regardless of switch state.
        /// Used to test FDIR handling of SPI bus faults.
        SetSpiFault(SpiFault),
    }

    #[derive(Debug, Copy, Clone, Serialize, Deserialize)]
    pub struct MgmRequestLis3MdlMgm0(pub MgmRequestLis3Mdl);

    impl SerializableSimMsgPayload<SimRequest> for MgmRequestLis3MdlMgm0 {
        const TARGET: SimComponent = SimComponent::Mgm0Lis3Mdl;
    }

    #[derive(Debug, Copy, Clone, Serialize, Deserialize)]
    pub struct MgmRequestLis3MdlMgm1(pub MgmRequestLis3Mdl);

    impl SerializableSimMsgPayload<SimRequest> for MgmRequestLis3MdlMgm1 {
        const TARGET: SimComponent = SimComponent::Mgm1Lis3Mdl;
    }

    // Normally, small magnetometers generate their output as a signed 16 bit raw format or something
    // similar which needs to be converted to a signed float value with physical units. We will
    // simplify this now and generate the signed float values directly. The unit is micro tesla.
    #[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
    pub struct MgmSensorValuesMicroTesla {
        pub x: f32,
        pub y: f32,
        pub z: f32,
    }

    #[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
    pub struct MgmReplyCommon {}

    pub const MGT_GEN_MAGNETIC_FIELD: MgmSensorValuesMicroTesla = MgmSensorValuesMicroTesla {
        x: 30.0,
        y: -30.0,
        z: 30.0,
    };
    pub const ALL_ONES_SENSOR_VAL: i16 = 0xffff_u16 as i16;
    pub const ALL_ZEROS_SENSOR_VAL: i16 = 0;

    /// MGM module strongly based on the LIS3MDL device.
    pub mod mgm {

        use super::*;

        // Field data register scaling
        pub const GAUSS_TO_MICROTESLA_FACTOR: u32 = 100;
        pub const FIELD_LSB_PER_GAUSS_4_SENS: f32 = 1.0 / 6842.0;
        pub const FIELD_LSB_PER_GAUSS_8_SENS: f32 = 1.0 / 3421.0;
        pub const FIELD_LSB_PER_GAUSS_12_SENS: f32 = 1.0 / 2281.0;
        pub const FIELD_LSB_PER_GAUSS_16_SENS: f32 = 1.0 / 1711.0;

        #[derive(Default, Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
        pub struct RawValues {
            pub x: i16,
            pub y: i16,
            pub z: i16,
        }

        #[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
        pub struct MgmReply {
            pub switch_state: SwitchStateBinary,
            pub sensor_values: MgmSensorValuesMicroTesla,
            // Raw sensor values which are transmitted by the LIS3 device in little-endian
            // order.
            pub raw: RawValues,
        }

        #[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
        pub enum MgmId {
            Mgm0,
            Mgm1,
        }

        impl MgmId {
            pub const fn sim_component(&self) -> SimComponent {
                match self {
                    MgmId::Mgm0 => SimComponent::Mgm0Lis3Mdl,
                    MgmId::Mgm1 => SimComponent::Mgm1Lis3Mdl,
                }
            }
        }

        /// Does not implement [SerializableSimMsgPayload] because the target depends on the
        /// MGM ID, which is only known at runtime.
        #[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
        pub struct MgmReplyWrapper {
            pub mgm_id: MgmId,
            pub reply: MgmReply,
        }

        impl MgmReplyWrapper {
            pub fn to_sim_reply(&self) -> SimReply {
                SimReply::new_with_target(self.mgm_id.sim_component(), self)
            }

            pub fn from_sim_reply(sim_reply: &SimReply) -> Result<Self, SimReplyError> {
                let wrapper: Self = serde_json::from_str(sim_reply.payload())?;
                if wrapper.mgm_id.sim_component() != sim_reply.component() {
                    return Err(SimMessageError::TargetRequestMissmatch(sim_reply.clone()));
                }
                Ok(wrapper)
            }
        }

        impl MgmReply {
            pub fn new(
                switch_state: SwitchStateBinary,
                sensor_values: MgmSensorValuesMicroTesla,
                fault_mode: SpiFaultMode,
            ) -> Self {
                match fault_mode {
                    SpiFaultMode::AllZeros => {
                        return Self {
                            switch_state,
                            sensor_values,
                            raw: RawValues {
                                x: ALL_ZEROS_SENSOR_VAL,
                                y: ALL_ZEROS_SENSOR_VAL,
                                z: ALL_ZEROS_SENSOR_VAL,
                            },
                        };
                    }
                    SpiFaultMode::AllOnes => {
                        return Self {
                            switch_state,
                            sensor_values,
                            raw: RawValues {
                                x: ALL_ONES_SENSOR_VAL,
                                y: ALL_ONES_SENSOR_VAL,
                                z: ALL_ONES_SENSOR_VAL,
                            },
                        };
                    }
                    SpiFaultMode::None => (),
                }
                match switch_state {
                    SwitchStateBinary::Off => Self {
                        switch_state,
                        sensor_values,
                        raw: RawValues {
                            x: ALL_ONES_SENSOR_VAL,
                            y: ALL_ONES_SENSOR_VAL,
                            z: ALL_ONES_SENSOR_VAL,
                        },
                    },
                    SwitchStateBinary::On => {
                        let mut raw_reply: [u8; 7] = [0; 7];
                        let raw_x: i16 = (sensor_values.x
                            / (GAUSS_TO_MICROTESLA_FACTOR as f32 * FIELD_LSB_PER_GAUSS_4_SENS))
                            .round() as i16;
                        let raw_y: i16 = (sensor_values.y
                            / (GAUSS_TO_MICROTESLA_FACTOR as f32 * FIELD_LSB_PER_GAUSS_4_SENS))
                            .round() as i16;
                        let raw_z: i16 = (sensor_values.z
                            / (GAUSS_TO_MICROTESLA_FACTOR as f32 * FIELD_LSB_PER_GAUSS_4_SENS))
                            .round() as i16;
                        // The first byte is a dummy byte.
                        raw_reply[1..3].copy_from_slice(&raw_x.to_be_bytes());
                        raw_reply[3..5].copy_from_slice(&raw_y.to_be_bytes());
                        raw_reply[5..7].copy_from_slice(&raw_z.to_be_bytes());
                        Self {
                            switch_state,
                            sensor_values,
                            raw: RawValues {
                                x: raw_x,
                                y: raw_y,
                                z: raw_z,
                            },
                        }
                    }
                }
            }
        }
    }

    // Simple model using i16 values.
    #[derive(Default, Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct MgtDipole {
        pub x: i16,
        pub y: i16,
        pub z: i16,
    }

    #[derive(Debug, Copy, Clone, PartialEq, Serialize, Deserialize)]
    pub enum MgtRequestType {
        ApplyTorque,
    }

    #[derive(Debug, Copy, Clone, Serialize, Deserialize)]
    pub enum MgtRequest {
        ApplyTorque {
            duration: Duration,
            dipole: MgtDipole,
        },
        RequestHk,
    }

    impl SerializableSimMsgPayload<SimRequest> for MgtRequest {
        const TARGET: SimComponent = SimComponent::Mgt;
    }

    #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct MgtHkSet {
        pub dipole: MgtDipole,
        pub torquing: bool,
    }

    #[derive(Debug, Copy, Clone, Serialize, Deserialize)]
    pub enum MgtReply {
        Ack(MgtRequestType),
        Nak(MgtRequestType),
        Hk(MgtHkSet),
    }

    impl SerializableSimMsgPayload<SimReply> for MgtReply {
        const TARGET: SimComponent = SimComponent::Mgm0Lis3Mdl;
    }
}

pub mod udp {
    pub const SIM_CTRL_PORT: u16 = 7303;
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub enum DummyRequest {
        Ping,
    }

    impl SerializableSimMsgPayload<SimRequest> for DummyRequest {
        const TARGET: SimComponent = SimComponent::SimCtrl;
    }

    #[derive(Debug, Copy, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub enum DummyReply {
        Pong,
    }

    impl SerializableSimMsgPayload<SimReply> for DummyReply {
        const TARGET: SimComponent = SimComponent::SimCtrl;
    }

    #[test]
    fn test_basic_request() {
        let sim_request = SimRequest::new_with_epoch_time(DummyRequest::Ping);
        assert_eq!(sim_request.component(), SimComponent::SimCtrl);
        assert_eq!(sim_request.msg_type(), SimMessageType::Request);
        let dummy_request =
            DummyRequest::from_sim_message(&sim_request).expect("deserialization failed");
        assert_eq!(dummy_request, DummyRequest::Ping);
    }

    #[test]
    fn test_basic_reply() {
        let sim_reply = SimReply::new(&DummyReply::Pong);
        assert_eq!(sim_reply.component(), SimComponent::SimCtrl);
        assert_eq!(sim_reply.msg_type(), SimMessageType::Reply);
        let dummy_request =
            DummyReply::from_sim_message(&sim_reply).expect("deserialization failed");
        assert_eq!(dummy_request, DummyReply::Pong);
    }
}
