use std::{f32::consts::PI, sync::mpsc, time::Duration};

use nexosim::model::{Context, Model};
use satrs_minisim::{
    acs::{
        mgm::{MgmId, MgmReply, MgmReplyWrapper},
        MgmSensorValuesMicroTesla, SpiFault,
    },
    SimReply,
};
use types::pcdu::SwitchStateBinary;

use crate::time::current_millis;
// Earth magnetic field varies between roughly -30 uT and 30 uT
const AMPLITUDE_MGM_UT: f32 = 30.0;
// Lets start with a simple frequency here.
const FREQUENCY_MGM: f32 = 1.0;
const PHASE_X: f32 = 0.0;
// Different phases to have different values on the other axes.
const PHASE_Y: f32 = 0.1;
const PHASE_Z: f32 = 0.2;

/// Simple model for a magnetometer where the measure magnetic fields are modeled with sine waves.
///
/// An ideal sensor would sample the magnetic field at a high fixed rate. This might not be
/// possible for a general purpose OS, but self self-sampling at a relatively high rate (20-40 ms)
/// might still be possible and is probably sufficient for many OBSW needs.
pub struct MagnetometerModel {
    pub id: MgmId,
    pub switch_state: SwitchStateBinary,
    #[allow(dead_code)]
    pub periodicity: Duration,
    pub external_mag_field: Option<MgmSensorValuesMicroTesla>,
    pub spi_fault: SpiFault,
    pub reply_sender: mpsc::Sender<SimReply>,
}

impl MagnetometerModel {
    pub fn new(mgm_id: MgmId, periodicity: Duration, reply_sender: mpsc::Sender<SimReply>) -> Self {
        Self {
            id: mgm_id,
            switch_state: SwitchStateBinary::Off,
            periodicity,
            external_mag_field: None,
            spi_fault: SpiFault::default(),
            reply_sender,
        }
    }

    pub async fn switch_device(&mut self, switch_state: SwitchStateBinary) {
        self.switch_state = switch_state;
        if switch_state == SwitchStateBinary::Off && self.spi_fault.cleared_by_power_cycle {
            self.spi_fault = SpiFault::default();
        }
    }

    /// Force (or clear) a stuck-bus SPI fault, for FDIR testing purposes.
    pub async fn set_spi_fault(&mut self, fault: SpiFault) {
        self.spi_fault = fault;
    }

    pub async fn send_sensor_values(&mut self, _: (), scheduler: &mut Context<Self>) {
        let reply = MgmReplyWrapper {
            mgm_id: self.id,
            reply: MgmReply::new(
                self.switch_state,
                self.calculate_current_mgm_tuple(current_millis(scheduler.time())),
                self.spi_fault.mode,
            ),
        };
        self.reply_sender
            .send(reply.to_sim_reply())
            .expect("sending MGM sensor values failed");
    }

    // Devices like magnetorquers generate a strong magnetic field which overrides the default
    // model for the measured magnetic field.
    pub async fn apply_external_magnetic_field(&mut self, field: MgmSensorValuesMicroTesla) {
        self.external_mag_field = Some(field);
    }

    fn calculate_current_mgm_tuple(&self, time_ms: u64) -> MgmSensorValuesMicroTesla {
        if SwitchStateBinary::On == self.switch_state {
            if let Some(ext_field) = self.external_mag_field {
                return ext_field;
            }
            let base_sin_val = 2.0 * PI * FREQUENCY_MGM * (time_ms as f32 / 1000.0);
            return MgmSensorValuesMicroTesla {
                x: AMPLITUDE_MGM_UT * (base_sin_val + PHASE_X).sin(),
                y: AMPLITUDE_MGM_UT * (base_sin_val + PHASE_Y).sin(),
                z: AMPLITUDE_MGM_UT * (base_sin_val + PHASE_Z).sin(),
            };
        }
        MgmSensorValuesMicroTesla {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }
}

impl Model for MagnetometerModel {}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use satrs_minisim::{
        acs::{
            mgm::{self, MgmId, MgmReply, MgmReplyWrapper},
            MgmRequestLis3Mdl, MgmRequestLis3MdlMgm0, MgmRequestLis3MdlMgm1, SpiFault,
            SpiFaultMode,
        },
        SimComponent, SimMessageProvider, SimRequest,
    };
    use types::pcdu::{SwitchId, SwitchStateBinary};

