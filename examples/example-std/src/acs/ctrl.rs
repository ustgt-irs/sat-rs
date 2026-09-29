use std::sync::mpsc::{Receiver, SyncSender, TryRecvError};

use types::acs::ctrl::{Mode, request::ModeRequest, response::ModeReport};

/// Helper component for communication with a parent component, which is usually an assembly
/// or a subsystem.
pub struct ModeLeafHelper {
    pub request_rx: Receiver<ModeRequest>,
    pub report_tx: SyncSender<ModeReport>,
}

/// Dummy ACS controller. It has no actual control law and reaches any commanded mode
/// immediately, which is sufficient to exercise the mode commanding of its parent subsystem.
pub struct Controller {
    mode: Mode,
    mode_leaf_helper: ModeLeafHelper,
}

impl Controller {
    pub fn new(mode_leaf_helper: ModeLeafHelper) -> Self {
        Self {
            mode: Mode::Passive,
            mode_leaf_helper,
        }
    }

    #[allow(dead_code)]
    #[inline]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn periodic_operation(&mut self) {
        self.handle_mode_leaf_handling();
    }

    fn handle_mode_leaf_handling(&mut self) {
        loop {
            match self.mode_leaf_helper.request_rx.try_recv() {
                Ok(request) => match request {
                    ModeRequest::SetMode(mode) => {
                        log::info!("ACS controller: transitioning to mode {:?}", mode);
                        self.mode = mode;
                        self.report_mode();
                    }
                    ModeRequest::ReadMode => self.report_mode(),
                },
                Err(e) => match e {
                    TryRecvError::Empty => break,
                    TryRecvError::Disconnected => log::warn!("packet sender disconnected"),
                },
            }
        }
    }

    fn report_mode(&self) {
        self.mode_leaf_helper
            .report_tx
            .send(ModeReport::Mode(self.mode))
            .expect("failed to send mode report to parent");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    struct ControllerTestbench {
        request_tx: SyncSender<ModeRequest>,
        report_rx: Receiver<ModeReport>,
        controller: Controller,
    }

    impl ControllerTestbench {
        fn new() -> Self {
            let (request_tx, request_rx) = mpsc::sync_channel(5);
            let (report_tx, report_rx) = mpsc::sync_channel(5);
            Self {
                request_tx,
                report_rx,
                controller: Controller::new(ModeLeafHelper {
                    request_rx,
                    report_tx,
                }),
            }
        }
    }

    #[test]
    fn test_initial_mode() {
        let testbench = ControllerTestbench::new();
        assert_eq!(testbench.controller.mode(), Mode::Passive);
    }

    #[test]
    fn test_set_mode() {
        let mut testbench = ControllerTestbench::new();
        testbench
            .request_tx
            .send(ModeRequest::SetMode(Mode::Safe))
            .unwrap();
        testbench.controller.periodic_operation();
        assert_eq!(testbench.controller.mode(), Mode::Safe);
        let report = testbench.report_rx.try_recv().expect("no mode report sent");
        assert_eq!(report, ModeReport::Mode(Mode::Safe));
    }

    #[test]
    fn test_read_mode() {
        let mut testbench = ControllerTestbench::new();
        testbench.request_tx.send(ModeRequest::ReadMode).unwrap();
        testbench.controller.periodic_operation();
        let report = testbench.report_rx.try_recv().expect("no mode report sent");
        assert_eq!(report, ModeReport::Mode(Mode::Passive));
    }
}
