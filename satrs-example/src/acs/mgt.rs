use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use satrs::fdir::{FaultCounterStd, RecoveryEvent};
use satrs::health::HealthTableMapSync;
use satrs::spacepackets::CcsdsPacketIdAndPsc;
use satrs_example::{HkHelperSingleSet, TmtcQueues};
use satrs_minisim::acs::mgt as sim_mgt;
use satrs_minisim::{SimReply, SimRequest, SimRequestWithTime};
use types::acs::mgt::{
    self, HkSet,
    request::{HealthRequest, ModeRequest, Request},
    response::{ModeResponse, Response},
};
use types::pcdu::SwitchId;
use types::{ComponentId, DeviceMode, HkRequestType};

use crate::ccsds::pack_ccsds_tm_packet_for_now;
use crate::device_fdir::{DeviceFdir, FdirEvent};
use crate::device_mode::{ModeTransitionEvent, SwitchAndModeHelper};
use crate::eps::PowerSwitchHelper;

// FDIR configuration for a stalled communication link. Chosen so a handful of transient
// timeouts or garbled frames are tolerated but a persistently unresponsive device is caught
// quickly.
pub const COMM_FAULT_THRESHOLD: u32 = 2;
pub const COMM_FAULT_DECREMENT_AFTER: Duration = Duration::from_secs(30);

/// Interface for ideal device which never fails.
#[derive(Default)]
pub struct DummyInterface {
    dipole: sim_mgt::Dipole,
    torque_end: Option<Instant>,
    replies: VecDeque<Vec<u8>>,
}

impl DummyInterface {
    fn send(&mut self, frame: &[u8]) {
        let Ok(request) = sim_mgt::Request::from_frame(frame) else {
            return;
        };
        let reply = match request {
            sim_mgt::Request::ApplyTorque { duration, dipole } => {
                self.dipole = dipole;
                self.torque_end = Some(Instant::now() + duration);
                sim_mgt::Reply::Ack
            }
            sim_mgt::Request::RequestHk => {
                let torquing = self.torque_end.is_some_and(|end| Instant::now() < end);
                sim_mgt::Reply::Hk(sim_mgt::HkSet {
                    dipole: if torquing {
                        self.dipole
                    } else {
                        sim_mgt::Dipole::default()
                    },
                    torquing,
                })
            }
        };
        self.replies.push_back(reply.to_frame());
    }
}

/// Records all sent frames and returns injected reply frames.
#[derive(Default)]
pub struct TestInterface {
    pub sent_frames: Vec<Vec<u8>>,
    pub replies: VecDeque<Vec<u8>>,
}

pub struct SimInterface {
    pub sim_request_tx: mpsc::Sender<SimRequestWithTime>,
    pub sim_reply_rx: mpsc::Receiver<SimReply>,
}

impl SimInterface {
    fn send(&mut self, frame: &[u8]) {
        if let Err(e) = self
            .sim_request_tx
            .send(SimRequestWithTime::new_with_epoch_time(SimRequest::Mgt(
                frame.to_vec(),
            )))
        {
            log::error!("failed to send MGT SIM request: {e}");
        }
    }

    fn try_recv(&mut self) -> Option<Vec<u8>> {
        let sim_reply = self.sim_reply_rx.try_recv().ok()?;
        match sim_reply {
            SimReply::Mgt(frame) => Some(frame),
            _ => {
                log::warn!("unexpected MGT SIM reply: {sim_reply:?}");
                None
            }
        }
    }
}

/// Frame based transport to the device. The handler implements the protocol on top of it.
pub enum MgtCommunication {
    Dummy(DummyInterface),
    Sim(SimInterface),
    #[allow(dead_code)]
    Test(TestInterface),
}

impl MgtCommunication {
    fn send(&mut self, frame: &[u8]) {
        match self {
            MgtCommunication::Dummy(dummy) => dummy.send(frame),
            MgtCommunication::Sim(sim) => sim.send(frame),
            MgtCommunication::Test(test) => test.sent_frames.push(frame.to_vec()),
        }
    }