    use crate::{
        eps::tests::{switch_device_off, switch_device_on},
        test_helpers::SimTestbench,
    };

    #[test]
    fn test_basic_mgm_request() {
        let mut sim_testbench = SimTestbench::new();
        let request = SimRequest::new_with_epoch_time(MgmRequestLis3MdlMgm0(
            MgmRequestLis3Mdl::RequestSensorData,
        ));
        sim_testbench
            .send_request(request)
            .expect("sending MGM request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
        let sim_reply = sim_testbench.try_receive_next_reply();
        assert!(sim_reply.is_some());
        let sim_reply = sim_reply.unwrap();
        assert_eq!(sim_reply.component(), SimComponent::Mgm0Lis3Mdl);
        let wrapper = MgmReplyWrapper::from_sim_reply(&sim_reply)
            .expect("failed to deserialize MGM sensor values");
        assert_eq!(wrapper.mgm_id, MgmId::Mgm0);
        assert_eq!(wrapper.reply.switch_state, SwitchStateBinary::Off);
        assert_eq!(wrapper.reply.sensor_values.x, 0.0);
        assert_eq!(wrapper.reply.sensor_values.y, 0.0);
        assert_eq!(wrapper.reply.sensor_values.z, 0.0);
    }

    fn inject_spi_fault(sim_testbench: &mut SimTestbench, cleared_by_power_cycle: bool) {
        let fault_request = SimRequest::new_with_epoch_time(MgmRequestLis3MdlMgm0(
            MgmRequestLis3Mdl::SetSpiFault(SpiFault {
                mode: SpiFaultMode::AllOnes,
                cleared_by_power_cycle,
            }),
        ));
        sim_testbench
            .send_request(fault_request)
            .expect("sending MGM fault injection request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
    }

    fn request_mgm_reply(sim_testbench: &mut SimTestbench) -> MgmReply {
        let data_request = SimRequest::new_with_epoch_time(MgmRequestLis3MdlMgm0(
            MgmRequestLis3Mdl::RequestSensorData,
        ));
        sim_testbench
            .send_request(data_request)
            .expect("sending MGM request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
        let sim_reply = sim_testbench
            .try_receive_next_reply()
            .expect("no MGM reply received");
        MgmReplyWrapper::from_sim_reply(&sim_reply)
            .expect("failed to deserialize MGM sensor values")
            .reply
    }

    fn is_stuck_bus_reply(reply: &MgmReply) -> bool {
        reply.raw.x == -1 && reply.raw.y == -1 && reply.raw.z == -1
    }

    #[test]
    fn test_mgm_spi_fault_injection_all_ones() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);
        inject_spi_fault(&mut sim_testbench, false);

        let reply = request_mgm_reply(&mut sim_testbench);
        // Even though the device is switched on, the injected fault forces a stuck-bus reply.
        assert_eq!(reply.switch_state, SwitchStateBinary::On);
        assert!(is_stuck_bus_reply(&reply));
    }

    #[test]
    fn test_mgm_spi_fault_cleared_by_power_cycle() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);
        inject_spi_fault(&mut sim_testbench, true);
        assert!(is_stuck_bus_reply(&request_mgm_reply(&mut sim_testbench)));

