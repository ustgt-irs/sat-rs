use nexosim::{
    model::{Context, Model},
    ports::Output,
};
use satrs_minisim::{
    acs::{MgmSensorValuesMicroTesla, MgtDipole, MgtHkSet, MgtReply, MGT_GEN_MAGNETIC_FIELD},
    SimReply,
};
use std::{sync::mpsc, time::Duration};
use types::pcdu::SwitchStateBinary;

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
mod tests {
    use std::time::Duration;

    use satrs_minisim::{
        acs::{MgtDipole, MgtHkSet, MgtReply, MgtRequest},
        SerializableSimMsgPayload, SimRequest,
    };
    use types::pcdu::SwitchId;

    use crate::{eps::tests::switch_device_on, test_helpers::SimTestbench};

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
