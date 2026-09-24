use std::{sync::mpsc, time::Duration};

use nexosim::{
    simulation::{Address, Scheduler, Simulation},
    time::{Clock, MonotonicTime, SystemClock},
};
use satrs_minisim::{
    acs::{MgmRequestLis3Mdl, MgmRequestLis3MdlMgm0, MgmRequestLis3MdlMgm1, MgtRequest},
    eps::PcduRequest,
    SerializableSimMsgPayload, SimComponent, SimCtrlReply, SimCtrlRequest, SimMessageProvider,
    SimReply, SimRequest, SimRequestError,
};

use crate::{
    acs::{mgm::MagnetometerModel, mgt::MagnetorquerModel},
    eps::PcduModel,
};

const WARNING_FOR_STALE_DATA: bool = false;

const SIM_CTRL_REQ_WIRETAPPING: bool = false;
const MGM_REQ_WIRETAPPING: bool = false;
const PCDU_REQ_WIRETAPPING: bool = false;
const MGT_REQ_WIRETAPPING: bool = false;

pub struct ModelAddrWrapper {
    mgm_0_addr: Address<MagnetometerModel>,
    mgm_1_addr: Address<MagnetometerModel>,
    pcdu_addr: Address<PcduModel>,
    mgt_addr: Address<MagnetorquerModel>,
}

// The simulation controller processes requests and drives the simulation.
#[allow(dead_code)]
pub struct SimController {
    pub sys_clock: SystemClock,
    pub request_receiver: mpsc::Receiver<SimRequest>,
    pub reply_sender: mpsc::Sender<SimReply>,
    pub simulation: Simulation,
    pub scheduler: Scheduler,
    pub addr_wrapper: ModelAddrWrapper,
}

