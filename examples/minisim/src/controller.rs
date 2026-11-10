use std::{
    sync::mpsc,
    time::{Duration, SystemTime},
};

use minisim_types::{
    SimCtrlReply, SimCtrlRequest, SimReply, SimRequest, SimRequestWithTime,
    acs::{mgm, mtq},
    eps::PcduRequest,
};
use nexosim::{
    ports::{EventQueueReader, EventSinkReader, EventSource, SinkState, event_queue},
    simulation::{EventId, ExecutionError, Mailbox, SimInit, Simulation},
    time::{Clock, Deadline, MonotonicTime, SystemClock},
};
use types::pcdu::{SwitchId, SwitchStateBinary};

use crate::{
    acs::{mgm::MgmModel, mtq::MtqModel},
    eps::PcduModel,
};

const WARNING_FOR_STALE_DATA: bool = false;

const SIM_CTRL_REQ_WIRETAPPING: bool = false;
const MGM_REQ_WIRETAPPING: bool = false;
const PCDU_REQ_WIRETAPPING: bool = false;
const MTQ_REQ_WIRETAPPING: bool = false;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ThreadingModel {
    Default = 0,
    Single = 1,
}

struct MgmInputs {
    send_sensor_values: EventId<()>,
    set_spi_fault: EventId<mgm::SpiFault>,
}

impl MgmInputs {
    fn register(sim_init: &mut SimInit, mailbox: &Mailbox<MgmModel>) -> Self {
        Self {
            send_sensor_values: EventSource::new()
                .connect(MgmModel::send_sensor_values, mailbox)
                .register(sim_init),
            set_spi_fault: EventSource::new()
                .connect(MgmModel::set_spi_fault, mailbox)
                .register(sim_init),
        }
    }
}

/// Model inputs which are driven by simulation requests.
struct ModelInputs {
    mgm_0: MgmInputs,
    mgm_1: MgmInputs,
    pcdu_request_switch_info: EventId<u16>,
    pcdu_switch_device: EventId<(SwitchId, SwitchStateBinary)>,
    mtq_apply_torque: EventId<(Duration, mtq::Dipole)>,
    mtq_request_hk: EventId<()>,
}

// The simulation controller processes requests and drives the simulation.
pub struct SimController {
    sys_clock: SystemClock,
    request_receiver: mpsc::Receiver<SimRequestWithTime>,
    reply_sender: mpsc::Sender<SimReply>,
    simulation: Simulation,
    inputs: ModelInputs,
    model_replies: EventQueueReader<SimReply>,
}

impl SimController {
    pub fn new(
        threading_model: ThreadingModel,
        start_time: MonotonicTime,
        reply_sender: mpsc::Sender<SimReply>,
        request_receiver: mpsc::Receiver<SimRequestWithTime>,
    ) -> Self {
        let mut mgm_0_model = MgmModel::new(mgm::Id::Mgm0);
        let mut mgm_1_model = MgmModel::new(mgm::Id::Mgm1);
        let mut pcdu_model = PcduModel::new();
        let mut mtq_model = MtqModel::new();

        let mgm_0_mailbox = Mailbox::new();
        let mgm_1_mailbox = Mailbox::new();
        let pcdu_mailbox = Mailbox::new();
        let mtq_mailbox = Mailbox::new();

        pcdu_model
            .mgm_0_switch
            .connect(MgmModel::switch_device, &mgm_0_mailbox);
        pcdu_model
            .mgm_1_switch
            .connect(MgmModel::switch_device, &mgm_1_mailbox);
        pcdu_model
            .mtq_switch
            .connect(MtqModel::switch_device, &mtq_mailbox);
        mtq_model
            .gen_magnetic_field
            .connect(MgmModel::apply_external_magnetic_field, &mgm_0_mailbox);
        mtq_model
            .gen_magnetic_field
            .connect(MgmModel::apply_external_magnetic_field, &mgm_1_mailbox);
        mtq_model
            .clear_magnetic_field
            .connect(MgmModel::clear_external_magnetic_field, &mgm_0_mailbox);
        mtq_model
            .clear_magnetic_field
            .connect(MgmModel::clear_external_magnetic_field, &mgm_1_mailbox);

        let (reply_sink, model_replies) = event_queue(SinkState::Enabled);
        mgm_0_model.reply.connect_sink(reply_sink.clone());
        mgm_1_model.reply.connect_sink(reply_sink.clone());
        pcdu_model.reply.connect_sink(reply_sink.clone());
        mtq_model.reply.connect_sink(reply_sink);

        let mut sim_init = if threading_model == ThreadingModel::Single {
            SimInit::with_num_threads(1)
        } else {
            SimInit::new()
        };
        let inputs = ModelInputs {
            mgm_0: MgmInputs::register(&mut sim_init, &mgm_0_mailbox),
            mgm_1: MgmInputs::register(&mut sim_init, &mgm_1_mailbox),
            pcdu_request_switch_info: EventSource::new()
                .connect(PcduModel::request_switch_info, &pcdu_mailbox)
                .register(&mut sim_init),
            pcdu_switch_device: EventSource::new()
                .connect(PcduModel::switch_device, &pcdu_mailbox)
                .register(&mut sim_init),
            mtq_apply_torque: EventSource::new()
                .connect(MtqModel::apply_torque, &mtq_mailbox)
                .register(&mut sim_init),
            mtq_request_hk: EventSource::new()
                .connect(MtqModel::request_housekeeping_data, &mtq_mailbox)
                .register(&mut sim_init),
        };
        let simulation = sim_init
            .add_model(mgm_0_model, mgm_0_mailbox, "MGM 0 model")
            .add_model(mgm_1_model, mgm_1_mailbox, "MGM 1 model")
            .add_model(pcdu_model, pcdu_mailbox, "PCDU model")
            .add_model(mtq_model, mtq_mailbox, "MTQ model")
            .init(start_time)
            .unwrap();
        Self {
            sys_clock: SystemClock::from_system_time(start_time, SystemTime::now()),
            request_receiver,
            reply_sender,
            simulation,
            inputs,
            model_replies,
        }
    }

