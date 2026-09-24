use nexosim::{
    model::{schedulable, Context, Model},
    ports::Output,
};
use satrs_minisim::{
    acs::{mgm, mgt},
    SimReply,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use types::pcdu::SwitchStateBinary;

/// Time the device needs to answer a command.
const REPLY_DELAY: Duration = Duration::from_millis(10);

/// Simple magnetorquer simulation model.
#[derive(Serialize, Deserialize)]
pub struct MgtModel {
    switch_state: SwitchStateBinary,
    torquing: bool,
    torque_dipole: mgt::Dipole,
    pub gen_magnetic_field: Output<mgm::SensorValuesMicroTesla>,
    pub clear_magnetic_field: Output<()>,
    pub reply: Output<SimReply>,
}

#[Model]
impl MgtModel {
    pub fn new() -> Self {
        Self {
            switch_state: SwitchStateBinary::Off,
            torquing: false,
            torque_dipole: mgt::Dipole::default(),
            gen_magnetic_field: Output::new(),
            clear_magnetic_field: Output::new(),
            reply: Output::new(),
        }
    }

    pub async fn apply_torque(
        &mut self,
        duration_and_dipole: (Duration, mgt::Dipole),
        cx: &Context<Self>,
    ) {
        if self.switch_state != SwitchStateBinary::On {
            return;
        }
        self.torque_dipole = duration_and_dipole.1;
        self.torquing = true;
        if cx
            .schedule_event(duration_and_dipole.0, schedulable!(Self::clear_torque), ())
            .is_err()
        {
            log::warn!("torque clearing can only be set for a future time.");
        }
        self.generate_magnetic_field(()).await;
        self.schedule_reply(mgt::Reply::Ack, cx);
    }

    #[nexosim(schedulable)]
    async fn clear_torque(&mut self) {
        self.torque_dipole = mgt::Dipole::default();
        self.torquing = false;
        self.clear_magnetic_field.send(()).await;
    }

    pub async fn switch_device(&mut self, switch_state: SwitchStateBinary) {
        self.switch_state = switch_state;
        match switch_state {
            SwitchStateBinary::On => self.generate_magnetic_field(()).await,
            SwitchStateBinary::Off => self.clear_torque().await,
        }
    }

    pub async fn request_housekeeping_data(&mut self, _: (), cx: &Context<Self>) {
        if self.switch_state != SwitchStateBinary::On {
            return;
        }
        // The HK is sampled when the command is processed, not when the reply is sent.
        let hk = mgt::HkSet {
            dipole: self.torque_dipole,
            torquing: self.torquing,
        };
        self.schedule_reply(mgt::Reply::Hk(hk), cx);
    }

    fn schedule_reply(&self, reply: mgt::Reply, cx: &Context<Self>) {
        cx.schedule_event(REPLY_DELAY, schedulable!(Self::send_reply), reply)
            .expect("scheduling MGT reply failed")
    }

    #[nexosim(schedulable)]
    async fn send_reply(&mut self, reply: mgt::Reply) {
        self.reply.send(SimReply::from(reply)).await;
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use satrs_minisim::{
        acs::{mgm, mgt},
        eps::PcduRequest,
        SimReply, SimRequest, SimRequestWithTime,
    };
    use types::pcdu::{SwitchId, SwitchStateBinary};

    use crate::{eps::tests::switch_device_on, test_helpers::SimTestbench};

    fn decode_reply(sim_reply: SimReply) -> mgt::Reply {
        let SimReply::Mgt(frame) = sim_reply else {
            panic!("unexpected reply {sim_reply:?}");
        };
        mgt::Reply::from_frame(&frame).expect("invalid MGT reply frame")
    }

    fn request_hk(sim_testbench: &mut SimTestbench) -> Option<mgt::HkSet> {
        let sim_reply = sim_testbench.request_reply(mgt::Request::RequestHk)?;
        let mgt::Reply::Hk(hk) = decode_reply(sim_reply) else {
            panic!("unexpected MGT reply");
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
        sim_testbench.step_until(Duration::from_millis(20)).unwrap();
        let ack = sim_testbench
            .try_receive_next_reply()
            .expect("no torque command ack");
        assert_eq!(decode_reply(ack), mgt::Reply::Ack);

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

    #[test]
    fn test_torque_command_not_acked_when_off() {
        let mut sim_testbench = SimTestbench::new();
        let reply = sim_testbench.request_reply(mgt::Request::ApplyTorque {
            duration: Duration::from_millis(100),
            dipole: mgt::Dipole { x: 1, y: 2, z: 3 },
        });
        assert!(reply.is_none());
    }

    #[test]
    fn test_invalid_frame_is_dropped() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgt);
        assert!(sim_testbench
            .request_reply(SimRequest::Mgt(vec![0x01, 0x00]))
            .is_none());
    }

    /// Processes the request without stepping, so scheduled events like the torque clearing do
    /// not fire.
    fn process_without_step(sim_testbench: &mut SimTestbench, request: impl Into<SimRequest>) {
        sim_testbench
            .send_request(SimRequestWithTime::new_with_epoch_time(request))
            .expect("sending request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
    }

    fn read_mgm_0_field(sim_testbench: &mut SimTestbench) -> mgm::SensorValuesMicroTesla {
        process_without_step(
            sim_testbench,
            SimRequest::Mgm {
                id: mgm::Id::Mgm0,
                request: mgm::Request::RequestSensorData,
            },
        );
        // Skips pending MGT replies, for example torque command acks.
        loop {
            let sim_reply = sim_testbench
                .try_receive_next_reply()
                .expect("no MGM reply received");
            if let SimReply::Mgm { reply, .. } = sim_reply {
                return reply.sensor_values;
            }
        }
    }

    fn start_torquing(sim_testbench: &mut SimTestbench, duration: Duration) {
        switch_device_on(sim_testbench, SwitchId::Mgm0);
        switch_device_on(sim_testbench, SwitchId::Mgt);
        process_without_step(
            sim_testbench,
            mgt::Request::ApplyTorque {
                duration,
                dipole: mgt::Dipole { x: 1, y: 2, z: 3 },
            },
        );
        assert_eq!(read_mgm_0_field(sim_testbench), mgm::MGT_GEN_MAGNETIC_FIELD);
    }

    #[test]
    fn test_mgm_field_cleared_after_torquing() {
        let mut sim_testbench = SimTestbench::new();
        start_torquing(&mut sim_testbench, Duration::from_millis(100));
        sim_testbench
            .step_until(Duration::from_millis(100))
            .unwrap();
        assert_ne!(
            read_mgm_0_field(&mut sim_testbench),
            mgm::MGT_GEN_MAGNETIC_FIELD
        );
    }

    #[test]
    fn test_mgm_field_cleared_by_switching_mgt_off() {
        let mut sim_testbench = SimTestbench::new();
        start_torquing(&mut sim_testbench, Duration::from_millis(100));
        process_without_step(
            &mut sim_testbench,
            PcduRequest::SwitchDevice {
                switch: SwitchId::Mgt,
                state: SwitchStateBinary::Off,
            },
        );
        assert_ne!(
            read_mgm_0_field(&mut sim_testbench),
            mgm::MGT_GEN_MAGNETIC_FIELD
        );
    }
}
