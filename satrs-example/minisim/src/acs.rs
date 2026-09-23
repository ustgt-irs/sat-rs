use std::{f32::consts::PI, sync::mpsc, time::Duration};

use nexosim::{
    model::{Context, Model},
    ports::Output,
};
use satrs_minisim::{
    acs::{
        lis3mdl::MgmLis3MdlReply, MgmReplyCommon, MgmReplyProvider, MgmSensorValuesMicroTesla,
        MgtDipole, MgtHkSet, MgtReply, SpiFault, MGT_GEN_MAGNETIC_FIELD,
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
pub struct MagnetometerModel<ReplyProvider: MgmReplyProvider> {
    pub switch_state: SwitchStateBinary,
    #[allow(dead_code)]
    pub periodicity: Duration,
    pub external_mag_field: Option<MgmSensorValuesMicroTesla>,
    pub spi_fault: SpiFault,
    pub reply_sender: mpsc::Sender<SimReply>,
    pub phatom: std::marker::PhantomData<ReplyProvider>,
}

impl MagnetometerModel<MgmLis3MdlReply> {
    pub fn new_for_lis3mdl(periodicity: Duration, reply_sender: mpsc::Sender<SimReply>) -> Self {
        Self {
            switch_state: SwitchStateBinary::Off,
            periodicity,
            external_mag_field: None,
            spi_fault: SpiFault::default(),
            reply_sender,
            phatom: std::marker::PhantomData,
        }
    }
}

impl<ReplyProvider: MgmReplyProvider> MagnetometerModel<ReplyProvider> {
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
        self.reply_sender
            .send(ReplyProvider::create_mgm_reply(
                MgmReplyCommon {
                    switch_state: self.switch_state,
                    sensor_values: self
                        .calculate_current_mgm_tuple(current_millis(scheduler.time())),
                },
                self.spi_fault.mode,
            ))
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

impl<ReplyProvider: MgmReplyProvider> Model for MagnetometerModel<ReplyProvider> {}

pub struct MagnetorquerModel {
    switch_state: SwitchStateBinary,
    torquing: bool,
    torque_dipole: MgtDipole,
    pub gen_magnetic_field: Output<MgmSensorValuesMicroTesla>,
    reply_sender: mpsc::Sender<SimReply>,
}

impl MagnetorquerModel {
    pub fn new(reply_sender: mpsc::Sender<SimReply>) -> Self {
        Self {
            switch_state: SwitchStateBinary::Off,
            torquing: false,
            torque_dipole: MgtDipole::default(),
            gen_magnetic_field: Output::new(),
            reply_sender,
        }
    }

    pub async fn apply_torque(
        &mut self,
        duration_and_dipole: (Duration, MgtDipole),
        cx: &mut Context<Self>,
    ) {
        self.torque_dipole = duration_and_dipole.1;
        self.torquing = true;
        if cx
            .schedule_event(duration_and_dipole.0, Self::clear_torque, ())
            .is_err()
        {
            log::warn!("torque clearing can only be set for a future time.");
        }
        self.generate_magnetic_field(()).await;
    }

    pub async fn clear_torque(&mut self, _: ()) {
        self.torque_dipole = MgtDipole::default();
        self.torquing = false;
        self.generate_magnetic_field(()).await;
    }

    pub async fn switch_device(&mut self, switch_state: SwitchStateBinary) {
        self.switch_state = switch_state;
        self.generate_magnetic_field(()).await;
    }

    pub async fn request_housekeeping_data(&mut self, _: (), cx: &mut Context<Self>) {
        if self.switch_state != SwitchStateBinary::On {
            return;
        }
        cx.schedule_event(Duration::from_millis(15), Self::send_housekeeping_data, ())
            .expect("requesting housekeeping data failed")
    }

    pub fn send_housekeeping_data(&mut self) {
        self.reply_sender
            .send(SimReply::new(&MgtReply::Hk(MgtHkSet {
                dipole: self.torque_dipole,
                torquing: self.torquing,
            })))
            .unwrap();
    }

    fn calc_magnetic_field(&self, _: MgtDipole) -> MgmSensorValuesMicroTesla {
        // Simplified model: Just returns some fixed magnetic field for now.
        // Later, we could make this more fancy by incorporating the commanded dipole.
        MGT_GEN_MAGNETIC_FIELD
    }

    /// A torquing magnetorquer generates a magnetic field. This function can be used to apply
    /// the magnetic field.
    async fn generate_magnetic_field(&mut self, _: ()) {
        if self.switch_state != SwitchStateBinary::On || !self.torquing {
            return;
        }
        self.gen_magnetic_field
            .send(self.calc_magnetic_field(self.torque_dipole))
            .await;
    }
}

impl Model for MagnetorquerModel {}

#[cfg(test)]
pub mod tests {
    use std::time::Duration;

    use satrs_minisim::{
        acs::{
            lis3mdl::{self, MgmLis3MdlReply},
            MgmRequestLis3Mdl, MgtDipole, MgtHkSet, MgtReply, MgtRequest, SpiFault, SpiFaultMode,
        },
        SerializableSimMsgPayload, SimComponent, SimMessageProvider, SimRequest,
    };
    use types::pcdu::{SwitchId, SwitchStateBinary};

    use crate::{
        eps::tests::{switch_device_off, switch_device_on},
        test_helpers::SimTestbench,
    };

    #[test]
    fn test_basic_mgm_request() {
        let mut sim_testbench = SimTestbench::new();
        let request = SimRequest::new_with_epoch_time(MgmRequestLis3Mdl::RequestSensorData);
        sim_testbench
            .send_request(request)
            .expect("sending MGM request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
        let sim_reply = sim_testbench.try_receive_next_reply();
        assert!(sim_reply.is_some());
        let sim_reply = sim_reply.unwrap();
        assert_eq!(sim_reply.component(), SimComponent::Mgm0Lis3Mdl);
        let reply = MgmLis3MdlReply::from_sim_message(&sim_reply)
            .expect("failed to deserialize MGM sensor values");
        assert_eq!(reply.common.switch_state, SwitchStateBinary::Off);
        assert_eq!(reply.common.sensor_values.x, 0.0);
        assert_eq!(reply.common.sensor_values.y, 0.0);
        assert_eq!(reply.common.sensor_values.z, 0.0);
    }

    fn inject_spi_fault(sim_testbench: &mut SimTestbench, cleared_by_power_cycle: bool) {
        let fault_request =
            SimRequest::new_with_epoch_time(MgmRequestLis3Mdl::SetSpiFault(SpiFault {
                mode: SpiFaultMode::AllOnes,
                cleared_by_power_cycle,
            }));
        sim_testbench
            .send_request(fault_request)
            .expect("sending MGM fault injection request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
    }

    fn request_mgm_reply(sim_testbench: &mut SimTestbench) -> MgmLis3MdlReply {
        let data_request = SimRequest::new_with_epoch_time(MgmRequestLis3Mdl::RequestSensorData);
        sim_testbench
            .send_request(data_request)
            .expect("sending MGM request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
        let sim_reply = sim_testbench
            .try_receive_next_reply()
            .expect("no MGM reply received");
        MgmLis3MdlReply::from_sim_message(&sim_reply)
            .expect("failed to deserialize MGM sensor values")
    }

    fn is_stuck_bus_reply(reply: &MgmLis3MdlReply) -> bool {
        reply.raw.x == -1 && reply.raw.y == -1 && reply.raw.z == -1
    }

    #[test]
    fn test_mgm_spi_fault_injection_all_ones() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);
        inject_spi_fault(&mut sim_testbench, false);

        let reply = request_mgm_reply(&mut sim_testbench);
        // Even though the device is switched on, the injected fault forces a stuck-bus reply.
        assert_eq!(reply.common.switch_state, SwitchStateBinary::On);
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
        assert_eq!(reply.common.switch_state, SwitchStateBinary::On);
        assert!(is_stuck_bus_reply(&reply));
    }

    #[test]
    fn test_basic_mgm_request_switched_on() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);

        let mut request = SimRequest::new_with_epoch_time(MgmRequestLis3Mdl::RequestSensorData);
        sim_testbench
            .send_request(request)
            .expect("sending MGM request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
        let mut sim_reply_res = sim_testbench.try_receive_next_reply();
        assert!(sim_reply_res.is_some());
        let mut sim_reply = sim_reply_res.unwrap();
        assert_eq!(sim_reply.component(), SimComponent::Mgm0Lis3Mdl);
        let first_reply = MgmLis3MdlReply::from_sim_message(&sim_reply)
            .expect("failed to deserialize MGM sensor values");
        sim_testbench.step_until(Duration::from_millis(50)).unwrap();

        request = SimRequest::new_with_epoch_time(MgmRequestLis3Mdl::RequestSensorData);
        sim_testbench
            .send_request(request)
            .expect("sending MGM request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
        sim_reply_res = sim_testbench.try_receive_next_reply();
        assert!(sim_reply_res.is_some());
        sim_reply = sim_reply_res.unwrap();

        let second_reply = MgmLis3MdlReply::from_sim_message(&sim_reply)
            .expect("failed to deserialize MGM sensor values");
        let x_conv_back = second_reply.raw.x as f32
            * lis3mdl::FIELD_LSB_PER_GAUSS_4_SENS
            * lis3mdl::GAUSS_TO_MICROTESLA_FACTOR as f32;
        let y_conv_back = second_reply.raw.y as f32
            * lis3mdl::FIELD_LSB_PER_GAUSS_4_SENS
            * lis3mdl::GAUSS_TO_MICROTESLA_FACTOR as f32;
        let z_conv_back = second_reply.raw.z as f32
            * lis3mdl::FIELD_LSB_PER_GAUSS_4_SENS
            * lis3mdl::GAUSS_TO_MICROTESLA_FACTOR as f32;
        let diff_x = (second_reply.common.sensor_values.x - x_conv_back).abs();
        assert!(diff_x < 0.01, "diff x too large: {}", diff_x);
        let diff_y = (second_reply.common.sensor_values.y - y_conv_back).abs();
        assert!(diff_y < 0.01, "diff y too large: {}", diff_y);
        let diff_z = (second_reply.common.sensor_values.z - z_conv_back).abs();
        assert!(diff_z < 0.01, "diff z too large: {}", diff_z);
        // assert_eq!(second_reply.raw_reply, SwitchStateBinary::On);
        // Check that the values are changing.
        assert!(first_reply != second_reply);
    }

    #[test]
    fn test_basic_mgt_request_is_off() {
        let mut sim_testbench = SimTestbench::new();
        let request = SimRequest::new_with_epoch_time(MgtRequest::RequestHk);
        sim_testbench
            .send_request(request)
            .expect("sending MGM request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
        let sim_reply_res = sim_testbench.try_receive_next_reply();
        assert!(sim_reply_res.is_none());
    }

    #[test]
    fn test_basic_mgt_request_is_on() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgt);
        let request = SimRequest::new_with_epoch_time(MgtRequest::RequestHk);

        sim_testbench
            .send_request(request)
            .expect("sending MGM request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
        let sim_reply_res = sim_testbench.try_receive_next_reply();
        assert!(sim_reply_res.is_some());
        let sim_reply = sim_reply_res.unwrap();
        let mgt_reply = MgtReply::from_sim_message(&sim_reply)
            .expect("failed to deserialize MGM sensor values");
        match mgt_reply {
            MgtReply::Hk(hk) => {
                assert_eq!(hk.dipole, MgtDipole::default());
                assert!(!hk.torquing);
            }
            _ => panic!("unexpected reply"),
        }
    }

    fn check_mgt_hk(sim_testbench: &mut SimTestbench, expected_hk_set: MgtHkSet) {
        let request = SimRequest::new_with_epoch_time(MgtRequest::RequestHk);
        sim_testbench
            .send_request(request)
            .expect("sending MGM request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
        let sim_reply_res = sim_testbench.try_receive_next_reply();
        assert!(sim_reply_res.is_some());
        let sim_reply = sim_reply_res.unwrap();
        let mgt_reply = MgtReply::from_sim_message(&sim_reply)
            .expect("failed to deserialize MGM sensor values");
        match mgt_reply {
            MgtReply::Hk(hk) => {
                assert_eq!(hk, expected_hk_set);
            }
            _ => panic!("unexpected reply"),
        }
    }

    #[test]
    fn test_basic_mgt_request_is_on_and_torquing() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgt);
        let commanded_dipole = MgtDipole {
            x: -200,
            y: 200,
            z: 1000,
        };
        let request = SimRequest::new_with_epoch_time(MgtRequest::ApplyTorque {
            duration: Duration::from_millis(100),
            dipole: commanded_dipole,
        });
        sim_testbench
            .send_request(request)
            .expect("sending MGM request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step_until(Duration::from_millis(5)).unwrap();

        check_mgt_hk(
            &mut sim_testbench,
            MgtHkSet {
                dipole: commanded_dipole,
                torquing: true,
            },
        );
        sim_testbench
            .step_until(Duration::from_millis(100))
            .unwrap();
        check_mgt_hk(
            &mut sim_testbench,
            MgtHkSet {
                dipole: MgtDipole::default(),
                torquing: false,
            },
        );
    }
}