impl ModelAddrWrapper {
    pub fn new(
        mgm_0_addr: Address<MagnetometerModel>,
        mgm_1_addr: Address<MagnetometerModel>,
        pcdu_addr: Address<PcduModel>,
        mgt_addr: Address<MagnetorquerModel>,
    ) -> Self {
        Self {
            mgm_0_addr,
            mgm_1_addr,
            pcdu_addr,
            mgt_addr,
        }
    }
}
impl SimController {
    pub fn new(
        sys_clock: SystemClock,
        request_receiver: mpsc::Receiver<SimRequest>,
        reply_sender: mpsc::Sender<SimReply>,
        simulation: Simulation,
        scheduler: Scheduler,
        addr_wrapper: ModelAddrWrapper,
    ) -> Self {
        Self {
            sys_clock,
            request_receiver,
            reply_sender,
            simulation,
            scheduler,
            addr_wrapper,
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
                    if let Err(e) = match request.component() {
                        SimComponent::SimCtrl => self.handle_ctrl_request(&request),
                        SimComponent::Mgm0Lis3Mdl => self.handle_mgm_request(0, &request),
                        SimComponent::Mgm1Lis3Mdl => self.handle_mgm_request(1, &request),
                        SimComponent::Mgt => self.handle_mgt_request(&request),
                        SimComponent::Pcdu => self.handle_pcdu_request(&request),
                    } {
                        self.handle_invalid_request_with_valid_target(e, &request)
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

    fn handle_ctrl_request(&mut self, request: &SimRequest) -> Result<(), SimRequestError> {
        let sim_ctrl_request = SimCtrlRequest::from_sim_message(request)?;
        if SIM_CTRL_REQ_WIRETAPPING {
            log::info!("received sim ctrl request: {sim_ctrl_request:?}");
        }
        match sim_ctrl_request {
            SimCtrlRequest::Ping => {
                log::info!("received ping request, a client is connecting");
                self.reply_sender
                    .send(SimReply::new(&SimCtrlReply::Pong))
                    .expect("sending reply from sim controller failed");
            }
        }
        Ok(())
    }

    fn handle_mgm_request(
        &mut self,
        mgm_idx: usize,
        request: &SimRequest,
    ) -> Result<(), SimRequestError> {
        let (mgm_request, addr) = match mgm_idx {
            0 => (
                MgmRequestLis3MdlMgm0::from_sim_message(request)?.0,
                &self.addr_wrapper.mgm_0_addr,
            ),
            1 => (
                MgmRequestLis3MdlMgm1::from_sim_message(request)?.0,
                &self.addr_wrapper.mgm_1_addr,
            ),
            _ => panic!("invalid mgm index"),
        };
        if MGM_REQ_WIRETAPPING {
            log::info!("received MGM{mgm_idx} request: {mgm_request:?}");
        }
        match mgm_request {
            MgmRequestLis3Mdl::RequestSensorData => {
                self.simulation
                    .process_event(MagnetometerModel::send_sensor_values, (), addr)
                    .expect("event execution error for mgm");
            }
            MgmRequestLis3Mdl::SetSpiFault(fault_mode) => {
                log::info!("MGM{mgm_idx}: setting SPI fault mode to {fault_mode:?}");
                self.simulation
                    .process_event(MagnetometerModel::set_spi_fault, fault_mode, addr)
                    .expect("event execution error for mgm");
            }
        }
        Ok(())
    }

    fn handle_pcdu_request(&mut self, request: &SimRequest) -> Result<(), SimRequestError> {
        let pcdu_request = PcduRequest::from_sim_message(request)?;
        if PCDU_REQ_WIRETAPPING {
            log::info!("received PCDU request: {pcdu_request:?}");
        }
        match pcdu_request {
            PcduRequest::RequestSwitchInfo => {
                self.simulation
                    .process_event(
                        PcduModel::request_switch_info,
                        (),
                        &self.addr_wrapper.pcdu_addr,
                    )
                    .unwrap();
            }
            PcduRequest::SwitchDevice { switch, state } => {
                self.simulation
                    .process_event(
                        PcduModel::switch_device,
                        (switch, state),
                        &self.addr_wrapper.pcdu_addr,
                    )
                    .unwrap();
            }
        }
        Ok(())
    }

    fn handle_mgt_request(&mut self, request: &SimRequest) -> Result<(), SimRequestError> {
        let mgt_request = MgtRequest::from_sim_message(request)?;
        if MGT_REQ_WIRETAPPING {
            log::info!("received MGT request: {mgt_request:?}");
        }
        match mgt_request {
            MgtRequest::ApplyTorque { duration, dipole } => self
                .simulation
                .process_event(
                    MagnetorquerModel::apply_torque,
                    (duration, dipole),
                    &self.addr_wrapper.mgt_addr,
                )
                .unwrap(),
            MgtRequest::RequestHk => self
                .simulation
                .process_event(
                    MagnetorquerModel::request_housekeeping_data,
                    (),
                    &self.addr_wrapper.mgt_addr,
                )
                .unwrap(),
        };
        Ok(())
    }

    fn handle_invalid_request_with_valid_target(
        &self,
        error: SimRequestError,
        request: &SimRequest,
    ) {
        log::warn!(
            "received invalid {:?} request: {:?}",
            request.component(),
            error
        );
        self.reply_sender
            .send(SimReply::new(&SimCtrlReply::from(error)))
            .expect("sending reply from sim controller failed");
    }
}

#[cfg(test)]
mod tests {
    use crate::test_helpers::SimTestbench;

    use super::*;

    #[test]
    fn test_basic_ping() {
        let mut sim_testbench = SimTestbench::new();
        let request = SimRequest::new_with_epoch_time(SimCtrlRequest::Ping);
        sim_testbench
            .send_request(request)
            .expect("sending sim ctrl request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step().unwrap();
        let sim_reply = sim_testbench.try_receive_next_reply();
        assert!(sim_reply.is_some());
        let sim_reply = sim_reply.unwrap();
        assert_eq!(sim_reply.component(), SimComponent::SimCtrl);
        let reply = SimCtrlReply::from_sim_message(&sim_reply)
            .expect("failed to deserialize MGM sensor values");
        assert_eq!(reply, SimCtrlReply::Pong);
    }
}
