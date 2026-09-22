use std::{sync::mpsc, time::Duration};

use satrs::spacepackets::CcsdsPacketIdAndPsc;
use satrs_example::{ModeHelper, TmtcQueues};
use types::{
    ComponentId, DeviceMode,
    acs::mgm_assembly::{Mode, request, response},
};

use crate::ccsds::pack_ccsds_tm_packet_for_now;

pub struct ParentQueueHelper {
    pub request_rx: mpsc::Receiver<request::ModeRequest>,
    pub report_tx: mpsc::SyncSender<response::ModeResponse>,
}

/// Helper component for communication with a parent component, which is usually as assembly.
pub struct ChildrenQueueHelper {
    pub request_tx_queues: [mpsc::SyncSender<types::acs::mgm::request::ModeRequest>; 2],
    pub report_rx_queues: [mpsc::Receiver<types::acs::mgm::response::ModeResponse>; 2],
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TransitionState {
    #[default]
    Idle,
    AwaitingReplies,
}

#[derive(Debug, Default, Copy, Clone)]
pub struct MgmInfo {
    reply_received: bool,
    mode: Option<DeviceMode>,
}

/// MGM assembly component.
pub struct Assembly {
    mode_helper: ModeHelper<Mode, TransitionState>,
    /// This boolean is used for the distinction between transitions commanded by the parent
    /// or by ground, and transitions which were commanded autonomously as part of children
    /// mode keeping.
    mode_keeping_transition: bool,
    tmtc_queues: TmtcQueues,
    mgm_modes: [MgmInfo; 2],
    parent_queues: ParentQueueHelper,
    pub(crate) children_queues: ChildrenQueueHelper,
    event_tx: mpsc::SyncSender<types::acs::mgm_assembly::Event>,
}

impl Assembly {
    pub const ID: ComponentId = ComponentId::AcsMgmAssembly;

    pub fn new(
        parent_queues: ParentQueueHelper,
        children_queues: ChildrenQueueHelper,
        tmtc_queues: TmtcQueues,
        mode_timeout: Duration,
        event_tx: mpsc::SyncSender<types::acs::mgm_assembly::Event>,
    ) -> Self {
        Self {
            mode_helper: ModeHelper::new(Mode::NoModeKeeping, mode_timeout),
            mode_keeping_transition: false,
            tmtc_queues,
            mgm_modes: [MgmInfo::default(); 2],
            parent_queues,
            children_queues,
            event_tx,
        }
    }

    pub fn periodic_operation(&mut self) {
        self.handle_telecommands();
        self.handle_parent_mode_queue();
        self.handle_children_mode_queues();

        if self.mode_helper.transition_active() {
            self.handle_mode_transition();
        }
    }

    pub fn handle_telecommands(&mut self) {
        loop {
            match self.tmtc_queues.tc_rx.try_recv() {
                Ok(packet) => {
                    let tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&packet.sp_header);
                    match postcard::from_bytes::<types::acs::mgm_assembly::request::Request>(
                        &packet.payload,
                    ) {
                        Ok(request) => match request {
                            types::acs::mgm_assembly::request::Request::Ping => {
                                self.send_telemetry(Some(tc_id), response::Response::Ok)
                            }
                            types::acs::mgm_assembly::request::Request::Mode(request) => {
                                match request {
                                    request::ModeRequest::SetMode(assembly_mode) => {
                                        self.start_transition(false, assembly_mode, Some(tc_id))
                                    }
                                    request::ModeRequest::ReadMode => self.send_telemetry(
                                        Some(tc_id),
                                        response::Response::Mode(response::ModeResponse::Mode(
                                            self.mode(),
                                        )),
                                    ),
                                }
                            }
                        },
                        Err(e) => {
                            log::warn!("failed to deserialize request: {}", e);
                        }
                    }
                }
                Err(e) => match e {
                    mpsc::TryRecvError::Empty => break,
                    mpsc::TryRecvError::Disconnected => log::warn!("packet sender disconnected"),
                },
            }
        }
    }

    pub fn send_telemetry(
        &self,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        response: types::acs::mgm_assembly::response::Response,
    ) {
        match pack_ccsds_tm_packet_for_now(Self::ID, tc_id, &response) {
            Ok(packet) => {
                if let Err(e) = self.tmtc_queues.tm_tx.send(packet) {
                    log::warn!("failed to send TM packet: {}", e);
                }
            }
            Err(e) => {
                log::warn!("failed to pack TM packet: {}", e);
            }
        }
    }

