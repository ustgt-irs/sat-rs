use std::{
    sync::mpsc,
    time::{Duration, SystemTime},
};

use nexosim::{
    simulation::{Address, Mailbox, SimInit, Simulation},
    time::{Clock, MonotonicTime, SystemClock},
};
use satrs_minisim::{
    acs::{mgm, mgt},
    eps::PcduRequest,
    SimCtrlReply, SimCtrlRequest, SimReply, SimRequest, SimRequestWithTime,
};

use crate::{
    acs::{mgm::MgmModel, mgt::MgtModel},
    eps::PcduModel,
};

const WARNING_FOR_STALE_DATA: bool = false;

const SIM_CTRL_REQ_WIRETAPPING: bool = false;
const MGM_REQ_WIRETAPPING: bool = false;
const PCDU_REQ_WIRETAPPING: bool = false;
const MGT_REQ_WIRETAPPING: bool = false;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ThreadingModel {
    Default = 0,
    Single = 1,
}

struct ModelAddresses {
    mgm_0: Address<MgmModel>,
    mgm_1: Address<MgmModel>,
    pcdu: Address<PcduModel>,
    mgt: Address<MgtModel>,
}

// The simulation controller processes requests and drives the simulation.
pub struct SimController {
    sys_clock: SystemClock,
    request_receiver: mpsc::Receiver<SimRequestWithTime>,
    reply_sender: mpsc::Sender<SimReply>,
    pub simulation: Simulation,
    addrs: ModelAddresses,
}

impl SimController {
    pub fn new(
        threading_model: ThreadingModel,
        start_time: MonotonicTime,
        reply_sender: mpsc::Sender<SimReply>,
        request_receiver: mpsc::Receiver<SimRequestWithTime>,
    ) -> Self {
        let mgm_0_model = MgmModel::new(mgm::Id::Mgm0, reply_sender.clone());
        let mgm_1_model = MgmModel::new(mgm::Id::Mgm1, reply_sender.clone());
        let mut pcdu_model = PcduModel::new(reply_sender.clone());
        let mut mgt_model = MgtModel::new(reply_sender.clone());

        let mgm_0_mailbox = Mailbox::new();
        let mgm_1_mailbox = Mailbox::new();
        let pcdu_mailbox = Mailbox::new();
        let mgt_mailbox = Mailbox::new();
        let addrs = ModelAddresses {
            mgm_0: mgm_0_mailbox.address(),
            mgm_1: mgm_1_mailbox.address(),
            pcdu: pcdu_mailbox.address(),
            mgt: mgt_mailbox.address(),
        };

        pcdu_model
            .mgm_0_switch
            .connect(MgmModel::switch_device, &addrs.mgm_0);
        pcdu_model
            .mgm_1_switch
            .connect(MgmModel::switch_device, &addrs.mgm_1);
        pcdu_model
            .mgt_switch
            .connect(MgtModel::switch_device, &addrs.mgt);
        mgt_model
            .gen_magnetic_field
            .connect(MgmModel::apply_external_magnetic_field, &addrs.mgm_0);
        mgt_model
            .gen_magnetic_field
            .connect(MgmModel::apply_external_magnetic_field, &addrs.mgm_1);
        mgt_model
            .clear_magnetic_field
            .connect(MgmModel::clear_external_magnetic_field, &addrs.mgm_0);
        mgt_model
            .clear_magnetic_field
            .connect(MgmModel::clear_external_magnetic_field, &addrs.mgm_1);

        let sim_init = if threading_model == ThreadingModel::Single {
            SimInit::with_num_threads(1)
        } else {
            SimInit::new()
        };
        let (simulation, _scheduler) = sim_init
            .add_model(mgm_0_model, mgm_0_mailbox, "MGM 0 model")
            .add_model(mgm_1_model, mgm_1_mailbox, "MGM 1 model")
            .add_model(pcdu_model, pcdu_mailbox, "PCDU model")
            .add_model(mgt_model, mgt_mailbox, "MGT model")
            .init(start_time)
            .unwrap();
        Self {
            sys_clock: SystemClock::from_system_time(start_time, SystemTime::now()),
            request_receiver,
            reply_sender,
            simulation,
            addrs,
        }
    }

