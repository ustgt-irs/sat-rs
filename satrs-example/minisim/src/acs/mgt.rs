use nexosim::{
    model::{Context, Model},
    ports::Output,
};
use satrs_minisim::{
    acs::{mgm, mgt},
    SimReply,
};
use std::{sync::mpsc, time::Duration};
use types::pcdu::SwitchStateBinary;

/// Simple magnetorquer simulation model.
pub struct MgtModel {
    switch_state: SwitchStateBinary,
    torquing: bool,
    torque_dipole: mgt::Dipole,
    pub gen_magnetic_field: Output<mgm::SensorValuesMicroTesla>,
    reply_sender: mpsc::Sender<SimReply>,
}

impl MgtModel {
    pub fn new(reply_sender: mpsc::Sender<SimReply>) -> Self {
        Self {
            switch_state: SwitchStateBinary::Off,
            torquing: false,
            torque_dipole: mgt::Dipole::default(),
            gen_magnetic_field: Output::new(),
            reply_sender,
        }
    }

    pub async fn apply_torque(
        &mut self,
        duration_and_dipole: (Duration, mgt::Dipole),
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
        self.torque_dipole = mgt::Dipole::default();
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
            .send(SimReply::from(mgt::Reply::Hk(mgt::HkSet {
                dipole: self.torque_dipole,
                torquing: self.torquing,
            })))
            .unwrap();
    }

    fn calc_magnetic_field(&self, _: mgt::Dipole) -> mgm::SensorValuesMicroTesla {
        // Simplified model: Just returns some fixed magnetic field for now.
        // Later, we could make this more fancy by incorporating the commanded dipole.
        mgm::MGT_GEN_MAGNETIC_FIELD
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

impl Model for MgtModel {}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use satrs_minisim::{acs::mgt, SimReply, SimRequestWithTime};
    use types::pcdu::SwitchId;

    use crate::{eps::tests::switch_device_on, test_helpers::SimTestbench};

    fn request_hk(sim_testbench: &mut SimTestbench) -> Option<mgt::HkSet> {
        let sim_reply = sim_testbench.request_reply(mgt::Request::RequestHk)?;
        let SimReply::Mgt(mgt::Reply::Hk(hk)) = sim_reply else {
            panic!("unexpected reply {sim_reply:?}");
        };
        Some(hk)
    }

    #[test]
    fn test_basic_mgt_request_is_off() {
        let mut sim_testbench = SimTestbench::new();
        assert!(request_hk(&mut sim_testbench).is_none());
    }

    #[test]
    fn test_basic_mgt_request_is_on() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgt);
        assert_eq!(
            request_hk(&mut sim_testbench),
            Some(mgt::HkSet {
                dipole: mgt::Dipole::default(),
                torquing: false,
            })
        );
    }

    #[test]
    fn test_basic_mgt_request_is_on_and_torquing() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgt);
        let commanded_dipole = mgt::Dipole {
            x: -200,
            y: 200,
            z: 1000,
        };
        let request = SimRequestWithTime::new_with_epoch_time(mgt::Request::ApplyTorque {
            duration: Duration::from_millis(100),
            dipole: commanded_dipole,
        });
        sim_testbench
            .send_request(request)
            .expect("sending MGT request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step_until(Duration::from_millis(5)).unwrap();

        assert_eq!(
            request_hk(&mut sim_testbench),
            Some(mgt::HkSet {
                dipole: commanded_dipole,
                torquing: true,
            })
        );
        sim_testbench
            .step_until(Duration::from_millis(100))
            .unwrap();
        assert_eq!(
            request_hk(&mut sim_testbench),
            Some(mgt::HkSet {
                dipole: mgt::Dipole::default(),
                torquing: false,
            })
        );
    }
}