    fn try_recv(&mut self) -> Option<Vec<u8>> {
        match self {
            MgtCommunication::Dummy(dummy) => dummy.replies.pop_front(),
            MgtCommunication::Sim(sim) => sim.try_recv(),
            MgtCommunication::Test(test) => test.replies.pop_front(),
        }
    }
}

/// Helper component for communication with a parent component, which is usually an assembly
/// or a subsystem.
pub struct ModeLeafHelper {
    pub request_rx: mpsc::Receiver<ModeRequest>,
    pub report_tx: mpsc::SyncSender<ModeResponse>,
}

/// Magnetorquer (MGT) device handler.
///
/// The device is powered through the PCDU and only accepts torque commands in normal mode.
/// In normal mode, the device HK is polled every cycle. The replies arrive asynchronously and
/// are cached as the HK set of the handler.
///
/// Communication is monitored for FDIR: a reply timeout or an undecodable frame counts as a
/// fault, which can trigger a power cycle recovery or mark the device faulty, same as the MGM
/// SPI fault handling.
pub struct MgtHandler {
    tmtc_queues: TmtcQueues,
    pub com: MgtCommunication,
    hk_set: HkSet,
    hk_helper: HkHelperSingleSet,
    switch_and_mode_helper: SwitchAndModeHelper<DeviceMode>,
    mode_leaf_helper: ModeLeafHelper,
    fdir: DeviceFdir,
    /// Set once HK is polled and cleared once a reply arrives. Still set by the time the next
    /// poll is due means the previous reply never arrived, which counts as a comm fault.
    awaiting_reply: bool,
    event_tx: mpsc::SyncSender<mgt::Event>,
}

