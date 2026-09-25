use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use minisim_types::acs::mgt as sim_mgt;
use minisim_types::{SimReply, SimRequestWithTime};
use satrs::spacepackets::CcsdsPacketIdAndPsc;
use satrs_example::{HkHelperSingleSet, TmtcQueues};
use types::acs::mgt::{
    self, HkSet,
    request::{ModeRequest, Request},
    response::{ModeResponse, Response},
};
use types::pcdu::SwitchId;
use types::{ComponentId, DeviceMode, HkRequestType};

use crate::ccsds::pack_ccsds_tm_packet_for_now;
use crate::device_mode::{ModeTransitionEvent, SwitchAndModeHelper};
use crate::eps::PowerSwitchHelper;

/// Interface for ideal device which never fails.
#[derive(Default)]
pub struct DummyInterface {
    dipole: sim_mgt::Dipole,
    torque_end: Option<Instant>,
    hk_requested: bool,
}

impl DummyInterface {
    fn send(&mut self, request: sim_mgt::Request) {
        match request {
            sim_mgt::Request::ApplyTorque { duration, dipole } => {
                self.dipole = dipole;
                self.torque_end = Some(Instant::now() + duration);
            }
            sim_mgt::Request::RequestHk => self.hk_requested = true,
        }
    }

    fn try_recv_hk(&mut self) -> Option<sim_mgt::HkSet> {
        if !std::mem::take(&mut self.hk_requested) {
            return None;
        }
        let torquing = self.torque_end.is_some_and(|end| Instant::now() < end);
        Some(sim_mgt::HkSet {
            dipole: if torquing {
                self.dipole
            } else {
                sim_mgt::Dipole::default()
            },
            torquing,
        })
    }
}

/// Records all requests and returns injected HK replies.
#[derive(Default)]
pub struct TestInterface {
    pub sent_requests: Vec<sim_mgt::Request>,
    pub hk_replies: VecDeque<sim_mgt::HkSet>,
}

pub struct SimInterface {
    pub sim_request_tx: mpsc::Sender<SimRequestWithTime>,
    pub sim_reply_rx: mpsc::Receiver<SimReply>,
}

impl SimInterface {
    fn send(&mut self, request: sim_mgt::Request) {
        if let Err(e) = self
            .sim_request_tx
            .send(SimRequestWithTime::new_with_epoch_time(request))
        {
            log::error!("failed to send MGT SIM request: {e}");
        }
    }

    fn try_recv_hk(&mut self) -> Option<sim_mgt::HkSet> {
        let sim_reply = self.sim_reply_rx.try_recv().ok()?;
        match sim_reply {
            SimReply::Mgt(sim_mgt::Reply::Hk(hk)) => Some(hk),
            _ => {
                log::warn!("unexpected MGT SIM reply: {sim_reply:?}");
                None
            }
        }
    }
}

pub enum MgtCommunication {
    Dummy(DummyInterface),
    Sim(SimInterface),
    #[allow(dead_code)]
    Test(TestInterface),
}

impl MgtCommunication {
    fn send(&mut self, request: sim_mgt::Request) {
        match self {
            MgtCommunication::Dummy(dummy) => dummy.send(request),
            MgtCommunication::Sim(sim) => sim.send(request),
            MgtCommunication::Test(test) => test.sent_requests.push(request),
        }
    }