    pub fn run(&mut self, start_time: MonotonicTime, udp_polling_interval_ms: u64) {
        let mut t = start_time;
        loop {
            let t_old = t;
            // Check for UDP requests every millisecond. Shift the simulator ahead here to prevent
            // replies lying in the past.
            t += Duration::from_millis(udp_polling_interval_ms);
            let _synch_status = self.sys_clock.synchronize(t);
            self.handle_sim_requests(t_old);
            self.simulation
                .step_until(t)
                .expect("simulation step failed");
        }
    }

    pub fn handle_sim_requests(&mut self, old_timestamp: MonotonicTime) {
        loop {
            match self.request_receiver.try_recv() {
                Ok(request) => {
                    if request.timestamp < old_timestamp && WARNING_FOR_STALE_DATA {
                        log::warn!("stale data with timestamp {:?} received", request.timestamp);
                    }
                    match request.request {
                        SimRequest::SimCtrl(request) => self.handle_ctrl_request(request),
                        SimRequest::Mgm { id, request } => self.handle_mgm_request(id, request),
                        SimRequest::Mgt(request) => self.handle_mgt_request(request),
                        SimRequest::Pcdu(request) => self.handle_pcdu_request(request),
                    }
                }
                Err(e) => match e {
                    mpsc::TryRecvError::Empty => break,
                    mpsc::TryRecvError::Disconnected => {
                        panic!("all request sender disconnected")
                    }
                },
            }
        }
    }

    fn handle_ctrl_request(&mut self, sim_ctrl_request: SimCtrlRequest) {
        if SIM_CTRL_REQ_WIRETAPPING {
            log::info!("received sim ctrl request: {sim_ctrl_request:?}");
        }
        match sim_ctrl_request {
            SimCtrlRequest::Ping => {
                log::info!("received ping request, a client is connecting");
                self.reply_sender
                    .send(SimReply::from(SimCtrlReply::Pong))
                    .expect("sending reply from sim controller failed");
            }
        }
    }

    fn handle_mgm_request(&mut self, mgm_id: mgm::Id, mgm_request: mgm::Request) {
        let addr = match mgm_id {
            mgm::Id::Mgm0 => &self.addrs.mgm_0,
            mgm::Id::Mgm1 => &self.addrs.mgm_1,
        };
        if MGM_REQ_WIRETAPPING {
            log::info!("received {mgm_id:?} request: {mgm_request:?}");
        }
        match mgm_request {
            mgm::Request::RequestSensorData => {
                self.simulation
                    .process_event(MgmModel::send_sensor_values, (), addr)
                    .expect("event execution error for mgm");
            }
            mgm::Request::SetSpiFault(fault_mode) => {
                log::info!("{mgm_id:?}: setting SPI fault mode to {fault_mode:?}");
                self.simulation
                    .process_event(MgmModel::set_spi_fault, fault_mode, addr)
                    .expect("event execution error for mgm");
            }
        }
    }

    fn handle_pcdu_request(&mut self, pcdu_request: PcduRequest) {
        if PCDU_REQ_WIRETAPPING {
            log::info!("received PCDU request: {pcdu_request:?}");
        }
        match pcdu_request {
            PcduRequest::RequestSwitchInfo => {
                self.simulation
                    .process_event(PcduModel::request_switch_info, (), &self.addrs.pcdu)
                    .unwrap();
            }
            PcduRequest::SwitchDevice { switch, state } => {
                self.simulation
                    .process_event(PcduModel::switch_device, (switch, state), &self.addrs.pcdu)
                    .unwrap();
            }
        }
    }

    fn handle_mgt_request(&mut self, mgt_request: mgt::Request) {
        if MGT_REQ_WIRETAPPING {
            log::info!("received MGT request: {mgt_request:?}");
        }
        match mgt_request {
            mgt::Request::ApplyTorque { duration, dipole } => self
                .simulation
                .process_event(MgtModel::apply_torque, (duration, dipole), &self.addrs.mgt)
                .unwrap(),
            mgt::Request::RequestHk => self
                .simulation
                .process_event(MgtModel::request_housekeeping_data, (), &self.addrs.mgt)
                .unwrap(),
        };
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::SimTestbench;

    use super::*;

    #[test]
    fn test_basic_ping() {
        let mut sim_testbench = SimTestbench::new();
        assert_eq!(
            sim_testbench.request_reply(SimCtrlRequest::Ping),
            Some(SimReply::SimCtrl(SimCtrlReply::Pong))
        );
    }
}