impl MgtHandler {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tmtc_queues: TmtcQueues,
        switch_helper: PowerSwitchHelper,
        com: MgtCommunication,
        mode_leaf_helper: ModeLeafHelper,
        mode_timeout: Duration,
        health_table: HealthTableMapSync,
        event_tx: mpsc::SyncSender<mgt::Event>,
    ) -> Self {
        Self {
            tmtc_queues,
            com,
            hk_set: HkSet::default(),
            hk_helper: HkHelperSingleSet::new(false, Duration::from_millis(200)),
            switch_and_mode_helper: SwitchAndModeHelper::new(
                DeviceMode::Off,
                mode_timeout,
                switch_helper,
                SwitchId::Mgt,
            ),
            mode_leaf_helper,
            fdir: DeviceFdir::new(
                "MGT",
                ComponentId::AcsMgt,
                health_table,
                FaultCounterStd::new(COMM_FAULT_THRESHOLD, COMM_FAULT_DECREMENT_AFTER),
            ),
            awaiting_reply: false,
            event_tx,
        }
    }

    #[inline]
    pub fn mode(&self) -> DeviceMode {
        self.switch_and_mode_helper.mode()
    }

    pub fn periodic_operation(&mut self) {
        self.handle_telecommands();
        self.handle_mode_leaf_handling();

        self.fdir
            .periodic_operation(&mut self.switch_and_mode_helper);
        self.handle_fdir_events();

        if let Some(event) = self.switch_and_mode_helper.handle_mode_transition() {
            match event {
                ModeTransitionEvent::Reached(tc_commander) => {
                    self.handle_mode_reached(tc_commander)
                }
                ModeTransitionEvent::Failed(tc_commander) => {
                    self.handle_mode_transition_failure(tc_commander)
                }
                // The mode did not change for other components, so there is nothing to report.
                ModeTransitionEvent::PowerCycleDone => {
                    self.fdir.handle_power_cycle_done();
                    self.handle_fdir_events();
                }
                ModeTransitionEvent::PowerCycleFailed { restore_mode } => {
                    self.fdir
                        .handle_power_cycle_failed(&mut self.switch_and_mode_helper, restore_mode);
                    self.handle_fdir_events();
                }
            }
        }

        // Process replies received since the last call before deciding whether the previous
        // poll was answered in time.
        self.handle_replies();

        if self.ready_for_commanding() {
            self.poll_hk();
        } else {
            // Not polling, so a reply missed while off must not be flagged as a fault later.
            self.awaiting_reply = false;
        }

        if self.hk_helper.needs_generation() {
            self.send_telemetry(None, Response::Hk(self.hk_set));
        }
    }

    fn send_request(&mut self, request: sim_mgt::Request) {
        self.com.send(&request.to_frame());
    }

    /// Polls HK. If the reply to the previous poll never arrived, that is a comm fault.
    fn poll_hk(&mut self) {
        if self.awaiting_reply {
            log::warn!("MGT: no reply to previous poll");
            self.register_comm_fault();
        }
        self.awaiting_reply = true;
        self.send_request(sim_mgt::Request::RequestHk);
    }

    fn handle_replies(&mut self) {
        while let Some(frame) = self.com.try_recv() {
            match sim_mgt::Reply::from_frame(&frame) {
                Ok(sim_mgt::Reply::Hk(hk)) => {
                    self.hk_set = HkSet {
                        valid: true,
                        dipole: types::acs::mgt::Dipole {
                            x: hk.dipole.x,
                            y: hk.dipole.y,
                            z: hk.dipole.z,
                        },
                        torquing: hk.torquing,
                    };
                    self.register_comm_success();
                }
                Ok(sim_mgt::Reply::Ack) => self.register_comm_success(),
                Err(e) => {
                    log::warn!("MGT: invalid reply frame {frame:02x?}: {e}");
                    self.awaiting_reply = false;
                    self.register_comm_fault();
                }
            }
        }
    }

    fn register_comm_success(&mut self) {
        self.awaiting_reply = false;
        self.fdir.register_success();
    }

    fn register_comm_fault(&mut self) {
        self.hk_set.valid = false;
        self.fdir.register_fault(&mut self.switch_and_mode_helper);
        self.handle_fdir_events();
    }

    fn handle_fdir_events(&mut self) {
        while let Some(event) = self.fdir.next_event() {
            let event = match event {
                FdirEvent::FaultThresholdExceeded => mgt::Event::CommFaultThresholdExceeded,
                FdirEvent::Recovery(recovery_event) => {
                    // The device is power cycled or switched off.
                    if matches!(
                        recovery_event,
                        RecoveryEvent::Started | RecoveryEvent::ThresholdExceeded
                    ) {
                        self.hk_set.valid = false;
                    }
                    mgt::Event::Recovery(recovery_event)
                }
            };
            self.send_event(event);
        }
    }

    fn ready_for_commanding(&self) -> bool {
        self.mode() == DeviceMode::Normal && self.switch_and_mode_helper.target().is_none()
    }

    fn handle_telecommands(&mut self) {
        while let Ok(packet) = self.tmtc_queues.tc_rx.try_recv() {
            let tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&packet.sp_header);
            let request = match postcard::from_bytes::<Request>(&packet.payload) {
                Ok(request) => request,
                Err(e) => {
                    log::warn!("MGT: failed to deserialize request: {}", e);
                    continue;
                }
            };
            log::info!(
                "MGT: received request {:?} with TC ID {:#010x}",
                request,
                tc_id.raw()
            );
            match request {
                Request::Ping => self.send_telemetry(Some(tc_id), Response::Ok),
                Request::Hk(hk_request) => self.handle_hk_request(tc_id, hk_request),
                Request::Mode(ModeRequest::SetMode(mode)) => {
                    self.handle_mode_command(mode, Some(tc_id))
                }
                Request::Mode(ModeRequest::ReadMode) => self
                    .send_telemetry(Some(tc_id), Response::Mode(ModeResponse::Mode(self.mode()))),
                Request::Health(HealthRequest::SetHealth(health_state)) => {
                    log::info!(
                        "MGT: setting health to {:?} via ground command",
                        health_state
                    );
                    self.fdir.set_health(health_state);
                    self.send_telemetry(Some(tc_id), Response::Ok);
                }
                Request::ApplyTorque { dipole, duration } => {
                    self.handle_torque_command(tc_id, dipole, duration)
                }
            }
        }
    }

    fn handle_mode_leaf_handling(&mut self) {
        while let Ok(request) = self.mode_leaf_helper.request_rx.try_recv() {
            match request {
                ModeRequest::SetMode(mode) => self.handle_mode_command(mode, None),
                ModeRequest::ReadMode => self.report_mode_to_parent(),
            }
        }
    }

    fn handle_mode_command(
        &mut self,
        target_mode: DeviceMode,
        tc_commander: Option<CcsdsPacketIdAndPsc>,
    ) {
        self.fdir.handle_mode_command(&self.switch_and_mode_helper);
        self.start_transition(target_mode, tc_commander);
    }

    fn handle_hk_request(&mut self, tc_id: CcsdsPacketIdAndPsc, hk_request: HkRequestType) {
        match hk_request {
            HkRequestType::OneShot => self.send_telemetry(Some(tc_id), Response::Hk(self.hk_set)),
            HkRequestType::EnablePeriodic(opt_interval) => {
                self.hk_helper.enabled = true;
                if let Some(interval) = opt_interval {
                    self.hk_helper.frequency = interval;
                }
            }
            HkRequestType::DisablePeriodic => self.hk_helper.enabled = false,
            HkRequestType::ModifyInterval(interval) => self.hk_helper.frequency = interval,
            _ => log::warn!("MGT: unhandled HK request"),
        }
    }

    fn handle_torque_command(
        &mut self,
        tc_id: CcsdsPacketIdAndPsc,
        dipole: types::acs::mgt::Dipole,
        duration: Duration,
    ) {
        if !self.ready_for_commanding() {
            log::warn!("MGT: rejecting torque command, device not in normal mode");
            self.send_telemetry(Some(tc_id), Response::NotInNormalMode);
            return;
        }
        self.send_request(sim_mgt::Request::ApplyTorque {
            duration,
            dipole: sim_mgt::Dipole {
                x: dipole.x,
                y: dipole.y,
                z: dipole.z,
            },
        });
        self.send_telemetry(Some(tc_id), Response::Ok);
    }

    fn start_transition(
        &mut self,
        target_mode: DeviceMode,
        tc_commander: Option<CcsdsPacketIdAndPsc>,
    ) {
        log::info!("MGT: transitioning to mode {:?}", target_mode);
        if target_mode == DeviceMode::Off {
            self.hk_set = HkSet::default();
        }
        self.switch_and_mode_helper
            .start_transition(target_mode, tc_commander);
    }

    fn handle_mode_reached(&mut self, tc_commander: Option<CcsdsPacketIdAndPsc>) {
        log::info!("MGT: mode {:?} reached", self.mode());
        self.send_event(mgt::Event::ModeChanged(self.mode()));
        if tc_commander.is_some() {
            self.send_telemetry(tc_commander, Response::Ok);
        }
        self.report_mode_to_parent();
    }

    fn handle_mode_transition_failure(&mut self, tc_commander: Option<CcsdsPacketIdAndPsc>) {
        if tc_commander.is_some() {
            self.send_telemetry(tc_commander, Response::Mode(ModeResponse::SetModeTimeout));
        }
        self.mode_leaf_helper
            .report_tx
            .send(ModeResponse::SetModeTimeout)
            .unwrap();
    }

    fn report_mode_to_parent(&self) {
        self.mode_leaf_helper
            .report_tx
            .send(ModeResponse::Mode(self.mode()))
            .unwrap();
    }

    fn send_event(&self, event: mgt::Event) {
        if let Err(e) = self.event_tx.send(event) {
            log::warn!("MGT: failed to send event {:?}: {}", event, e);
        }
    }

    fn send_telemetry(&self, tc_id: Option<CcsdsPacketIdAndPsc>, response: Response) {
        match pack_ccsds_tm_packet_for_now(ComponentId::AcsMgt, tc_id, &response) {
            Ok(packet) => {
                if let Err(e) = self.tmtc_queues.tm_tx.send(packet) {
                    log::warn!("MGT: failed to send TM packet: {}", e);
                }
            }
            Err(e) => log::warn!("MGT: failed to pack TM packet: {}", e),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use arbitrary_int::u11;
    use satrs::health::{HealthState, HealthTableProvider};
    use satrs::spacepackets::SpacePacketHeader;
    use types::{
        Apid, Message as _, TcHeader,
        ccsds::{CcsdsTcPacketOwned, CcsdsTmPacketOwned},
        pcdu::{SwitchRequest, SwitchState, SwitchStateBinary},
    };

    use crate::device_fdir::RECOVERY_THRESHOLD;
    use crate::eps::pcdu::{SharedSwitchSet, SwitchMap, SwitchSet};

    use super::*;

    impl TestInterface {
        fn sent_requests(&self) -> Vec<sim_mgt::Request> {
            self.sent_frames
                .iter()
                .map(|frame| sim_mgt::Request::from_frame(frame).unwrap())
                .collect()
        }

        fn push_reply(&mut self, reply: sim_mgt::Reply) {
            self.replies.push_back(reply.to_frame());
        }
    }

    struct MgtTestbench {
        parent_request_tx: mpsc::SyncSender<ModeRequest>,
        parent_report_rx: mpsc::Receiver<ModeResponse>,
        shared_switch_set: SharedSwitchSet,
        switch_rx: mpsc::Receiver<SwitchRequest>,
        tc_tx: mpsc::SyncSender<CcsdsTcPacketOwned>,
        tm_rx: mpsc::Receiver<CcsdsTmPacketOwned>,
        event_rx: mpsc::Receiver<mgt::Event>,
        health_table: HealthTableMapSync,
        handler: MgtHandler,
    }

    impl MgtTestbench {
        fn new() -> Self {
            let (parent_request_tx, request_rx) = mpsc::sync_channel(5);
            let (report_tx, parent_report_rx) = mpsc::sync_channel(5);
            let (tc_tx, tc_rx) = mpsc::sync_channel(10);
            let (tm_tx, tm_rx) = mpsc::sync_channel(10);
            let (switch_tx, switch_rx) = mpsc::sync_channel(10);
            let (event_tx, event_rx) = mpsc::sync_channel(10);
            let mut switch_map = SwitchMap::new();
            switch_map.insert(SwitchId::Mgt, SwitchState::Off);
            let shared_switch_set = SharedSwitchSet::new(Mutex::new(SwitchSet::new(switch_map)));
            let health_table = HealthTableMapSync::default();
            let mut handler = MgtHandler::new(
                TmtcQueues { tc_rx, tm_tx },
                PowerSwitchHelper::new(switch_tx, shared_switch_set.clone()),
                MgtCommunication::Test(TestInterface::default()),
                ModeLeafHelper {
                    request_rx,
                    report_tx,
                },
                Duration::from_millis(100),
                health_table.clone(),
                event_tx,
            );
            handler.fdir.recovery_off_duration = Duration::ZERO;
            Self {
                parent_request_tx,
                parent_report_rx,
                shared_switch_set,
                switch_rx,
                tc_tx,
                tm_rx,
                event_rx,
                health_table,
                handler,
            }
        }

        fn send_tc(&self, request: Request) {
            self.tc_tx
                .send(CcsdsTcPacketOwned::new_with_request(
                    SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
                    TcHeader::new(ComponentId::AcsMgt, request.message_type()),
                    request,
                ))
                .unwrap();
        }

        fn next_response(&self) -> Response {
            let tm = self.tm_rx.try_recv().expect("no TM generated");
            assert_eq!(tm.tm_header.sender_id, ComponentId::AcsMgt);
            postcard::from_bytes(&tm.payload).expect("invalid MGT response")
        }

        /// Completes the power switch handshake for a commanded switch-on.
        fn switch_to_normal(&mut self) {
            self.send_tc(Request::Mode(ModeRequest::SetMode(DeviceMode::Normal)));
            self.handler.periodic_operation();
            self.shared_switch_set
                .lock()
                .unwrap()
                .set_switch_state(SwitchId::Mgt, SwitchState::On);
            self.handler.periodic_operation();
            assert_eq!(self.handler.mode(), DeviceMode::Normal);
            assert_eq!(self.next_response(), Response::Ok);
        }

        fn test_interface(&mut self) -> &mut TestInterface {
            match &mut self.handler.com {
                MgtCommunication::Test(test) => test,
                _ => panic!("unexpected MGT interface"),
            }
        }

        fn set_switch_state(&self, state: SwitchState) {
            self.shared_switch_set
                .lock()
                .unwrap()
                .set_switch_state(SwitchId::Mgt, state);
        }

        fn health(&self) -> Option<HealthState> {
            self.health_table.health(ComponentId::AcsMgt.into())
        }

        fn drain_events(&self) -> Vec<mgt::Event> {
            self.event_rx.try_iter().collect()
        }

        fn drain_switch_requests(&self) -> Vec<SwitchStateBinary> {
            self.switch_rx
                .try_iter()
                .map(|req| req.target_state)
                .collect()
        }

        /// Drives comm timeout faults until the fault threshold is exceeded once. No replies
        /// must be queued on the test interface for this to trigger. Assumes one poll is
        /// already outstanding, for example right after [Self::switch_to_normal].
        fn exceed_comm_fault_threshold(&mut self) {
            for _ in 0..COMM_FAULT_THRESHOLD + 1 {
                self.handler.periodic_operation();
            }
        }

        /// Drives a started power cycle recovery to completion, completing both power-switch
        /// handshakes.
        fn complete_power_cycle(&mut self) {
            self.handler.periodic_operation();
            self.set_switch_state(SwitchState::Off);
            self.handler.periodic_operation();
            assert_eq!(self.handler.mode(), DeviceMode::Off);
            self.handler.periodic_operation();
            assert_eq!(
                self.handler.switch_and_mode_helper.target(),
                Some(DeviceMode::Normal)
            );
            self.set_switch_state(SwitchState::On);
            self.handler.periodic_operation();
            assert_eq!(self.handler.mode(), DeviceMode::Normal);
        }

        /// Drives recoveries with a permanently stalled link until the component is marked
        /// faulty.
        fn recover_until_faulty(&mut self) {
            self.exceed_comm_fault_threshold();
            for _ in 0..RECOVERY_THRESHOLD {
                assert_eq!(self.health(), Some(HealthState::NeedsRecovery));
                self.complete_power_cycle();
                // The last cycle of the power cycle only starts a new poll, so the full
                // threshold is needed again to trip.
                for _ in 0..COMM_FAULT_THRESHOLD + 1 {
                    self.handler.periodic_operation();
                }
            }
            assert_eq!(self.health(), Some(HealthState::Faulty));
        }
    }

    #[test]
    fn test_initial_state_no_polling() {
        let mut testbench = MgtTestbench::new();
        testbench.handler.periodic_operation();
        assert_eq!(testbench.handler.mode(), DeviceMode::Off);
        assert!(testbench.test_interface().sent_frames.is_empty());
    }

    #[test]
    fn test_switch_to_normal() {
        let mut testbench = MgtTestbench::new();
        testbench.send_tc(Request::Mode(ModeRequest::SetMode(DeviceMode::Normal)));
        testbench.handler.periodic_operation();
        let switch_request = testbench.switch_rx.try_recv().expect("no switch request");
        assert_eq!(switch_request.switch_id, SwitchId::Mgt);
        assert_eq!(switch_request.target_state, SwitchStateBinary::On);
        assert_eq!(testbench.handler.mode(), DeviceMode::Off);

        testbench
            .shared_switch_set
            .lock()
            .unwrap()
            .set_switch_state(SwitchId::Mgt, SwitchState::On);
        testbench.handler.periodic_operation();
        assert_eq!(testbench.handler.mode(), DeviceMode::Normal);
        assert_eq!(testbench.next_response(), Response::Ok);
        assert!(matches!(
            testbench.event_rx.try_recv(),
            Ok(mgt::Event::ModeChanged(DeviceMode::Normal))
        ));
        assert_eq!(
            testbench.parent_report_rx.try_recv(),
            Ok(ModeResponse::Mode(DeviceMode::Normal))
        );
    }

    #[test]
    fn test_mode_command_from_parent() {
        let mut testbench = MgtTestbench::new();
        testbench
            .parent_request_tx
            .send(ModeRequest::SetMode(DeviceMode::Normal))
            .unwrap();
        testbench.handler.periodic_operation();
        testbench
            .shared_switch_set
            .lock()
            .unwrap()
            .set_switch_state(SwitchId::Mgt, SwitchState::On);
        testbench.handler.periodic_operation();
        assert_eq!(
            testbench.parent_report_rx.try_recv(),
            Ok(ModeResponse::Mode(DeviceMode::Normal))
        );
        // Commanded by the parent, so no TC response is expected.
        assert!(testbench.tm_rx.try_recv().is_err());
    }

    #[test]
    fn test_torque_command_rejected_when_off() {
        let mut testbench = MgtTestbench::new();
        testbench.send_tc(Request::ApplyTorque {
            dipole: mgt::Dipole { x: 1, y: 2, z: 3 },
            duration: Duration::from_millis(100),
        });
        testbench.handler.periodic_operation();
        assert_eq!(testbench.next_response(), Response::NotInNormalMode);
        assert!(testbench.test_interface().sent_frames.is_empty());
    }

    #[test]
    fn test_torque_command_forwarded_in_normal_mode() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench.test_interface().sent_frames.clear();

        testbench.send_tc(Request::ApplyTorque {
            dipole: mgt::Dipole { x: 1, y: 2, z: 3 },
            duration: Duration::from_millis(100),
        });
        testbench.handler.periodic_operation();
        assert_eq!(testbench.next_response(), Response::Ok);
        assert_eq!(
            testbench.test_interface().sent_requests().first(),
            Some(&sim_mgt::Request::ApplyTorque {
                duration: Duration::from_millis(100),
                dipole: sim_mgt::Dipole { x: 1, y: 2, z: 3 },
            })
        );
    }

    #[test]
    fn test_hk_polling_updates_hk_set() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        assert!(
            testbench
                .test_interface()
                .sent_requests()
                .contains(&sim_mgt::Request::RequestHk)
        );

        testbench
            .test_interface()
            .push_reply(sim_mgt::Reply::Hk(sim_mgt::HkSet {
                dipole: sim_mgt::Dipole { x: 1, y: 2, z: 3 },
                torquing: true,
            }));
        testbench.handler.periodic_operation();
        testbench.send_tc(Request::Hk(HkRequestType::OneShot));
        testbench.handler.periodic_operation();
        assert_eq!(
            testbench.next_response(),
            Response::Hk(HkSet {
                valid: true,
                dipole: mgt::Dipole { x: 1, y: 2, z: 3 },
                torquing: true,
            })
        );
    }

    #[test]
    fn test_hk_set_invalid_after_switch_off() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench
            .test_interface()
            .push_reply(sim_mgt::Reply::Hk(sim_mgt::HkSet {
                dipole: sim_mgt::Dipole::default(),
                torquing: false,
            }));
        testbench.handler.periodic_operation();

        testbench.send_tc(Request::Mode(ModeRequest::SetMode(DeviceMode::Off)));
        testbench.send_tc(Request::Hk(HkRequestType::OneShot));
        testbench.handler.periodic_operation();
        assert_eq!(testbench.next_response(), Response::Hk(HkSet::default()));
    }

    #[test]
    fn test_periodic_hk() {
        let mut testbench = MgtTestbench::new();
        testbench.send_tc(Request::Hk(HkRequestType::EnablePeriodic(Some(
            Duration::ZERO,
        ))));
        testbench.handler.periodic_operation();
        assert!(matches!(testbench.next_response(), Response::Hk(_)));
        let tm = testbench.tm_rx.try_recv();
        assert!(tm.is_err(), "only one HK packet per cycle expected");

        testbench.send_tc(Request::Hk(HkRequestType::DisablePeriodic));
        testbench.handler.periodic_operation();
        assert!(testbench.tm_rx.try_recv().is_err());
    }

    #[test]
    fn test_missing_replies_below_threshold_stay_healthy() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        // One missing reply should not be enough to trip COMM_FAULT_THRESHOLD.
        testbench.handler.periodic_operation();
        assert_eq!(testbench.health(), None);
        assert!(!testbench.handler.hk_set.valid);
    }

    #[test]
    fn test_missing_replies_above_threshold_starts_recovery() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench.drain_events();
        testbench.exceed_comm_fault_threshold();
        assert_eq!(testbench.health(), Some(HealthState::NeedsRecovery));
        assert!(!testbench.handler.hk_set.valid);
        let events = testbench.drain_events();
        assert!(matches!(
            events[..],
            [
                mgt::Event::CommFaultThresholdExceeded,
                mgt::Event::Recovery(RecoveryEvent::Started)
            ]
        ));
    }

    #[test]
    fn test_invalid_frame_counts_as_comm_fault() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench.drain_events();
        // Consumed by the outstanding HK poll from switch_to_normal, then two more invalid
        // frames to exceed the threshold.
        for _ in 0..COMM_FAULT_THRESHOLD + 1 {
            testbench.test_interface().replies.push_back(vec![0xff]);
            testbench.handler.periodic_operation();
        }
        assert_eq!(testbench.health(), Some(HealthState::NeedsRecovery));
    }

    #[test]
    fn test_recovery_power_cycles_device() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench.drain_events();
        testbench.drain_switch_requests();
        testbench.exceed_comm_fault_threshold();

        testbench.complete_power_cycle();

        assert_eq!(testbench.health(), Some(HealthState::Healthy));
        assert_eq!(
            testbench.drain_switch_requests(),
            [SwitchStateBinary::Off, SwitchStateBinary::On]
        );
        let events = testbench.drain_events();
        assert!(matches!(
            events[..],
            [
                mgt::Event::CommFaultThresholdExceeded,
                mgt::Event::Recovery(RecoveryEvent::Started),
                mgt::Event::Recovery(RecoveryEvent::Done),
            ]
        ));
    }

    #[test]
    fn test_repeated_recovery_marks_component_faulty() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench.recover_until_faulty();
        let events = testbench.drain_events();
        assert!(matches!(
            events[..],
            [
                ..,
                mgt::Event::CommFaultThresholdExceeded,
                mgt::Event::Recovery(RecoveryEvent::ThresholdExceeded)
            ]
        ));
        testbench.drain_switch_requests();

        testbench.handler.periodic_operation();
        assert_eq!(testbench.drain_switch_requests(), [SwitchStateBinary::Off]);
        testbench.set_switch_state(SwitchState::Off);
        testbench.handler.periodic_operation();
        assert_eq!(testbench.handler.mode(), DeviceMode::Off);
        assert_eq!(testbench.health(), Some(HealthState::Faulty));
    }

    #[test]
    fn test_ground_health_override_clears_faulty_state() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench.recover_until_faulty();
        testbench.set_switch_state(SwitchState::Off);
        testbench.handler.periodic_operation();

        testbench.send_tc(Request::Health(HealthRequest::SetHealth(
            HealthState::Healthy,
        )));
        testbench.handler.periodic_operation();
        assert_eq!(testbench.next_response(), Response::Ok);
        assert_eq!(testbench.health(), Some(HealthState::Healthy));
    }

    #[test]
    fn test_mode_command_aborts_recovery() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench.exceed_comm_fault_threshold();
        testbench.handler.periodic_operation();
        testbench.set_switch_state(SwitchState::Off);
        testbench
            .parent_request_tx
            .send(ModeRequest::SetMode(DeviceMode::Off))
            .unwrap();
        testbench.handler.periodic_operation();
        testbench.handler.periodic_operation();
        assert_eq!(testbench.handler.mode(), DeviceMode::Off);
        assert_eq!(testbench.handler.switch_and_mode_helper.target(), None);
        assert_eq!(testbench.health(), Some(HealthState::Healthy));
    }

    #[test]
    fn test_replies_arriving_in_time_stay_healthy() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        // A reply for the outstanding poll arrives before the next poll is sent, every cycle.
        for _ in 0..COMM_FAULT_THRESHOLD + 5 {
            testbench
                .test_interface()
                .push_reply(sim_mgt::Reply::Hk(sim_mgt::HkSet {
                    dipole: sim_mgt::Dipole::default(),
                    torquing: false,
                }));
            testbench.handler.periodic_operation();
        }
        assert_eq!(testbench.health(), None);
        assert!(testbench.handler.hk_set.valid);
    }
}
