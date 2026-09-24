#![allow(dead_code)]
use std::{
    sync::mpsc::{self, Receiver, SyncSender},
    time::Duration,
};

use satrs::{
    mode_tree::{
        ModeStoreProvider, ModeStoreVec, SequenceModeTables, SequenceTableEntry,
        SequenceTableMapTable, SequenceTablesMapValue, TargetModeTables,
    },
    spacepackets::CcsdsPacketIdAndPsc,
    subsystem::{
        ModeCommandingResult, ModeRaw, ModeResponse, ModeSetRequest, ModeTreeHelperError,
        SubsystemCommandingHelper, SubsystemHelperResult,
    },
};
use satrs_example::{ModeHelper, TmtcQueues};
use types::{
    ComponentId,
    acs::subsystem::{Mode, response},
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TransitionState {
    #[default]
    Idle,
    AwaitingReplies,
}

fn build_sequence_tables() -> SequenceModeTables {
    let mut seq_tables = SequenceModeTables::default();

    let mut off_table = SequenceTablesMapValue::new("OFF");
    let mut off_step_0 = SequenceTableMapTable::new("OFF_STEP_0");
    off_step_0.add_entry(SequenceTableEntry::new(
        "OFF_CTRL_PASSIVE",
        ComponentId::AcsController as satrs::ComponentId,
        types::acs::ctrl::Mode::Passive.into(),
        true,
    ));
    off_table.add_sequence_table(off_step_0);
    let mut off_step_1 = SequenceTableMapTable::new("OFF_STEP_1");
    off_step_1.add_entry(SequenceTableEntry::new(
        "OFF_MGM_ASSY_OFF",
        ComponentId::AcsMgmAssembly as satrs::ComponentId,
        types::acs::mgm_assembly::Mode::Device(types::DeviceMode::Off).into(),
        false,
    ));
    off_step_1.add_entry(SequenceTableEntry::new(
        "OFF_MGT_OFF",
        ComponentId::AcsMgt as satrs::ComponentId,
        types::DeviceMode::Off.into(),
        false,
    ));
    off_table.add_sequence_table(off_step_1);
    seq_tables.0.insert(Mode::Off as ModeRaw, off_table);

    let mut safe_table = SequenceTablesMapValue::new("SAFE");
    let mut safe_step_0 = SequenceTableMapTable::new("SAFE_STEP_0");
    safe_step_0.add_entry(SequenceTableEntry::new(
        "SAFE_MGM_ASSY_NORMAL",
        ComponentId::AcsMgmAssembly as satrs::ComponentId,
        types::acs::mgm_assembly::Mode::Device(types::DeviceMode::Normal).into(),
        false,
    ));
    safe_step_0.add_entry(SequenceTableEntry::new(
        "SAFE_MGT_NORMAL",
        ComponentId::AcsMgt as satrs::ComponentId,
        types::DeviceMode::Normal.into(),
        false,
    ));
    safe_table.add_sequence_table(safe_step_0);
    let mut safe_step_1 = SequenceTableMapTable::new("SAFE_STEP_1");
    safe_step_1.add_entry(SequenceTableEntry::new(
        "SAFE_CTRL_SAFE",
        ComponentId::AcsController as satrs::ComponentId,
        types::acs::ctrl::Mode::Safe.into(),
        false,
    ));
    safe_table.add_sequence_table(safe_step_1);
    seq_tables.0.insert(Mode::Safe as ModeRaw, safe_table);

    seq_tables
}

fn ctrl_response_to_mode_response(
    response: types::acs::ctrl::response::ModeReport,
) -> ModeResponse {
    let sender_id = ComponentId::AcsController as satrs::ComponentId;
    match response {
        types::acs::ctrl::response::ModeReport::Mode(mode) => ModeResponse {
            request_id: 0,
            sender_id,
            reported_mode: mode.into(),
            success: true,
        },
        types::acs::ctrl::response::ModeReport::WrongMode(_) => ModeResponse {
            request_id: 0,
            sender_id,
            reported_mode: 0,
            success: false,
        },
    }
}

fn mgm_assy_response_to_mode_response(
    response: types::acs::mgm_assembly::response::ModeResponse,
) -> ModeResponse {
    let sender_id = ComponentId::AcsMgmAssembly as satrs::ComponentId;
    match response {
        types::acs::mgm_assembly::response::ModeResponse::Mode(mode) => ModeResponse {
            request_id: 0,
            sender_id,
            reported_mode: mode.into(),
            success: true,
        },
        types::acs::mgm_assembly::response::ModeResponse::SetModeTimeout(_)
        | types::acs::mgm_assembly::response::ModeResponse::WrongMode(_)
        | types::acs::mgm_assembly::response::ModeResponse::CanNotKeepMode(_) => ModeResponse {
            request_id: 0,
            sender_id,
            reported_mode: 0,
            success: false,
        },
    }
}

fn mgt_response_to_mode_response(
    response: types::acs::mgt::response::ModeResponse,
) -> ModeResponse {
    let sender_id = ComponentId::AcsMgt as satrs::ComponentId;
    match response {
        types::acs::mgt::response::ModeResponse::Mode(mode) => ModeResponse {
            request_id: 0,
            sender_id,
            reported_mode: mode.into(),
            success: true,
        },
        types::acs::mgt::response::ModeResponse::SetModeTimeout => ModeResponse {
            request_id: 0,
            sender_id,
            reported_mode: 0,
            success: false,
        },
    }
}

#[derive(Debug)]
pub struct ModeRequestSenders {
    pub mode_request_ctrl: SyncSender<types::acs::ctrl::request::ModeRequest>,
    pub mode_request_mgm_assy: SyncSender<types::acs::mgm_assembly::request::ModeRequest>,
    pub mode_request_mgt: SyncSender<types::acs::mgt::request::ModeRequest>,
}

#[derive(Debug)]
pub struct ModeReportReceivers {
    pub mode_response_ctrl: Receiver<types::acs::ctrl::response::ModeReport>,
    pub mode_response_mgm_assy: Receiver<types::acs::mgm_assembly::response::ModeResponse>,
    pub mode_response_mgt: Receiver<types::acs::mgt::response::ModeResponse>,
}

#[derive(Debug)]
pub struct Subsystem {
    mode_helper: ModeHelper<types::acs::subsystem::Mode, TransitionState>,
    mode_request_senders: ModeRequestSenders,
    mode_report_receivers: ModeReportReceivers,
    tmtc_queues: TmtcQueues,
    subsystem_helper: SubsystemCommandingHelper,
}

impl Subsystem {
    pub const ID: ComponentId = ComponentId::AcsSubsystem;

    pub fn new(
        mode_request_senders: ModeRequestSenders,
        mode_report_receivers: ModeReportReceivers,
        tmtc_queues: TmtcQueues,
    ) -> Self {
        let mut mode_store_vec = ModeStoreVec::default();
        mode_store_vec
            .add_component(
                ComponentId::AcsMgmAssembly as satrs::ComponentId,
                types::acs::mgm_assembly::Mode::NoModeKeeping.into(),
            )
            .unwrap();
        mode_store_vec
            .add_component(
                ComponentId::AcsController as satrs::ComponentId,
                types::acs::ctrl::Mode::Passive.into(),
            )
            .unwrap();
        mode_store_vec
            .add_component(
                ComponentId::AcsMgt as satrs::ComponentId,
                types::DeviceMode::Off.into(),
            )
            .unwrap();

        let target_tables = TargetModeTables::default();

        let sequence_tables = build_sequence_tables();

        Self {
            mode_helper: ModeHelper::new(
                types::acs::subsystem::Mode::Off,
                Duration::from_millis(2000),
            ),
            mode_request_senders,
            mode_report_receivers,
            tmtc_queues,
            subsystem_helper: SubsystemCommandingHelper::new(
                mode_store_vec,
                target_tables,
                sequence_tables,
            ),
        }
    }

    pub fn periodic_operation(&mut self) {
        self.handle_telecommands();
        let mut mode_requests = Vec::new();

        while let Ok(response) = self.mode_report_receivers.mode_response_ctrl.try_recv() {
            let result = self
                .subsystem_helper
                .state_machine(Some(ctrl_response_to_mode_response(response)), |request| {
                    mode_requests.push(request)
                });
            self.handle_state_machine_result(result);
        }
        while let Ok(response) = self.mode_report_receivers.mode_response_mgm_assy.try_recv() {
            let result = self.subsystem_helper.state_machine(
                Some(mgm_assy_response_to_mode_response(response)),
                |request| mode_requests.push(request),
            );
            self.handle_state_machine_result(result);
        }
        while let Ok(response) = self.mode_report_receivers.mode_response_mgt.try_recv() {
            let result = self
                .subsystem_helper
                .state_machine(Some(mgt_response_to_mode_response(response)), |request| {
                    mode_requests.push(request)
                });
            self.handle_state_machine_result(result);
        }

        let result = self
            .subsystem_helper
            .state_machine(None, |request| mode_requests.push(request));
        self.handle_state_machine_result(result);

        for request in mode_requests {
            self.handle_mode_set_request(request);
        }
    }

    fn handle_state_machine_result(
        &mut self,
        result: Result<SubsystemHelperResult, ModeTreeHelperError>,
    ) {
        match result {
            Ok(result) => match result {
                SubsystemHelperResult::Idle => (),
                SubsystemHelperResult::TargetKeeping => (),
                SubsystemHelperResult::ModeCommanding(mode_commanding_result) => {
                    match mode_commanding_result {
                        ModeCommandingResult::Done => {
                            let target_mode_typed =
                                self.subsystem_helper.seq_exec_helper.target_mode();
                            let target_mode = target_mode_typed.map(Mode::try_from);
                            log::info!(
                                "ACS SS: mode commanding for target mode {:?} done",
                                target_mode
                            );
                            self.finish_mode_transition();
                        }
                        ModeCommandingResult::StepDone => log::info!("mode commanding step done"),
                        ModeCommandingResult::AwaitingSuccessCheck => {
                            log::info!("ACS SS: mode commanding awaiting success check")
                        }
                    }
                }
            },
            Err(e) => {
                log::error!("mode tree helper error: {}", e);
                self.mode_helper.finish(false);
            }
        }
    }

    fn finish_mode_transition(&mut self) {
        let tc_commander = self.mode_helper.finish(true);
        if let Some(requestor) = tc_commander {
            self.send_telemetry(
                Some(requestor),
                response::Response::Mode(response::ModeResponse::Mode(self.mode_helper.current)),
            );
        }
    }

    fn handle_mode_set_request(&mut self, request: ModeSetRequest) {
        let target_id = ComponentId::try_from(request.target_id);
        if let Ok(target_id) = target_id {
            match target_id {
                ComponentId::AcsMgmAssembly => self
                    .mode_request_senders
                    .mode_request_mgm_assy
                    .send(types::acs::mgm_assembly::request::ModeRequest::SetMode(
                        types::acs::mgm_assembly::Mode::try_from(request.mode).unwrap(),
                    ))
                    .unwrap(),
                ComponentId::AcsController => self
                    .mode_request_senders
                    .mode_request_ctrl
                    .send(types::acs::ctrl::request::ModeRequest::SetMode(
                        types::acs::ctrl::Mode::try_from(request.mode).unwrap(),
                    ))
                    .unwrap(),
                ComponentId::AcsMgt => self
                    .mode_request_senders
                    .mode_request_mgt
                    .send(types::acs::mgt::request::ModeRequest::SetMode(
                        types::DeviceMode::try_from(request.mode).unwrap(),
                    ))
                    .unwrap(),
                _ => {
                    log::error!("invalid target ID {:?} for mode command", target_id);
                }
            }
        }
    }

    pub fn handle_telecommands(&mut self) {
        loop {
            match self.tmtc_queues.tc_rx.try_recv() {
                Ok(packet) => {
                    let tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&packet.sp_header);
                    match postcard::from_bytes::<types::acs::subsystem::request::Request>(
                        &packet.payload,
                    ) {
                        Ok(request) => match request {
                            types::acs::subsystem::request::Request::Ping => {
                                self.send_telemetry(Some(tc_id), response::Response::Ok)
                            }
                            types::acs::subsystem::request::Request::Mode(mode_request) => {
                                self.handle_mode_request(Some(tc_id), mode_request);
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

    pub fn handle_mode_request(
        &mut self,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        mode_request: types::acs::subsystem::request::ModeRequest,
    ) {
        match mode_request {
            types::acs::subsystem::request::ModeRequest::SetMode(target_mode) => {
                self.mode_helper.start(target_mode);
                self.mode_helper.tc_commander = tc_id;
                if let Err(e) = self
                    .subsystem_helper
                    .start_command_sequence(target_mode as ModeRaw)
                {
                    log::error!("error starting command sequence: {}", e);
                }
            }
            types::acs::subsystem::request::ModeRequest::ReadMode => {
                self.send_telemetry(
                    None,
                    response::Response::Mode(response::ModeResponse::Mode(
                        self.mode_helper.current,
                    )),
                );
            }
        }
    }

    pub fn send_telemetry(
        &self,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        response: types::acs::subsystem::response::Response,
    ) {
        match crate::ccsds::pack_ccsds_tm_packet_for_now(Self::ID, tc_id, &response) {
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
}