    fn try_recv_hk(&mut self) -> Option<sim_mgt::HkSet> {
        match self {
            MgtCommunication::Dummy(dummy) => dummy.try_recv_hk(),
            MgtCommunication::Sim(sim) => sim.try_recv_hk(),
            MgtCommunication::Test(test) => test.hk_replies.pop_front(),
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
pub struct MgtHandler {
    tmtc_queues: TmtcQueues,
    pub com: MgtCommunication,
    hk_set: HkSet,
    hk_helper: HkHelperSingleSet,
    switch_and_mode_helper: SwitchAndModeHelper<DeviceMode>,
    mode_leaf_helper: ModeLeafHelper,
    event_tx: mpsc::SyncSender<mgt::Event>,
}

impl MgtHandler {
    pub fn new(
        tmtc_queues: TmtcQueues,
        switch_helper: PowerSwitchHelper,
        com: MgtCommunication,
        mode_leaf_helper: ModeLeafHelper,
        mode_timeout: Duration,
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

        if let Some(event) = self.switch_and_mode_helper.handle_mode_transition() {
            match event {
                ModeTransitionEvent::Reached(tc_commander) => {
                    self.handle_mode_reached(tc_commander)
                }
                ModeTransitionEvent::Failed(tc_commander) => {
                    self.handle_mode_transition_failure(tc_commander)
                }
                // No power cycles are started without FDIR.
                ModeTransitionEvent::PowerCycleDone
                | ModeTransitionEvent::PowerCycleFailed { .. } => (),
            }
        }

        if self.ready_for_commanding() {
            self.com.send(sim_mgt::Request::RequestHk);
        }
        while let Some(hk) = self.com.try_recv_hk() {
            self.hk_set = HkSet {
                valid: true,
                dipole: types::acs::mgt::Dipole {
                    x: hk.dipole.x,
                    y: hk.dipole.y,
                    z: hk.dipole.z,
                },
                torquing: hk.torquing,
            };
        }

        if self.hk_helper.needs_generation() {
            self.send_telemetry(None, Response::Hk(self.hk_set));
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
                Request::Mode(ModeRequest::ReadMode) => self
                    .send_telemetry(Some(tc_id), Response::Mode(ModeResponse::Mode(self.mode()))),
                Request::ApplyTorque { dipole, duration } => {
                    self.handle_torque_command(tc_id, dipole, duration)
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
        dipole: types::acs::mgt::Dipole,
        duration: Duration,
    ) {
        if !self.ready_for_commanding() {
            log::warn!("MGT: rejecting torque command, device not in normal mode");
            self.send_telemetry(Some(tc_id), Response::NotInNormalMode);
            return;
        }
        self.com.send(sim_mgt::Request::ApplyTorque {
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
    use satrs::spacepackets::SpacePacketHeader;
    use types::{
        Apid, Message as _, TcHeader,
        ccsds::{CcsdsTcPacketOwned, CcsdsTmPacketOwned},
        pcdu::{SwitchRequest, SwitchState, SwitchStateBinary},
    };

    use crate::eps::pcdu::{SharedSwitchSet, SwitchMap, SwitchSet};

    use super::*;

    struct MgtTestbench {
        parent_request_tx: mpsc::SyncSender<ModeRequest>,
        parent_report_rx: mpsc::Receiver<ModeResponse>,
        shared_switch_set: SharedSwitchSet,
        switch_rx: mpsc::Receiver<SwitchRequest>,
        tc_tx: mpsc::SyncSender<CcsdsTcPacketOwned>,
        tm_rx: mpsc::Receiver<CcsdsTmPacketOwned>,
        event_rx: mpsc::Receiver<mgt::Event>,
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
            let handler = MgtHandler::new(
                TmtcQueues { tc_rx, tm_tx },
                PowerSwitchHelper::new(switch_tx, shared_switch_set.clone()),
                MgtCommunication::Test(TestInterface::default()),
                ModeLeafHelper {
                    request_rx,
                    report_tx,
                },
                Duration::from_millis(100),
                event_tx,
            );
            Self {
                parent_request_tx,
                parent_report_rx,
                shared_switch_set,
                switch_rx,
                tc_tx,
                tm_rx,
                event_rx,
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
    }

    #[test]
    fn test_initial_state_no_polling() {
        let mut testbench = MgtTestbench::new();
        testbench.handler.periodic_operation();
        assert_eq!(testbench.handler.mode(), DeviceMode::Off);
        assert!(testbench.test_interface().sent_requests.is_empty());
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
        assert!(testbench.test_interface().sent_requests.is_empty());
    }

    #[test]
    fn test_torque_command_forwarded_in_normal_mode() {
        let mut testbench = MgtTestbench::new();
        testbench.switch_to_normal();
        testbench.test_interface().sent_requests.clear();

        testbench.send_tc(Request::ApplyTorque {
            dipole: mgt::Dipole { x: 1, y: 2, z: 3 },
            duration: Duration::from_millis(100),
        });
        testbench.handler.periodic_operation();
        assert_eq!(testbench.next_response(), Response::Ok);
        assert_eq!(
            testbench.test_interface().sent_requests.first(),
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
                .sent_requests
                .contains(&sim_mgt::Request::RequestHk)
        );

        testbench
            .test_interface()
            .hk_replies
            .push_back(sim_mgt::HkSet {
                dipole: sim_mgt::Dipole { x: 1, y: 2, z: 3 },
                torquing: true,
            });
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
            .hk_replies
            .push_back(sim_mgt::HkSet {
                dipole: sim_mgt::Dipole::default(),
                torquing: false,
            });
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
}
