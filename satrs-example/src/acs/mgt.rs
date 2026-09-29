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
    request::{ModeRequest, Request},
    response::{ModeResponse, Response},
};
use types::pcdu::SwitchId;
use types::{ComponentId, DeviceMode, HealthRequest, HkRequestType};

use crate::ccsds::pack_ccsds_tm_packet_for_now;
use crate::device_fdir::{DeviceFdir, FdirEvent};
use crate::device_mode::{ModeTransitionEvent, SwitchAndModeHelper};
use crate::eps::PowerSwitchHelper;

// The handler blocks while waiting for a reply, so this must be well below the cycle time
// of the ACS thread.
pub const REPLY_TIMEOUT: Duration = Duration::from_millis(50);
pub const REPLY_FAULT_THRESHOLD: u32 = 3;
pub const REPLY_FAULT_DECREMENT_AFTER: Duration = Duration::from_secs(30);

/// Interface for ideal device which never fails.
#[derive(Default)]
pub struct DummyInterface {
    dipole: mgt::Dipole,
    torque_end: Option<Instant>,
}

impl DummyInterface {
    fn transfer(&mut self, frame: &[u8]) -> Option<Vec<u8>> {
        let reply = match sim_mgt::Request::from_frame(frame).ok()? {
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
                        mgt::Dipole::default()
                    },
                    torquing,
                })
            }
        };
        Some(reply.to_frame())
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
    fn transfer(&mut self, frame: &[u8]) -> Option<Vec<u8>> {
        // Replies which arrived after a previous timeout must not be mistaken for this reply.
        while self.sim_reply_rx.try_recv().is_ok() {}
        if let Err(e) = self
            .sim_request_tx
            .send(SimRequestWithTime::new_with_epoch_time(SimRequest::Mgt(
                frame.to_vec(),
            )))
        {
            log::error!("failed to send MGT SIM request: {e}");
            return None;
        }
        match self.sim_reply_rx.recv_timeout(REPLY_TIMEOUT).ok()? {
            SimReply::Mgt(frame) => Some(frame),
            sim_reply => {
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
    /// Sends a request frame and blocks until the reply frame arrives. Returns [None] if there
    /// was no reply in time.
    fn transfer(&mut self, frame: &[u8]) -> Option<Vec<u8>> {
        match self {
            MgtCommunication::Dummy(dummy) => dummy.transfer(frame),
            MgtCommunication::Sim(sim) => sim.transfer(frame),
            MgtCommunication::Test(test) => {
                test.sent_frames.push(frame.to_vec());
                test.replies.pop_front()
            }
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
/// In normal mode, the device HK is polled every cycle and cached as the HK set of the handler.
/// Every request is answered with exactly one reply. A missing, invalid or unexpected reply is
/// a fault which is handled by the FDIR.
///
/// This device handler includes several components beyond the scope of commanding the device:
///
/// - The [MgtCommunication] structure models different communication interfaces to the
///   physical device.
/// - The device manages and commands its own power switch using the [SwitchAndModeHelper].
/// - The device FDIR is integrated directly into the device handler using the [DeviceFdir]
///   helper.
/// - Periodic HK is generated using the [HkHelperSingleSet] helper.
/// - The device is a mode leaf in the ACS tree and has a [ModeLeafHelper] for this.
pub struct MgtHandler {
    tmtc_queues: TmtcQueues,
    pub com: MgtCommunication,
    hk_set: HkSet,
    hk_helper: HkHelperSingleSet,
    switch_and_mode_helper: SwitchAndModeHelper<DeviceMode>,
    mode_leaf_helper: ModeLeafHelper,
    fdir: DeviceFdir,
    event_tx: mpsc::SyncSender<mgt::Event>,
}

impl MgtHandler {
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
                FaultCounterStd::new(REPLY_FAULT_THRESHOLD, REPLY_FAULT_DECREMENT_AFTER),
            ),
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

        if self.ready_for_commanding() {
            self.poll_hk();
        }

        if self.hk_helper.needs_generation() {
            self.send_telemetry(None, Response::Hk(self.hk_set));
        }
    }

    fn poll_hk(&mut self) {
        match self.transfer(sim_mgt::Request::RequestHk) {
            Some(sim_mgt::Reply::Hk(hk)) => {
                self.fdir.register_success();
                self.hk_set = HkSet {
                    valid: true,
                    dipole: hk.dipole,
                    torquing: hk.torquing,
                };
            }
            reply => self.register_reply_fault(reply),
        }
    }

    /// Returns [None] if there was no reply in time or the reply frame was invalid.
    fn transfer(&mut self, request: sim_mgt::Request) -> Option<sim_mgt::Reply> {
        let frame = self.com.transfer(&request.to_frame())?;
        sim_mgt::Reply::from_frame(&frame)
            .inspect_err(|e| log::warn!("MGT: invalid reply frame {frame:02x?}: {e}"))
            .ok()
    }

    fn register_reply_fault(&mut self, reply: Option<sim_mgt::Reply>) {
        log::warn!("MGT: missing or unexpected reply {reply:?}");
        self.hk_set.valid = false;
        self.fdir.register_fault(&mut self.switch_and_mode_helper);
        self.handle_fdir_events();
    }

    fn handle_fdir_events(&mut self) {
        while let Some(event) = self.fdir.next_event() {
            let event = match event {
                FdirEvent::FaultThresholdExceeded => mgt::Event::ReplyFaultThresholdExceeded,
                FdirEvent::Recovery(recovery_event) => {
                    // The device is power cycled or switched off.
                    if matches!(
                        recovery_event,
                        RecoveryEvent::Started | RecoveryEvent::ThresholdExceeded
                    ) {
                        self.hk_set = HkSet::default();
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
                    self.start_transition(mode, Some(tc_id))
                }
                Request::Mode(ModeRequest::ReadMode) => self.send_telemetry(
                    Some(tc_id),
                    Response::Mode(ModeResponse::Mode(
                        self.switch_and_mode_helper.reported_mode(),
                    )),
                ),
                Request::ApplyTorque { dipole, duration } => {
                    self.handle_torque_command(tc_id, dipole, duration)
                }
                Request::Health(HealthRequest::SetHealth(health)) => {
                    log::info!("MGT: setting health to {health:?} via ground command");
                    self.fdir.set_health(health);
                    self.send_telemetry(Some(tc_id), Response::Ok);
                }
            }
        }
    }

    fn handle_mode_leaf_handling(&mut self) {
        while let Ok(request) = self.mode_leaf_helper.request_rx.try_recv() {
            match request {
                ModeRequest::SetMode(mode) => self.start_transition(mode, None),
                ModeRequest::ReadMode => self.report_mode_to_parent(),
            }
        }
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
        dipole: mgt::Dipole,
        duration: Duration,
    ) {
        if !self.ready_for_commanding() {
            log::warn!("MGT: rejecting torque command, device not in normal mode");
            self.send_telemetry(Some(tc_id), Response::NotInNormalMode);
            return;
        }
        match self.transfer(sim_mgt::Request::ApplyTorque { duration, dipole }) {
            Some(sim_mgt::Reply::Ack) => {
                self.fdir.register_success();
                self.send_telemetry(Some(tc_id), Response::Ok);
            }
            reply => {
                self.register_reply_fault(reply);
                self.send_telemetry(Some(tc_id), Response::ReplyTimeout);
            }
        }
    }

    fn start_transition(
        &mut self,
        target_mode: DeviceMode,
        tc_commander: Option<CcsdsPacketIdAndPsc>,
    ) {
        log::info!("MGT: transitioning to mode {:?}", target_mode);
        self.fdir.handle_mode_command(&self.switch_and_mode_helper);
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
            .send(ModeResponse::Mode(
                self.switch_and_mode_helper.reported_mode(),
            ))
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
            let (event_tx, event_rx) = mpsc::sync_channel(20);
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
            self.set_switch_state(SwitchState::On);
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

        /// No replies are injected, so every HK poll is a fault. The poll in the cycle which
        /// reached normal mode already registered the first fault.
        fn exceed_reply_fault_threshold(&mut self) {
            for _ in 0..REPLY_FAULT_THRESHOLD {
                self.handler.periodic_operation();
            }
        }

        /// Drives a started power cycle recovery to completion, completing both power switch
        /// handshakes.
        fn complete_power_cycle(&mut self) {
            self.handler.periodic_operation();
            self.set_switch_state(SwitchState::Off);
            self.handler.periodic_operation();
            assert_eq!(self.handler.mode(), DeviceMode::Off);
            self.handler.periodic_operation();
            self.set_switch_state(SwitchState::On);
            self.handler.periodic_operation();
            assert_eq!(self.handler.mode(), DeviceMode::Normal);
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

        testbench.set_switch_state(SwitchState::On);
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
        testbench.set_switch_state(SwitchState::On);
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
        testbench.test_interface().push_reply(sim_mgt::Reply::Ack);

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
                dipole: mgt::Dipole { x: 1, y: 2, z: 3 },
            })
        );
    }

    #[test]
    fn test_torque_command_without_ack() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench.send_tc(Request::ApplyTorque {
            dipole: mgt::Dipole { x: 1, y: 2, z: 3 },
            duration: Duration::from_millis(100),
        });
        testbench.handler.periodic_operation();
        assert_eq!(testbench.next_response(), Response::ReplyTimeout);
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
                dipole: mgt::Dipole { x: 1, y: 2, z: 3 },
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
    fn test_switch_off() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench.drain_events();
        while testbench.parent_report_rx.try_recv().is_ok() {}

        testbench.send_tc(Request::Mode(ModeRequest::SetMode(DeviceMode::Off)));
        testbench.handler.periodic_operation();
        let switch_request = testbench
            .switch_rx
            .try_iter()
            .last()
            .expect("no switch request");
        assert_eq!(switch_request.target_state, SwitchStateBinary::Off);
        testbench.set_switch_state(SwitchState::Off);
        testbench.handler.periodic_operation();
        assert_eq!(testbench.handler.mode(), DeviceMode::Off);
        assert_eq!(testbench.next_response(), Response::Ok);
        assert!(matches!(
            testbench.drain_events().as_slice(),
            [mgt::Event::ModeChanged(DeviceMode::Off)]
        ));
        assert_eq!(
            testbench.parent_report_rx.try_recv(),
            Ok(ModeResponse::Mode(DeviceMode::Off))
        );
    }

    #[test]
    fn test_hk_set_invalid_after_switch_off() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench
            .test_interface()
            .push_reply(sim_mgt::Reply::Hk(sim_mgt::HkSet {
                dipole: mgt::Dipole::default(),
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
    fn test_valid_reply_no_fault() {
        let mut testbench = MgtTestbench::new();
        testbench
            .test_interface()
            .push_reply(sim_mgt::Reply::Hk(sim_mgt::HkSet {
                dipole: mgt::Dipole::default(),
                torquing: false,
            }));
        testbench.switch_to_normal();
        assert_eq!(testbench.handler.fdir.fault_count(), 0);
        assert!(testbench.handler.hk_set.valid);
    }

    #[test]
    fn test_missing_reply_registers_fault() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        assert_eq!(testbench.handler.fdir.fault_count(), 1);
        assert!(!testbench.handler.hk_set.valid);
        assert!(matches!(
            testbench.drain_events().as_slice(),
            [mgt::Event::ModeChanged(DeviceMode::Normal)]
        ));
    }

    #[test]
    fn test_unexpected_reply_registers_fault() {
        let mut testbench = MgtTestbench::new();
        testbench.test_interface().push_reply(sim_mgt::Reply::Ack);
        testbench.switch_to_normal();
        assert_eq!(testbench.handler.fdir.fault_count(), 1);
        assert!(matches!(
            testbench.drain_events().as_slice(),
            [mgt::Event::ModeChanged(DeviceMode::Normal)]
        ));
    }

    #[test]
    fn test_invalid_reply_registers_fault() {
        let mut testbench = MgtTestbench::new();
        testbench.test_interface().replies.push_back(vec![0xff]);
        testbench.switch_to_normal();
        assert_eq!(testbench.handler.fdir.fault_count(), 1);
        assert!(matches!(
            testbench.drain_events().as_slice(),
            [mgt::Event::ModeChanged(DeviceMode::Normal)]
        ));
    }

    #[test]
    fn test_reply_fault_threshold_starts_recovery() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench.drain_events();
        while testbench.parent_report_rx.try_recv().is_ok() {}
        testbench.exceed_reply_fault_threshold();
        assert_eq!(testbench.health(), Some(HealthState::NeedsRecovery));
        assert!(matches!(
            testbench.drain_events().as_slice(),
            [
                mgt::Event::ReplyFaultThresholdExceeded,
                mgt::Event::Recovery(RecoveryEvent::Started)
            ]
        ));

        testbench.complete_power_cycle();
        assert_eq!(testbench.health(), Some(HealthState::Healthy));
        assert!(matches!(
            testbench.drain_events().as_slice(),
            [mgt::Event::Recovery(RecoveryEvent::Done)]
        ));
        // The power cycle is hidden from the parent.
        assert!(testbench.parent_report_rx.try_recv().is_err());
    }

    #[test]
    fn test_unresponsive_device_marked_faulty() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        for _ in 0..RECOVERY_THRESHOLD {
            testbench.exceed_reply_fault_threshold();
            assert_eq!(testbench.health(), Some(HealthState::NeedsRecovery));
            testbench.complete_power_cycle();
        }
        testbench.drain_events();
        testbench.exceed_reply_fault_threshold();
        assert_eq!(testbench.health(), Some(HealthState::Faulty));
        assert!(matches!(
            testbench.drain_events().as_slice(),
            [
                mgt::Event::ReplyFaultThresholdExceeded,
                mgt::Event::Recovery(RecoveryEvent::ThresholdExceeded)
            ]
        ));
        assert_eq!(
            testbench.handler.switch_and_mode_helper.target(),
            Some(DeviceMode::Off)
        );
    }

    #[test]
    fn test_set_health() {
        let mut testbench = MgtTestbench::new();
        testbench.send_tc(Request::Health(HealthRequest::SetHealth(
            HealthState::Faulty,
        )));
        testbench.handler.periodic_operation();
        assert_eq!(testbench.next_response(), Response::Ok);
        assert_eq!(testbench.health(), Some(HealthState::Faulty));
    }
}