    pub fn handle_parent_mode_queue(&mut self) {
        loop {
            match self.parent_queues.request_rx.try_recv() {
                Ok(request) => match request {
                    request::ModeRequest::SetMode(assembly_mode) => match assembly_mode {
                        Mode::Device(_device_mode) => {
                            self.start_transition(false, assembly_mode, None);
                        }
                        Mode::NoModeKeeping => {
                            self.mode_helper.current = Mode::NoModeKeeping;
                        }
                    },
                    request::ModeRequest::ReadMode => self
                        .parent_queues
                        .report_tx
                        .send(response::ModeResponse::Mode(self.mode_helper.current))
                        .unwrap(),
                },
                Err(e) => match e {
                    mpsc::TryRecvError::Empty => break,
                    mpsc::TryRecvError::Disconnected => {
                        log::warn!("packet sender disconnected")
                    }
                },
            }
        }
    }

    pub fn handle_children_mode_queues(&mut self) {
        let mut mode_report_received = false;
        for (idx, rx) in self.children_queues.report_rx_queues.iter_mut().enumerate() {
            loop {
                match rx.try_recv() {
                    Ok(report) => match report {
                        types::acs::mgm::response::ModeResponse::Mode(device_mode) => {
                            self.mgm_modes[idx].mode = Some(device_mode);
                            self.mgm_modes[idx].reply_received = true;
                            mode_report_received = true;
                        }
                        types::acs::mgm::response::ModeResponse::SetModeTimeout => {
                            // Ignore, handle this with our own timeout.
                            log::warn!("MGM {} mode timeout", idx);
                        }
                    },
                    Err(e) => match e {
                        mpsc::TryRecvError::Empty => break,
                        mpsc::TryRecvError::Disconnected => {
                            log::warn!("packet sender disconnected")
                        }
                    },
                }
            }
        }
        if !mode_report_received {
            return;
        }

        // Transition is active, check for completion.
        if self.mode_helper.transition_active()
            && self.mgm_modes.iter().all(|i| i.reply_received)
            && let Mode::Device(device_mode) = self.mode_helper.target.unwrap()
        {
            // If at least one child reached the correct mode, we are done.
            if self.mgm_modes.iter().any(|i| i.mode == Some(device_mode)) {
                self.handle_mode_reached(true);
            } else {
                let report = if self.mode_keeping_transition {
                    response::ModeResponse::CanNotKeepMode(self.mgm_modes.map(|info| info.mode))
                } else {
                    response::ModeResponse::WrongMode(self.mgm_modes.map(|info| info.mode))
                };
                self.handle_mode_transition_failure(report);
            }
        }

        // Mode keeping active: Check children modes.
        if let Mode::Device(device_mode) = self.mode_helper.current
            && self
                .mgm_modes
                .iter()
                .all(|info| info.mode != Some(device_mode))
        {
            // Children lost mode. Try to command them back to the correct
            // mode.
            self.start_transition(true, self.mode_helper.current, None);
        }
    }

    pub fn handle_mode_transition(&mut self) {
        if self.mode_helper.target.is_none() {
            self.handle_mode_reached(true);
            return;
        }
        let target = self.mode_helper.target.unwrap();
        let device_mode = match target {
            Mode::Device(device_mode) => device_mode,
            Mode::NoModeKeeping => {
                self.handle_mode_reached(true);
                return;
            }
        };
        if self.mode_helper.transition_state == TransitionState::Idle {
            self.command_children(device_mode);
            self.mode_helper.transition_state = TransitionState::AwaitingReplies;
        }
        if self.mode_helper.transition_state == TransitionState::AwaitingReplies
            && self.mode_helper.timed_out()
        {
            let report = if self.mode_keeping_transition {
                response::ModeResponse::CanNotKeepMode(self.mgm_modes.map(|info| info.mode))
            } else {
                response::ModeResponse::SetModeTimeout(self.mgm_modes.map(|info| info.mode))
            };
            self.handle_mode_transition_failure(report);
        }
    }

    pub fn handle_mode_reached(&mut self, success: bool) {
        let tc_commander = self.mode_helper.finish(success);
        self.announce_mode();
        if tc_commander.is_some() {
            self.send_telemetry(tc_commander, response::Response::Ok);
        }
        self.parent_queues
            .report_tx
            .send(response::ModeResponse::Mode(self.mode_helper.current))
            .unwrap();
    }

