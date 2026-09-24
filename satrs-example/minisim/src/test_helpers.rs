use delegate::delegate;
use std::sync::mpsc;

use nexosim::{
    simulation::ExecutionError,
    time::{Deadline, MonotonicTime},
};
use satrs_minisim::{SimReply, SimRequest, SimRequestWithTime};

use crate::controller::{SimController, ThreadingModel};

pub struct SimTestbench {
    pub sim_controller: SimController,
    pub reply_receiver: mpsc::Receiver<SimReply>,
    pub request_sender: mpsc::Sender<SimRequestWithTime>,
}

impl SimTestbench {
    pub fn new() -> Self {
        let (request_sender, request_receiver) = mpsc::channel();
        let (reply_sender, reply_receiver) = mpsc::channel();
        let t0 = MonotonicTime::EPOCH;
        let sim_ctrl =
            SimController::new(ThreadingModel::Single, t0, reply_sender, request_receiver);

        Self {
            sim_controller: sim_ctrl,
            reply_receiver,
            request_sender,
        }
    }
    pub fn handle_sim_requests_time_agnostic(&mut self) {
        self.handle_sim_requests(MonotonicTime::EPOCH);
    }

    delegate! {
        to self.sim_controller {
            pub fn handle_sim_requests(&mut self, old_timestamp: MonotonicTime);
        }
        to self.sim_controller.simulation {
            pub fn step(&mut self) -> Result<(), ExecutionError>;
            pub fn step_until(&mut self, duration: impl Deadline) -> Result<(), ExecutionError>;
        }
    }

    pub fn send_request(
        &self,
        request: SimRequestWithTime,
    ) -> Result<(), mpsc::SendError<SimRequestWithTime>> {
        self.request_sender.send(request)
    }

    /// Sends the request and steps the simulation to the next scheduled event.
    pub fn send_and_step(&mut self, request: impl Into<SimRequest>) {
        self.send_request(SimRequestWithTime::new_with_epoch_time(request))
            .expect("sending request failed");
        self.handle_sim_requests_time_agnostic();
        self.step().unwrap();
    }

    pub fn request_reply(&mut self, request: impl Into<SimRequest>) -> Option<SimReply> {
        self.send_and_step(request);
        self.try_receive_next_reply()
    }

    pub fn try_receive_next_reply(&self) -> Option<SimReply> {
        match self.reply_receiver.try_recv() {
            Ok(reply) => Some(reply),
            Err(e) => {
                if e == mpsc::TryRecvError::Empty {
                    None
                } else {
                    panic!("reply_receiver disconnected");
                }
            }
        }
    }
}