        switch_device_off(&mut sim_testbench, SwitchId::Mgm0);
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);
        sim_testbench.step_until(Duration::from_millis(50)).unwrap();
        assert!(!is_stuck_bus_reply(&request_mgm_reply(&mut sim_testbench)));
    }

    #[test]
    fn test_mgm_spi_fault_persists_after_power_cycle() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);
        inject_spi_fault(&mut sim_testbench, false);

        switch_device_off(&mut sim_testbench, SwitchId::Mgm0);
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);
        let reply = request_mgm_reply(&mut sim_testbench);
        assert_eq!(reply.switch_state, SwitchStateBinary::On);
        assert!(is_stuck_bus_reply(&reply));
    }

    #[test]
    fn test_basic_mgm_request_switched_on() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);

        let mut request = SimRequest::new_with_epoch_time(MgmRequestLis3MdlMgm0(
            MgmRequestLis3Mdl::RequestSensorData,
        ));
        sim_testbench
            .send_request(request)
            .expect("sending MGM request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
        let mut sim_reply_res = sim_testbench.try_receive_next_reply();
        assert!(sim_reply_res.is_some());
        let mut sim_reply = sim_reply_res.unwrap();
        assert_eq!(sim_reply.component(), SimComponent::Mgm0Lis3Mdl);
        let first_reply = MgmReplyWrapper::from_sim_reply(&sim_reply)
            .expect("failed to deserialize MGM sensor values")
            .reply;
        sim_testbench.step_until(Duration::from_millis(50)).unwrap();

        request = SimRequest::new_with_epoch_time(MgmRequestLis3MdlMgm0(
            MgmRequestLis3Mdl::RequestSensorData,
        ));
        sim_testbench
            .send_request(request)
            .expect("sending MGM request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
        sim_reply_res = sim_testbench.try_receive_next_reply();
        assert!(sim_reply_res.is_some());
        sim_reply = sim_reply_res.unwrap();

        let second_reply = MgmReplyWrapper::from_sim_reply(&sim_reply)
            .expect("failed to deserialize MGM sensor values")
            .reply;
        let x_conv_back = second_reply.raw.x as f32
            * mgm::FIELD_LSB_PER_GAUSS_4_SENS
            * mgm::GAUSS_TO_MICROTESLA_FACTOR as f32;
        let y_conv_back = second_reply.raw.y as f32
            * mgm::FIELD_LSB_PER_GAUSS_4_SENS
            * mgm::GAUSS_TO_MICROTESLA_FACTOR as f32;
        let z_conv_back = second_reply.raw.z as f32
            * mgm::FIELD_LSB_PER_GAUSS_4_SENS
            * mgm::GAUSS_TO_MICROTESLA_FACTOR as f32;
        let diff_x = (second_reply.sensor_values.x - x_conv_back).abs();
        assert!(diff_x < 0.01, "diff x too large: {}", diff_x);
        let diff_y = (second_reply.sensor_values.y - y_conv_back).abs();
        assert!(diff_y < 0.01, "diff y too large: {}", diff_y);
        let diff_z = (second_reply.sensor_values.z - z_conv_back).abs();
        assert!(diff_z < 0.01, "diff z too large: {}", diff_z);
        // assert_eq!(second_reply.raw_reply, SwitchStateBinary::On);
        // Check that the values are changing.
        assert!(first_reply != second_reply);
    }

    #[test]
    fn test_mgm_1_request_switched_on() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgm1);

        for request in [
            SimRequest::new_with_epoch_time(MgmRequestLis3MdlMgm0(
                MgmRequestLis3Mdl::RequestSensorData,
            )),
            SimRequest::new_with_epoch_time(MgmRequestLis3MdlMgm1(
                MgmRequestLis3Mdl::RequestSensorData,
            )),
        ] {
            sim_testbench
                .send_request(request)
                .expect("sending MGM request failed");
        }
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();

        let sim_reply = sim_testbench
            .try_receive_next_reply()
            .expect("no MGM0 reply received");
        assert_eq!(sim_reply.component(), SimComponent::Mgm0Lis3Mdl);
        let mgm_0_reply = MgmReplyWrapper::from_sim_reply(&sim_reply)
            .expect("failed to deserialize MGM0 sensor values");
        assert_eq!(mgm_0_reply.mgm_id, MgmId::Mgm0);
        assert_eq!(mgm_0_reply.reply.switch_state, SwitchStateBinary::Off);

        let sim_reply = sim_testbench
            .try_receive_next_reply()
            .expect("no MGM1 reply received");
        assert_eq!(sim_reply.component(), SimComponent::Mgm1Lis3Mdl);
        let mgm_1_reply = MgmReplyWrapper::from_sim_reply(&sim_reply)
            .expect("failed to deserialize MGM1 sensor values");
        assert_eq!(mgm_1_reply.mgm_id, MgmId::Mgm1);
        assert_eq!(mgm_1_reply.reply.switch_state, SwitchStateBinary::On);
    }
}