    pub fn handle_mode_transition_failure(&mut self, report: response::ModeResponse) {
        if self.mode_helper.tc_commander.is_some() {
            self.send_telemetry(
                self.mode_helper.tc_commander,
                response::Response::Mode(response::ModeResponse::SetModeTimeout(
                    self.mgm_modes.map(|info| info.mode),
                )),
            );
        }
        self.parent_queues.report_tx.send(report).unwrap();
        self.mode_helper.finish(false);
    }

    pub fn command_children(&self, mode: DeviceMode) {
        for tx in &self.children_queues.request_tx_queues {
            tx.send(types::acs::mgm::request::ModeRequest::SetMode(mode))
                .unwrap();
        }
    }

    pub fn start_transition(
        &mut self,
        mode_keeping: bool,
        target: Mode,
        tc_id: Option<CcsdsPacketIdAndPsc>,
    ) {
        self.mode_keeping_transition = mode_keeping;
        self.mode_helper.tc_commander = tc_id;
        self.mgm_modes
            .iter_mut()
            .for_each(|m| m.reply_received = false);
        self.mode_helper.start(target);
    }

    fn announce_mode(&self) {
        log::info!(
            "{:?} announcing mode: {:?}",
            Self::ID,
            self.mode_helper.current
        );
        if let Err(e) = self
            .event_tx
            .send(types::acs::mgm_assembly::Event::ModeChanged(
                self.mode_helper.current,
            ))
        {
            log::warn!("{:?}: failed to send mode changed event: {}", Self::ID, e);
        }
    }

    #[inline]
    pub fn mode(&self) -> Mode {
        self.mode_helper.current
    }