    #[cfg(test)]
    pub fn step(&mut self) -> Result<(), ExecutionError> {
        self.simulation.step()?;
        self.forward_model_replies();
        Ok(())
    }

    pub fn step_until(&mut self, deadline: impl Deadline) -> Result<(), ExecutionError> {
        self.simulation.step_until(deadline)?;
        self.forward_model_replies();
        Ok(())
    }

    fn forward_model_replies(&mut self) {
        while let Some(reply) = self.model_replies.try_read() {
            self.reply_sender
                .send(reply)
                .expect("sending model reply failed");
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
            self.step_until(t).expect("simulation step failed");
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
                        SimRequest::Mtq(request) => self.handle_mtq_request(request),
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
        self.forward_model_replies();
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
        let inputs = match mgm_id {
            mgm::Id::Mgm0 => &self.inputs.mgm_0,
            mgm::Id::Mgm1 => &self.inputs.mgm_1,
        };
        if MGM_REQ_WIRETAPPING {
            log::info!("received {mgm_id:?} request: {mgm_request:?}");
        }
        match mgm_request {
            mgm::Request::RequestSensorData => {
                self.simulation
                    .process_event(&inputs.send_sensor_values, ())
                    .expect("event execution error for mgm");
            }
            mgm::Request::SetSpiFault(fault_mode) => {
                log::info!("{mgm_id:?}: setting SPI fault mode to {fault_mode:?}");
                self.simulation
                    .process_event(&inputs.set_spi_fault, fault_mode)
                    .expect("event execution error for mgm");
            }
        }
    }

    fn handle_pcdu_request(&mut self, pcdu_request: PcduRequest) {
        if PCDU_REQ_WIRETAPPING {
            log::info!("received PCDU request: {pcdu_request:?}");
        }
        match pcdu_request {
            PcduRequest::RequestSwitchInfo { id } => {
                self.simulation
                    .process_event(&self.inputs.pcdu_request_switch_info, id)
                    .unwrap();
            }
            PcduRequest::SwitchDevice { switch, state } => {
                self.simulation
                    .process_event(&self.inputs.pcdu_switch_device, (switch, state))
                    .unwrap();
            }
        }
    }

    fn handle_mtq_request(&mut self, frame: Vec<u8>) {
        let mtq_request = match mtq::Request::from_frame(&frame) {
            Ok(request) => request,
            Err(e) => {
                log::warn!("dropping invalid MTQ frame {frame:02x?}: {e}");
                return;
            }
        };
        if MTQ_REQ_WIRETAPPING {
            log::info!("received MTQ request: {mtq_request:?}");
        }
        match mtq_request {
            mtq::Request::ApplyTorque { duration, dipole } => self
                .simulation
                .process_event(&self.inputs.mtq_apply_torque, (duration, dipole))
                .unwrap(),
            mtq::Request::RequestHk => self
                .simulation
                .process_event(&self.inputs.mtq_request_hk, ())
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