    #[inline]
    #[cfg(test)]
    fn mode_transition_active(&self) -> bool {
        self.mode_helper.transition_active()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::TryRecvError;

    use arbitrary_int::u11;
    use satrs::spacepackets::SpacePacketHeader;
    use types::{
        Apid, Message, MessageType, TcHeader,
        acs::mgm_assembly,
        ccsds::{CcsdsTcPacketOwned, CcsdsTmPacketOwned},
    };

    use super::*;

    pub struct Testbench {
        subsystem_req_tx: mpsc::SyncSender<request::ModeRequest>,
        subsystem_report_rx: mpsc::Receiver<response::ModeResponse>,
        mgm_request_rx: [mpsc::Receiver<types::acs::mgm::request::ModeRequest>; 2],
        mgm_report_tx: [mpsc::SyncSender<types::acs::mgm::response::ModeResponse>; 2],
        tc_tx: mpsc::SyncSender<CcsdsTcPacketOwned>,
        tm_rx: mpsc::Receiver<CcsdsTmPacketOwned>,
        event_rx: mpsc::Receiver<mgm_assembly::Event>,
        assembly: Assembly,
    }

    impl Testbench {
        pub fn new() -> Self {
            let (subsystem_req_tx, subsystem_req_rx) = mpsc::sync_channel(5);
            let (subsystem_report_tx, subsystem_report_rx) = mpsc::sync_channel(5);

            let (mgm_0_mode_request_tx, mgm_0_mode_request_rx) = mpsc::sync_channel(5);
            let (mgm_1_mode_request_tx, mgm_1_mode_request_rx) = mpsc::sync_channel(5);
            let (mgm_0_mode_report_tx, mgm_0_mode_report_rx) = mpsc::sync_channel(5);
            let (mgm_1_mode_report_tx, mgm_1_mode_report_rx) = mpsc::sync_channel(5);

            let (tc_tx, tc_rx) = mpsc::sync_channel(5);
            let (tm_tx, tm_rx) = mpsc::sync_channel(5);
            let (event_tx, event_rx) = mpsc::sync_channel(5);

            Self {
                subsystem_req_tx,
                subsystem_report_rx,
                mgm_request_rx: [mgm_0_mode_request_rx, mgm_1_mode_request_rx],
                mgm_report_tx: [mgm_0_mode_report_tx, mgm_1_mode_report_tx],
                tc_tx,
                tm_rx,
                event_rx,
                assembly: Assembly::new(
                    ParentQueueHelper {
                        request_rx: subsystem_req_rx,
                        report_tx: subsystem_report_tx,
                    },
                    ChildrenQueueHelper {
                        request_tx_queues: [mgm_0_mode_request_tx, mgm_1_mode_request_tx],
                        report_rx_queues: [mgm_0_mode_report_rx, mgm_1_mode_report_rx],
                    },
                    TmtcQueues { tc_rx, tm_tx },
                    Duration::from_millis(20),
                    event_tx,
                ),
            }
        }

        pub fn assert_all_queues_empty(&self) {
            assert!(
                matches!(self.tm_rx.try_recv().unwrap_err(), TryRecvError::Empty),
                "TM queue not empty"
            );
            assert!(
                matches!(
                    self.subsystem_report_rx.try_recv().unwrap_err(),
                    TryRecvError::Empty
                ),
                "subsystem report queue not empty"
            );
            for rx in self.mgm_request_rx.iter() {
                assert!(
                    matches!(rx.try_recv().unwrap_err(), TryRecvError::Empty),
                    "mgm request queue not empty"
                )
            }
        }
    }

    pub fn create_request_tc(
        request: types::acs::mgm_assembly::request::Request,
    ) -> types::ccsds::CcsdsTcPacketOwned {
        types::ccsds::CcsdsTcPacketOwned::new_with_request(
            SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
            TcHeader::new(Assembly::ID, request.message_type()),
            request,
        )
    }

    #[test]
    fn basic_test() {
        let mut tb = Testbench::new();
        tb.assert_all_queues_empty();
        tb.assembly.periodic_operation();
        tb.assert_all_queues_empty();
        assert_eq!(tb.assembly.mode(), Mode::NoModeKeeping);
    }

    #[test]
    fn test_tc_commanded_transition() {
        let mut tb = Testbench::new();
        tb.tc_tx
            .send(create_request_tc(mgm_assembly::request::Request::Mode(
                request::ModeRequest::SetMode(Mode::Device(DeviceMode::Normal)),
            )))
            .unwrap();
        tb.assembly.periodic_operation();
        assert!(tb.assembly.mode_transition_active());

        for rx in tb.mgm_request_rx.iter() {
            let request = rx.try_recv().unwrap();
            assert_eq!(
                request,
                types::acs::mgm::request::ModeRequest::SetMode(DeviceMode::Normal)
            );
        }

        // Confirm the mode is set.
        for tx in tb.mgm_report_tx.iter() {
            tx.send(types::acs::mgm::response::ModeResponse::Mode(
                DeviceMode::Normal,
            ))
            .unwrap();
        }

        tb.assembly.periodic_operation();
        assert!(!tb.assembly.mode_transition_active());
        assert_eq!(tb.assembly.mode(), Mode::Device(DeviceMode::Normal));

        let response = tb.tm_rx.try_recv().unwrap();
        assert_eq!(response.tm_header.sender_id, Assembly::ID);
        assert_eq!(response.tm_header.message_type, MessageType::Verification);
        let response: response::Response = postcard::from_bytes(&response.payload).unwrap();
        assert_eq!(response, response::Response::Ok);

        let event = tb.event_rx.try_recv().expect("expected mode changed event");
        assert!(matches!(
            event,
            mgm_assembly::Event::ModeChanged(Mode::Device(DeviceMode::Normal))
        ));
    }

    #[test]
    fn test_parent_commanded_transition() {
        let mut tb = Testbench::new();
        tb.subsystem_req_tx
            .send(request::ModeRequest::SetMode(Mode::Device(
                DeviceMode::Normal,
            )))
            .unwrap();
        tb.assembly.periodic_operation();
        assert!(tb.assembly.mode_transition_active());

        for rx in tb.mgm_request_rx.iter() {
            let request = rx.try_recv().unwrap();
            assert_eq!(
                request,
                types::acs::mgm::request::ModeRequest::SetMode(DeviceMode::Normal)
            );
        }

        // Confirm the mode is set.
        for tx in tb.mgm_report_tx.iter() {
            tx.send(types::acs::mgm::response::ModeResponse::Mode(
                DeviceMode::Normal,
            ))
            .unwrap();
        }

        tb.assembly.periodic_operation();
        assert!(!tb.assembly.mode_transition_active());
        assert_eq!(tb.assembly.mode(), Mode::Device(DeviceMode::Normal));

        let report = tb.subsystem_report_rx.try_recv().unwrap();
        assert_eq!(
            report,
            response::ModeResponse::Mode(Mode::Device(DeviceMode::Normal))
        );
    }

    #[test]
    fn test_one_mgm_is_sufficient() {
        let mut tb = Testbench::new();
        tb.subsystem_req_tx
            .send(request::ModeRequest::SetMode(Mode::Device(
                DeviceMode::Normal,
            )))
            .unwrap();
        tb.assembly.periodic_operation();
        assert!(tb.assembly.mode_transition_active());

        for rx in tb.mgm_request_rx.iter() {
            let request = rx.try_recv().unwrap();
            assert_eq!(
                request,
                types::acs::mgm::request::ModeRequest::SetMode(DeviceMode::Normal)
            );
        }

        // One device is sufficient.
        tb.mgm_report_tx[0]
            .send(types::acs::mgm::response::ModeResponse::Mode(
                DeviceMode::Normal,
            ))
            .unwrap();
        tb.mgm_report_tx[1]
            .send(types::acs::mgm::response::ModeResponse::Mode(
                DeviceMode::Off,
            ))
            .unwrap();

        tb.assembly.periodic_operation();
        assert!(!tb.assembly.mode_transition_active());
        assert_eq!(tb.assembly.mode(), Mode::Device(DeviceMode::Normal));

        let report = tb.subsystem_report_rx.try_recv().unwrap();
        assert_eq!(
            report,
            response::ModeResponse::Mode(Mode::Device(DeviceMode::Normal))
        );
    }

    #[test]
    fn test_mode_commanding_fails() {
        let mut tb = Testbench::new();
        tb.subsystem_req_tx
            .send(request::ModeRequest::SetMode(Mode::Device(
                DeviceMode::Normal,
            )))
            .unwrap();
        tb.assembly.periodic_operation();
        assert!(tb.assembly.mode_transition_active());

        for rx in tb.mgm_request_rx.iter() {
            let request = rx.try_recv().unwrap();
            assert_eq!(
                request,
                types::acs::mgm::request::ModeRequest::SetMode(DeviceMode::Normal)
            );
        }

        // Confirm the mode is set.
        for tx in tb.mgm_report_tx.iter() {
            tx.send(types::acs::mgm::response::ModeResponse::Mode(
                DeviceMode::Off,
            ))
            .unwrap();
        }

        tb.assembly.periodic_operation();
        assert!(!tb.assembly.mode_transition_active());
        assert_eq!(tb.assembly.mode(), Mode::NoModeKeeping);

        let report = tb.subsystem_report_rx.try_recv().unwrap();
        assert_eq!(
            report,
            response::ModeResponse::WrongMode([Some(DeviceMode::Off), Some(DeviceMode::Off)])
        );
    }

    #[test]
    fn test_mode_keeping_fails() {
        let mut tb = Testbench::new();
        tb.subsystem_req_tx
            .send(request::ModeRequest::SetMode(Mode::Device(
                DeviceMode::Normal,
            )))
            .unwrap();
        tb.assembly.periodic_operation();
        assert!(tb.assembly.mode_transition_active());

        for rx in tb.mgm_request_rx.iter() {
            let request = rx.try_recv().unwrap();
            assert_eq!(
                request,
                types::acs::mgm::request::ModeRequest::SetMode(DeviceMode::Normal)
            );
        }

        // Confirm the mode is set.
        for tx in tb.mgm_report_tx.iter() {
            tx.send(types::acs::mgm::response::ModeResponse::Mode(
                DeviceMode::Normal,
            ))
            .unwrap();
        }

        tb.assembly.periodic_operation();
        assert!(!tb.assembly.mode_transition_active());
        assert_eq!(tb.assembly.mode(), Mode::Device(DeviceMode::Normal));

        let report = tb.subsystem_report_rx.try_recv().unwrap();
        assert_eq!(
            report,
            response::ModeResponse::Mode(Mode::Device(DeviceMode::Normal))
        );

        for tx in tb.mgm_report_tx.iter() {
            tx.send(types::acs::mgm::response::ModeResponse::Mode(
                DeviceMode::Off,
            ))
            .unwrap();
        }
        // This should start mode keeping.
        tb.assembly.periodic_operation();
        assert!(tb.assembly.mode_transition_active());

        for rx in tb.mgm_request_rx.iter() {
            let request = rx.try_recv().unwrap();
            assert_eq!(
                request,
                types::acs::mgm::request::ModeRequest::SetMode(DeviceMode::Normal)
            );
        }

        // Let the mode keeping fail.
        for tx in tb.mgm_report_tx.iter() {
            tx.send(types::acs::mgm::response::ModeResponse::Mode(
                DeviceMode::Off,
            ))
            .unwrap();
        }
        tb.assembly.periodic_operation();
        let report = tb.subsystem_report_rx.try_recv().unwrap();
        assert_eq!(
            report,
            response::ModeResponse::CanNotKeepMode([Some(DeviceMode::Off), Some(DeviceMode::Off)])
        );
    }
}
