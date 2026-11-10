/*
use core::cell::Cell;
use num_enum::TryFromPrimitive;
use satrs::dev_mgmt::{
    DevManagerCommandingHelper, DevManagerHelperResult, TransparentDevManagerHook,
};
use satrs::mode::{
    INVALID_MODE, Mode, ModeError, ModeProvider, ModeReplyReceiver, ModeReplySender,
    ModeRequestHandler, ModeRequestHandlerMpscBounded, ModeRequestReceiver,
    ModeRequestorAndHandlerMpscBounded, ModeRequestorOneChildBoundedMpsc, UNKNOWN_MODE,
};
use satrs::mode_tree::{
    ModeChild, ModeNode, ModeParent, ModeStoreProvider, SequenceTableEntry, SequenceTableMapTable,
    TargetTableEntry, connect_mode_nodes,
};
use satrs::mode_tree::{SequenceTablesMapValue, TargetTablesMapValue};
use satrs::request::{MessageMetadata, RequestId};
use satrs::res_code::ResultU16;
use satrs::subsystem::{
    IsChildCommandable, ModeCommandingResult, ModeTreeHelperError, ModeTreeHelperState,
    StartSequenceError, SubsystemCommandingHelper, SubsystemHelperResult,
};
use satrs::{
    ComponentId,
    mode::{ModeAndSubmode, ModeReply, ModeRequest},
    queue::GenericTargetedMessagingError,
    request::GenericMessage,
};
use std::borrow::{Borrow, BorrowMut};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::{println, sync::mpsc};

pub enum DefaultMode {
    OFF = 0,
    ON = 1,
    NORMAL = 2,
}

#[derive(Debug)]
pub enum AcsMode {
    OFF = 0,
    SAFE = 1,
    IDLE = 2,
}

#[derive(Debug, TryFromPrimitive)]
#[repr(u32)]
pub enum TestComponentId {
    MagnetometerDevice0 = 1,
    MagnetometerDevice1 = 2,
    MagnetorquerDevice = 5,
    ReactionWheelDevice = 6,
    StartrackerDevice = 7,
    MtqDevManager = 8,
    ReactionWheelAssembly = 10,
    MagnetometerAssembly = 11,
    AcsController = 14,
    AcsSubsystem = 15,
    PusModeService = 16,
}

pub type RequestSenderType = mpsc::SyncSender<GenericMessage<ModeRequest>>;
pub type ReplySenderType = mpsc::SyncSender<GenericMessage<ModeReply>>;

#[derive(Default, Debug)]
pub struct ModeRequestHandlerMock {
    pub id: ComponentId,
    get_mode_calls: RefCell<usize>,
    start_transition_calls: VecDeque<(MessageMetadata, ModeAndSubmode)>,
    announce_mode_calls: RefCell<VecDeque<AnnounceModeInfo>>,
    handle_mode_info_calls: VecDeque<(MessageMetadata, ModeAndSubmode)>,
    handle_mode_reached_calls: RefCell<VecDeque<Option<MessageMetadata>>>,
    send_mode_reply_calls: RefCell<VecDeque<(MessageMetadata, ModeReply)>>,
}

impl ModeRequestHandlerMock {
    pub fn new(id: ComponentId) -> Self {
        Self {
            id,
            ..Default::default()
        }
    }
}

impl ModeRequestHandlerMock {
    pub fn clear(&mut self) {
        self.get_mode_calls.replace(0);
        self.start_transition_calls.clear();
        self.announce_mode_calls.borrow_mut().clear();
        self.handle_mode_reached_calls.borrow_mut().clear();
        self.handle_mode_info_calls.clear();
        self.send_mode_reply_calls.borrow_mut().clear();
    }
    pub fn mode_messages_received(&self) -> usize {
        *self.get_mode_calls.borrow()
            + self.start_transition_calls.borrow().len()
            + self.announce_mode_calls.borrow().len()
            + self.handle_mode_info_calls.borrow().len()
            + self.handle_mode_reached_calls.borrow().len()
            + self.send_mode_reply_calls.borrow().len()
    }
}

impl ModeProvider for ModeRequestHandlerMock {
    fn mode_and_submode(&self) -> ModeAndSubmode {
        *self.get_mode_calls.borrow_mut() += 1;
        INVALID_MODE
    }
}

impl ModeRequestHandler for ModeRequestHandlerMock {
    type Error = Infallible;

    fn start_transition(
        &mut self,
        requestor: MessageMetadata,
        mode_and_submode: ModeAndSubmode,
        _forced: bool,
    ) -> Result<(), Self::Error> {
        self.start_transition_calls
            .push_back((requestor, mode_and_submode));
        Ok(())
    }

    fn announce_mode(&self, requestor_info: Option<MessageMetadata>, recursive: bool) {
        self.announce_mode_calls
            .borrow_mut()
            .push_back(AnnounceModeInfo {
                requestor: requestor_info,
                recursive,
            });
    }

    fn handle_mode_reached(
        &mut self,
        requestor_info: Option<MessageMetadata>,
    ) -> Result<(), Self::Error> {
        self.handle_mode_reached_calls
            .borrow_mut()
            .push_back(requestor_info);
        Ok(())
    }

    fn handle_mode_info(
        &mut self,
        requestor_info: MessageMetadata,
        info: ModeAndSubmode,
    ) -> Result<(), Self::Error> {
        self.handle_mode_info_calls
            .push_back((requestor_info, info));
        todo!()
    }

    fn send_mode_reply(
        &self,
        requestor_info: MessageMetadata,
        reply: ModeReply,
    ) -> Result<(), Self::Error> {
        self.send_mode_reply_calls
            .borrow_mut()
            .push_back((requestor_info, reply));
        Ok(())
    }
}

#[derive(Debug)]
pub struct ModeReplyHandlerMock {
    pub id: ComponentId,
    mode_info_messages: VecDeque<(MessageMetadata, ModeAndSubmode)>,
    mode_reply_messages: VecDeque<(MessageMetadata, ModeAndSubmode)>,
    cant_reach_mode_messages: VecDeque<(MessageMetadata, ResultU16)>,
    wrong_mode_messages: VecDeque<(MessageMetadata, ModeAndSubmode, ModeAndSubmode)>,
}

impl ModeReplyHandlerMock {
    pub fn new(id: ComponentId) -> Self {
        Self {
            id,
            mode_info_messages: Default::default(),
            mode_reply_messages: Default::default(),
            cant_reach_mode_messages: Default::default(),
            wrong_mode_messages: Default::default(),
        }
    }

    pub fn num_of_received_mode_replies(&self) -> usize {
        self.mode_info_messages.len()
            + self.mode_reply_messages.len()
            + self.cant_reach_mode_messages.len()
            + self.wrong_mode_messages.len()
    }

    pub fn handle_mode_reply(&mut self, request: &GenericMessage<ModeReply>) {
        match request.message {
            ModeReply::ModeInfo(mode_and_submode) => {
                self.mode_info_messages
                    .push_back((request.requestor_info, mode_and_submode));
            }
            ModeReply::ModeReply(mode_and_submode) => {
                self.mode_reply_messages
                    .push_back((request.requestor_info, mode_and_submode));
            }
            ModeReply::CantReachMode(result_u16) => {
                self.cant_reach_mode_messages
                    .push_back((request.requestor_info, result_u16));
            }
            ModeReply::WrongMode { expected, reached } => {
                self.wrong_mode_messages
                    .push_back((request.requestor_info, expected, reached));
            }
        }
    }
}

struct PusModeService {
    pub request_id_counter: Cell<u32>,
    pub mode_reply_mock: ModeReplyHandlerMock,
    mode_node: ModeRequestorOneChildBoundedMpsc,
}

impl PusModeService {
    pub fn new(init_req_count: u32, mode_node: ModeRequestorOneChildBoundedMpsc) -> Self {
        Self {
            request_id_counter: Cell::new(init_req_count),
            mode_reply_mock: ModeReplyHandlerMock::new(
                TestComponentId::PusModeService as ComponentId,
            ),
            mode_node,
        }
    }
    pub fn run(&mut self) {
        while let Some(reply) = self.mode_node.try_recv_mode_reply().unwrap() {
            self.mode_reply_mock.handle_mode_reply(&reply);
        }
    }

    pub fn announce_modes_recursively(&self) {
        self.mode_node
            .send_mode_request(
                self.request_id_counter.get(),
                TestComponentId::AcsSubsystem as ComponentId,
                ModeRequest::AnnounceModeRecursive,
            )
            .unwrap();
        self.request_id_counter
            .replace(self.request_id_counter.get() + 1);
    }

    pub fn send_mode_cmd(&self, mode: ModeAndSubmode) -> RequestId {
        let request_id = self.request_id_counter.get();
        self.mode_node
            .send_mode_request(
                request_id,
                TestComponentId::AcsSubsystem as ComponentId,
                ModeRequest::SetMode {
                    mode_and_submode: mode,
                    forced: false,
                },
            )
            .unwrap();
        self.request_id_counter.replace(request_id + 1);
        request_id
    }
}

impl ModeNode for PusModeService {
    fn id(&self) -> ComponentId {
        TestComponentId::PusModeService as ComponentId
    }
}

impl ModeParent for PusModeService {
    type Sender = RequestSenderType;

    fn add_mode_child(&mut self, id: ComponentId, request_sender: Self::Sender) {
        self.mode_node.add_message_target(id, request_sender);
    }
}

#[derive(Debug, Default)]
struct IsCommandableDummy {}

impl IsChildCommandable for IsCommandableDummy {
    fn is_commandable(&self, _id: ComponentId) -> bool {
        true
    }
}

struct AcsSubsystem {
    pub mode_node: ModeRequestorAndHandlerMpscBounded,
    pub mode_requestor_info: Option<MessageMetadata>,
    pub target_mode_and_submode: Option<ModeAndSubmode>,
    pub subsystem_helper: SubsystemCommandingHelper,
    is_commandable_dummy: IsCommandableDummy,
    pub mode_req_mock: ModeRequestHandlerMock,
    pub mode_reply_mock: ModeReplyHandlerMock,
}

impl AcsSubsystem {
    pub fn id() -> ComponentId {
        TestComponentId::AcsSubsystem as ComponentId
    }

    pub fn new(mode_node: ModeRequestorAndHandlerMpscBounded) -> Self {
        Self {
            mode_node,
            mode_requestor_info: None,
            target_mode_and_submode: None,
            is_commandable_dummy: IsCommandableDummy::default(),
            subsystem_helper: SubsystemCommandingHelper::default(),
            mode_req_mock: ModeRequestHandlerMock::new(Self::id()),
            mode_reply_mock: ModeReplyHandlerMock::new(Self::id()),
        }
    }

    pub fn get_num_mode_requests(&mut self) -> usize {
        self.mode_req_mock.mode_messages_received()
    }

    pub fn handle_subsystem_helper_result(
        &mut self,
        result: Result<SubsystemHelperResult, ModeTreeHelperError>,
    ) {
        match result {
            Ok(result) => {
                if let SubsystemHelperResult::ModeCommanding(ModeCommandingResult::Done) = result {
                    self.handle_mode_reached(self.mode_requestor_info)
                        .expect("mode reply handling failed");
                }
            }
            Err(error) => match error {
                ModeTreeHelperError::Message(_generic_targeted_messaging_error) => {
                    panic!("messaging error")
                }
                ModeTreeHelperError::CurrentModeNotInTargetTable(_) => panic!("mode not found"),
                ModeTreeHelperError::ModeCommmandFailure { seq_table_index: _ } => {
                    // TODO: Cache the command failure.
                }
                ModeTreeHelperError::TargetKeepingViolation { fallback_mode } => {
                    if let Some(fallback_mode) = fallback_mode {
                        self.subsystem_helper
                            .start_command_sequence(fallback_mode, 0)
                            .unwrap();
                    }
                }
            },
        }
    }

    pub fn run(&mut self) {
        while let Some(request) = self.mode_node.try_recv_mode_request().unwrap() {
            self.handle_mode_request(request)
                .expect("mode messaging error");
        }

        let mut received_reply = false;
        while let Some(mode_reply) = self.mode_node.try_recv_mode_reply().unwrap() {
            received_reply = true;
            self.mode_reply_mock.handle_mode_reply(&mode_reply);
            let result = self.subsystem_helper.state_machine(
                Some(mode_reply),
                &self.mode_node,
                &self.is_commandable_dummy,
            );
            self.handle_subsystem_helper_result(result);
        }
        if !received_reply {
            let result = self.subsystem_helper.state_machine(
                None,
                &self.mode_node,
                &self.is_commandable_dummy,
            );
            self.handle_subsystem_helper_result(result);
        }
    }

    pub fn add_target_and_sequence_table(
        &mut self,
        mode: Mode,
        target_table_val: TargetTablesMapValue,
        sequence_table_val: SequenceTablesMapValue,
    ) {
        self.subsystem_helper.add_target_and_sequence_table(
            mode,
            target_table_val,
            sequence_table_val,
        );
    }
}

impl ModeNode for AcsSubsystem {
    fn id(&self) -> ComponentId {
        Self::id()
    }
}

impl ModeParent for AcsSubsystem {
    type Sender = RequestSenderType;

    fn add_mode_child(&mut self, id: ComponentId, request_sender: RequestSenderType) {
        self.subsystem_helper.add_mode_child(id, UNKNOWN_MODE);
        self.mode_node.add_request_target(id, request_sender);
    }
}

impl ModeChild for AcsSubsystem {
    type Sender = ReplySenderType;

    fn add_mode_parent(&mut self, id: ComponentId, reply_sender: ReplySenderType) {
        self.mode_node.add_reply_target(id, reply_sender);
    }
}

impl ModeProvider for AcsSubsystem {
    fn mode_and_submode(&self) -> ModeAndSubmode {
        ModeAndSubmode::new(self.subsystem_helper.mode(), 0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SubsystemModeError {
    #[error("messaging error: {0:?}")]
    Mode(#[from] ModeError),
    #[error("start sequence error: {0}")]
    StartError(#[from] StartSequenceError),
}

impl ModeRequestHandler for AcsSubsystem {
    type Error = SubsystemModeError;

    fn start_transition(
        &mut self,
        requestor: MessageMetadata,
        mode_and_submode: ModeAndSubmode,
        forced: bool,
    ) -> Result<(), Self::Error> {
        if !forced && self.subsystem_helper.state() == ModeTreeHelperState::ModeCommanding {
            return Err(ModeError::Busy.into());
        }
        self.mode_requestor_info = Some(requestor);
        self.target_mode_and_submode = Some(mode_and_submode);
        self.mode_req_mock
            .start_transition(requestor, mode_and_submode, forced)
            .unwrap();
        println!("subsystem: mode req with ID {}", requestor.request_id());
        self.subsystem_helper
            .start_command_sequence(mode_and_submode.mode(), requestor.request_id())?;
        Ok(())
    }

    fn announce_mode(&self, requestor_info: Option<MessageMetadata>, recursive: bool) {
        println!(
            "TestAssembly: Announcing mode (recursively: {}): {:?}",
            recursive,
            self.subsystem_helper.mode()
        );
        let request_id = requestor_info.map_or(0, |info| info.request_id());
        self.subsystem_helper
            .send_announce_mode_cmd_to_children(request_id, &self.mode_node, recursive)
            .expect("sending mode request failed");
        self.mode_req_mock.announce_mode(requestor_info, recursive);
    }

    fn handle_mode_reached(
        &mut self,
        requestor_info: Option<MessageMetadata>,
    ) -> Result<(), Self::Error> {
        self.mode_req_mock
            .handle_mode_reached(requestor_info)
            .unwrap();
        if let Some(requestor) = requestor_info {
            self.send_mode_reply(
                requestor,
                ModeReply::ModeReply(ModeAndSubmode::new(self.subsystem_helper.mode(), 0)),
            )?;
        }
        Ok(())
    }

    fn handle_mode_info(
        &mut self,
        requestor_info: MessageMetadata,
        info: ModeAndSubmode,
    ) -> Result<(), Self::Error> {
        self.mode_req_mock
            .handle_mode_info(requestor_info, info)
            .unwrap();
        // TODO: Need to check whether mode table execution is finished.
        // This works by checking the children modes received through replies against the
        // mode table after all transition tables were executed.
        Ok(())
    }

    fn send_mode_reply(
        &self,
        requestor_info: MessageMetadata,
        reply: ModeReply,
    ) -> Result<(), Self::Error> {
        self.mode_req_mock
            .send_mode_reply(requestor_info, reply)
            .unwrap();
        self.mode_node
            .send_mode_reply(requestor_info, reply)
            .map_err(ModeError::Send)?;
        Ok(())
    }
}

struct MgmAssembly {
    pub mode_node: ModeRequestorAndHandlerMpscBounded,
    pub mode_requestor_info: Option<MessageMetadata>,
    pub mode_and_submode: ModeAndSubmode,
    pub commanding_helper: DevManagerCommandingHelper<TransparentDevManagerHook>,
    pub mode_req_mock: ModeRequestHandlerMock,
    pub mode_reply_mock: ModeReplyHandlerMock,
}

impl MgmAssembly {
    pub fn id() -> ComponentId {
        TestComponentId::MagnetometerAssembly as ComponentId
    }
    pub fn new(mode_node: ModeRequestorAndHandlerMpscBounded) -> Self {
        Self {
            mode_node,
            mode_requestor_info: None,
            mode_and_submode: UNKNOWN_MODE,
            commanding_helper: DevManagerCommandingHelper::new(TransparentDevManagerHook::default()),
            mode_req_mock: ModeRequestHandlerMock::new(Self::id()),
            mode_reply_mock: ModeReplyHandlerMock::new(Self::id()),
        }
    }

    pub fn run(&mut self) {
        self.check_mode_requests().expect("mode messaging error");
        self.check_mode_replies().expect("mode messaging error");
    }

    pub fn get_num_mode_requests(&mut self) -> usize {
        self.mode_req_mock.mode_messages_received()
    }

    pub fn check_mode_requests(&mut self) -> Result<(), GenericTargetedMessagingError> {
        while let Some(request) = self.mode_node.try_recv_mode_request()? {
            self.handle_mode_request(request).unwrap();
        }
        Ok(())
    }

    pub fn check_mode_replies(&mut self) -> Result<(), ModeError> {
        while let Some(reply_and_id) = self.mode_node.try_recv_mode_reply()? {
            self.mode_reply_mock.handle_mode_reply(&reply_and_id);
            match self.commanding_helper.handle_mode_reply(&reply_and_id) {
                Ok(result) => {
                    if let DevManagerHelperResult::ModeCommandingDone(context) = result {
                        // Complete the mode command.
                        self.mode_and_submode = context.target_mode;
                        self.handle_mode_reached(self.mode_requestor_info)?;
                    }
                }
                Err(err) => match err {
                    satrs::dev_mgmt::DevManagerHelperError::ChildNotInStore => todo!(),
                },
            }
        }
        Ok(())
    }
}

impl ModeNode for MgmAssembly {
    fn id(&self) -> ComponentId {
        Self::id()
    }
}
impl ModeParent for MgmAssembly {
    type Sender = RequestSenderType;

    fn add_mode_child(&mut self, id: ComponentId, request_sender: RequestSenderType) {
        self.mode_node.add_request_target(id, request_sender);
        self.commanding_helper.add_mode_child(id, UNKNOWN_MODE);
    }
}

impl ModeChild for MgmAssembly {
    type Sender = ReplySenderType;

    fn add_mode_parent(&mut self, id: ComponentId, reply_sender: ReplySenderType) {
        self.mode_node.add_reply_target(id, reply_sender);
    }
}

impl ModeProvider for MgmAssembly {
    fn mode_and_submode(&self) -> ModeAndSubmode {
        self.mode_and_submode
    }
}

impl ModeRequestHandler for MgmAssembly {
    type Error = ModeError;
    fn start_transition(
        &mut self,
        requestor: MessageMetadata,
        mode_and_submode: ModeAndSubmode,
        forced: bool,
    ) -> Result<(), Self::Error> {
        // Always accept forced commands and commands to mode OFF.
        if self.commanding_helper.target_mode().is_some()
            && !forced
            && mode_and_submode.mode() != DefaultMode::OFF as u32
        {
            return Err(ModeError::Busy);
        }
        self.mode_requestor_info = Some(requestor);
        self.mode_req_mock
            .start_transition(requestor, mode_and_submode, forced)
            .unwrap();
        self.commanding_helper.send_mode_cmd_to_all_children(
            requestor.request_id(),
            mode_and_submode,
            forced,
            &self.mode_node,
        )?;
        Ok(())
    }

    fn announce_mode(&self, requestor_info: Option<MessageMetadata>, recursive: bool) {
        println!(
            "TestAssembly: Announcing mode (recursively: {}): {:?}",
            recursive, self.mode_and_submode
        );
        let request_id = requestor_info.map_or(0, |info| info.request_id());
        self.commanding_helper
            .send_announce_mode_cmd_to_children(request_id, &self.mode_node, recursive)
            .expect("sending mode request failed");
        self.mode_req_mock.announce_mode(requestor_info, recursive);
    }

    fn handle_mode_reached(
        &mut self,
        mode_requestor: Option<MessageMetadata>,
    ) -> Result<(), Self::Error> {
        if let Some(requestor) = mode_requestor {
            self.send_mode_reply(requestor, ModeReply::ModeReply(self.mode_and_submode))?;
        }
        self.mode_req_mock
            .handle_mode_reached(mode_requestor)
            .unwrap();
        Ok(())
    }

    fn handle_mode_info(
        &mut self,
        requestor_info: MessageMetadata,
        info: ModeAndSubmode,
    ) -> Result<(), Self::Error> {
        self.mode_req_mock
            .handle_mode_info(requestor_info, info)
            .unwrap();
        // TODO: A proper assembly must reach to mode changes of its children..
        Ok(())
    }

    fn send_mode_reply(
        &self,
        requestor: MessageMetadata,
        reply: ModeReply,
    ) -> Result<(), Self::Error> {
        self.mode_node.send_mode_reply(requestor, reply)?;
        self.mode_req_mock
            .send_mode_reply(requestor, reply)
            .unwrap();
        Ok(())
    }
}

struct DeviceManager {
    name: &'static str,
    pub id: ComponentId,
    pub commanding_helper: DevManagerCommandingHelper<TransparentDevManagerHook>,
    pub mode_node: ModeRequestorAndHandlerMpscBounded,
    pub mode_requestor_info: Option<MessageMetadata>,
    pub mode_and_submode: ModeAndSubmode,
    pub mode_req_mock: ModeRequestHandlerMock,
    pub mode_reply_mock: ModeReplyHandlerMock,
}

impl DeviceManager {
    pub fn new(
        name: &'static str,
        id: ComponentId,
        mode_node: ModeRequestorAndHandlerMpscBounded,
    ) -> Self {
        Self {
            name,
            id,
            mode_node,
            mode_requestor_info: None,
            commanding_helper: DevManagerCommandingHelper::new(TransparentDevManagerHook::default()),
            mode_and_submode: UNKNOWN_MODE,
            mode_req_mock: ModeRequestHandlerMock::new(id),
            mode_reply_mock: ModeReplyHandlerMock::new(id),
        }
    }

    pub fn get_num_mode_requests(&mut self) -> usize {
        self.mode_req_mock.mode_messages_received()
    }

    pub fn run(&mut self) {
        self.check_mode_requests().expect("mode messaging error");
        self.check_mode_replies().expect("mode reply error");
    }

    pub fn check_mode_requests(&mut self) -> Result<(), ModeError> {
        while let Some(request) = self.mode_node.try_recv_mode_request()? {
            self.handle_mode_request(request)?
        }
        Ok(())
    }

    pub fn check_mode_replies(&mut self) -> Result<(), ModeError> {
        while let Some(reply) = self.mode_node.try_recv_mode_reply()? {
            self.handle_mode_reply(&reply)?;
        }
        Ok(())
    }

    pub fn handle_mode_reply(
        &mut self,
        mode_reply: &GenericMessage<ModeReply>,
    ) -> Result<(), ModeError> {
        self.mode_reply_mock.handle_mode_reply(mode_reply);
        match self.commanding_helper.handle_mode_reply(mode_reply) {
            Ok(result) => {
                match result {
                    DevManagerHelperResult::Idle => todo!(),
                    DevManagerHelperResult::Busy => todo!(),
                    DevManagerHelperResult::ModeCommandingDone(context) => {
                        // Complete the mode command.
                        self.handle_mode_reached(self.mode_requestor_info)?;
                        self.mode_and_submode = context.target_mode;
                    }
                }
            }
            Err(e) => match e {
                satrs::dev_mgmt::DevManagerHelperError::ChildNotInStore => todo!(),
            },
        }
        Ok(())
    }
}

impl ModeNode for DeviceManager {
    fn id(&self) -> ComponentId {
        self.id
    }
}

impl ModeChild for DeviceManager {
    type Sender = ReplySenderType;

    fn add_mode_parent(&mut self, id: ComponentId, reply_sender: ReplySenderType) {
        self.mode_node.add_reply_target(id, reply_sender);
    }
}

impl ModeParent for DeviceManager {
    type Sender = RequestSenderType;

    fn add_mode_child(&mut self, id: ComponentId, request_sender: Self::Sender) {
        self.commanding_helper
            .children_mode_store
            .add_component(id, UNKNOWN_MODE);
        self.mode_node.add_request_target(id, request_sender);
    }
}

impl ModeProvider for DeviceManager {
    fn mode_and_submode(&self) -> ModeAndSubmode {
        self.mode_and_submode
    }
}

impl ModeRequestHandler for DeviceManager {
    type Error = ModeError;

    fn start_transition(
        &mut self,
        requestor: MessageMetadata,
        mode_and_submode: ModeAndSubmode,
        forced: bool,
    ) -> Result<(), ModeError> {
        self.mode_and_submode = mode_and_submode;
        self.mode_requestor_info = Some(requestor);
        self.mode_req_mock
            .start_transition(requestor, mode_and_submode, forced)
            .unwrap();
        self.commanding_helper.send_mode_cmd_to_all_children(
            requestor.request_id(),
            mode_and_submode,
            forced,
            &self.mode_node,
        )?;
        Ok(())
    }

    fn announce_mode(&self, requestor_info: Option<MessageMetadata>, recursive: bool) {
        println!(
            "{}: announcing mode: {:?}",
            self.name, self.mode_and_submode
        );
        let request_id = requestor_info.map_or(0, |info| info.request_id());
        self.commanding_helper
            .send_announce_mode_cmd_to_children(request_id, &self.mode_node, recursive)
            .expect("sending mode announce request failed");
        self.mode_req_mock.announce_mode(requestor_info, recursive);
    }

    fn handle_mode_reached(&mut self, requestor: Option<MessageMetadata>) -> Result<(), ModeError> {
        if let Some(requestor) = requestor {
            self.send_mode_reply(requestor, ModeReply::ModeReply(self.mode_and_submode))?;
        }
        self.mode_req_mock.handle_mode_reached(requestor).unwrap();
        Ok(())
    }

    fn handle_mode_info(
        &mut self,
        requestor_info: MessageMetadata,
        info: ModeAndSubmode,
    ) -> Result<(), ModeError> {
        // A device is a leaf in the tree.. so this really should not happen
        println!(
            "{}: unexpected mode info from {:?} with mode: {:?}",
            self.name,
            requestor_info.sender_id(),
            info
        );
        self.mode_req_mock
            .handle_mode_info(requestor_info, info)
            .unwrap();
        Ok(())
    }

    fn send_mode_reply(
        &self,
        requestor_info: MessageMetadata,
        reply: ModeReply,
    ) -> Result<(), ModeError> {
        self.mode_node.send_mode_reply(requestor_info, reply)?;
        self.mode_req_mock
            .send_mode_reply(requestor_info, reply)
            .unwrap();
        Ok(())
    }
}

struct CommonDevice {
    name: &'static str,
    pub id: ComponentId,
    pub mode_node: ModeRequestHandlerMpscBounded,
    pub mode_and_submode: ModeAndSubmode,
    pub mode_req_mock: ModeRequestHandlerMock,
}

impl CommonDevice {
    pub fn new(
        name: &'static str,
        id: ComponentId,
        mode_node: ModeRequestHandlerMpscBounded,
    ) -> Self {
        Self {
            name,
            id,
            mode_node,
            mode_and_submode: UNKNOWN_MODE,
            mode_req_mock: ModeRequestHandlerMock::new(id),
        }
    }

    pub fn run(&mut self) {
        self.check_mode_requests().expect("mode messaging error");
    }

    pub fn check_mode_requests(&mut self) -> Result<(), ModeError> {
        if let Some(request) = self.mode_node.try_recv_mode_request()? {
            self.handle_mode_request(request)?
        }
        Ok(())
    }

    pub fn get_and_clear_num_mode_msgs(&mut self) -> usize {
        self.mode_req_mock.mode_messages_received()
    }
}

impl ModeNode for CommonDevice {
    fn id(&self) -> ComponentId {
        self.id
    }
}

impl ModeChild for CommonDevice {
    type Sender = ReplySenderType;

    fn add_mode_parent(&mut self, id: ComponentId, reply_sender: ReplySenderType) {
        self.mode_node.add_message_target(id, reply_sender);
    }
}

impl ModeProvider for CommonDevice {
    fn mode_and_submode(&self) -> ModeAndSubmode {
        self.mode_and_submode
    }
}

impl ModeRequestHandler for CommonDevice {
    type Error = ModeError;

    fn start_transition(
        &mut self,
        requestor: MessageMetadata,
        mode_and_submode: ModeAndSubmode,
        forced: bool,
    ) -> Result<(), ModeError> {
        if self.id() == TestComponentId::MagnetorquerDevice as ComponentId {
            println!("test");
        }
        self.mode_and_submode = mode_and_submode;
        self.handle_mode_reached(Some(requestor))?;
        self.mode_req_mock
            .start_transition(requestor, mode_and_submode, forced)
            .unwrap();
        Ok(())
    }

    fn announce_mode(&self, requestor_info: Option<MessageMetadata>, recursive: bool) {
        println!(
            "{}: announcing mode: {:?}",
            self.name, self.mode_and_submode
        );
        self.mode_req_mock.announce_mode(requestor_info, recursive);
    }

    fn handle_mode_reached(&mut self, requestor: Option<MessageMetadata>) -> Result<(), ModeError> {
        if let Some(requestor) = requestor {
            self.send_mode_reply(requestor, ModeReply::ModeReply(self.mode_and_submode))?;
        }
        self.mode_req_mock.handle_mode_reached(requestor).unwrap();
        Ok(())
    }

    fn handle_mode_info(
        &mut self,
        requestor_info: MessageMetadata,
        info: ModeAndSubmode,
    ) -> Result<(), ModeError> {
        // A device is a leaf in the tree.. so this really should not happen
        println!(
            "{}: unexpected mode info from {:?} with mode: {:?}",
            self.name,
            requestor_info.sender_id(),
            info
        );
        self.mode_req_mock
            .handle_mode_info(requestor_info, info)
            .unwrap();
        Ok(())
    }

    fn send_mode_reply(
        &self,
        requestor_info: MessageMetadata,
        reply: ModeReply,
    ) -> Result<(), ModeError> {
        self.mode_node.send_mode_reply(requestor_info, reply)?;
        self.mode_req_mock
            .send_mode_reply(requestor_info, reply)
            .unwrap();
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct AnnounceModeInfo {
    pub requestor: Option<MessageMetadata>,
    pub recursive: bool,
}

pub struct AcsController {
    pub mode_node: ModeRequestHandlerMpscBounded,
    pub mode_and_submode: ModeAndSubmode,
    pub announce_mode_queue: RefCell<VecDeque<AnnounceModeInfo>>,
    pub mode_req_mock: ModeRequestHandlerMock,
}

impl AcsController {
    pub fn id() -> ComponentId {
        TestComponentId::AcsController as ComponentId
    }
    pub fn new(mode_node: ModeRequestHandlerMpscBounded) -> Self {
        Self {
            mode_node,
            mode_and_submode: UNKNOWN_MODE,
            announce_mode_queue: Default::default(),
            mode_req_mock: ModeRequestHandlerMock::new(Self::id()),
        }
    }
    pub fn run(&mut self) {
        self.check_mode_requests().expect("mode messaging error");
    }

    pub fn check_mode_requests(&mut self) -> Result<(), ModeError> {
        if let Some(request) = self.mode_node.try_recv_mode_request()? {
            self.handle_mode_request(request)?
        }
        Ok(())
    }
}

impl ModeNode for AcsController {
    fn id(&self) -> ComponentId {
        TestComponentId::AcsController as ComponentId
    }
}

impl ModeChild for AcsController {
    type Sender = ReplySenderType;

    fn add_mode_parent(&mut self, id: ComponentId, reply_sender: ReplySenderType) {
        self.mode_node.add_message_target(id, reply_sender);
    }
}

impl ModeProvider for AcsController {
    fn mode_and_submode(&self) -> ModeAndSubmode {
        self.mode_and_submode
    }
}

impl ModeRequestHandler for AcsController {
    type Error = ModeError;

    fn start_transition(
        &mut self,
        requestor: MessageMetadata,
        mode_and_submode: ModeAndSubmode,
        forced: bool,
    ) -> Result<(), Self::Error> {
        self.mode_and_submode = mode_and_submode;
        self.handle_mode_reached(Some(requestor))?;
        self.mode_req_mock
            .start_transition(requestor, mode_and_submode, forced)
            .unwrap();
        Ok(())
    }

    fn announce_mode(&self, requestor_info: Option<MessageMetadata>, recursive: bool) {
        self.announce_mode_queue
            .borrow_mut()
            .push_back(AnnounceModeInfo {
                requestor: requestor_info,
                recursive,
            });
        self.mode_req_mock.announce_mode(requestor_info, recursive);
    }

    fn handle_mode_reached(
        &mut self,
        requestor_info: Option<MessageMetadata>,
    ) -> Result<(), Self::Error> {
        if let Some(requestor) = requestor_info {
            self.send_mode_reply(requestor, ModeReply::ModeReply(self.mode_and_submode))?;
        }
        self.mode_req_mock
            .handle_mode_reached(requestor_info)
            .unwrap();
        Ok(())
    }

    fn handle_mode_info(
        &mut self,
        requestor_info: MessageMetadata,
        info: ModeAndSubmode,
    ) -> Result<(), Self::Error> {
        // The controller is a leaf in the tree.. so this really should not happen
        println!(
            "ACS Controller: unexpected mode info from {:?} with mode: {:?}",
            requestor_info.sender_id(),
            info
        );
        self.mode_req_mock
            .handle_mode_info(requestor_info, info)
            .unwrap();
        Ok(())
    }

    fn send_mode_reply(
        &self,
        requestor_info: MessageMetadata,
        reply: ModeReply,
    ) -> Result<(), Self::Error> {
        self.mode_node.send_mode_reply(requestor_info, reply)?;
        self.mode_req_mock
            .send_mode_reply(requestor_info, reply)
            .unwrap();
        Ok(())
    }
}

pub struct TreeTestbench {
    pus: PusModeService,
    subsystem: AcsSubsystem,
    ctrl: AcsController,
    mtq_manager: DeviceManager,
    mgm_assy: MgmAssembly,
    mgm_devs: [CommonDevice; 2],
    mtq_dev: CommonDevice,
}

impl Default for TreeTestbench {
    fn default() -> Self {
        Self::new()
    }
}

impl TreeTestbench {
    pub fn new() -> Self {
        // All request channel handles.
        let (request_sender_to_mgm_dev_0, request_receiver_mgm_dev_0) = mpsc::sync_channel(10);
        let (request_sender_to_mgm_dev_1, request_receiver_mgm_dev_1) = mpsc::sync_channel(10);
        let (request_sender_to_mtq_dev, request_receiver_mtq_dev) = mpsc::sync_channel(10);
        let (request_sender_to_mtq_man, request_receiver_mtq_man) = mpsc::sync_channel(10);
        let (request_sender_to_mgm_assy, request_receiver_mgm_assy) = mpsc::sync_channel(10);
        let (request_sender_to_acs_subsystem, request_receiver_acs_subsystem) =
            mpsc::sync_channel(10);
        let (request_sender_to_acs_ctrl, request_receiver_acs_ctrl) = mpsc::sync_channel(10);

        // All reply channel handles.
        let (reply_sender_to_mgm_assy, reply_receiver_mgm_assy) = mpsc::sync_channel(10);
        let (reply_sender_to_acs_subsystem, reply_receiver_acs_subsystem) = mpsc::sync_channel(10);
        let (reply_sender_to_pus, reply_receiver_pus) = mpsc::sync_channel(10);
        let (reply_sender_to_mtq_man, reply_receiver_mtq_man) = mpsc::sync_channel(10);

        // Mode requestors only.
        let mode_node_pus = ModeRequestorOneChildBoundedMpsc::new(
            TestComponentId::PusModeService as ComponentId,
            reply_receiver_pus,
        );

        // Mode requestors and handlers.
        let mgm_assy_node = ModeRequestorAndHandlerMpscBounded::new(
            TestComponentId::MagnetometerAssembly as ComponentId,
            request_receiver_mgm_assy,
            reply_receiver_mgm_assy,
        );
        let mtq_dev_mgmt_node = ModeRequestorAndHandlerMpscBounded::new(
            TestComponentId::MtqDevManager as ComponentId,
            request_receiver_mtq_man,
            reply_receiver_mtq_man,
        );
        let acs_subsystem_node = ModeRequestorAndHandlerMpscBounded::new(
            TestComponentId::AcsSubsystem as ComponentId,
            request_receiver_acs_subsystem,
            reply_receiver_acs_subsystem,
        );

        // Request handlers only.
        let mgm_dev_node_0 = ModeRequestHandlerMpscBounded::new(
            TestComponentId::MagnetometerDevice0 as ComponentId,
            request_receiver_mgm_dev_0,
        );
        let mgm_dev_node_1 = ModeRequestHandlerMpscBounded::new(
            TestComponentId::MagnetometerDevice1 as ComponentId,
            request_receiver_mgm_dev_1,
        );
        let mtq_dev_node = ModeRequestHandlerMpscBounded::new(
            TestComponentId::MagnetorquerDevice as ComponentId,
            request_receiver_mtq_dev,
        );
        let acs_ctrl_node = ModeRequestHandlerMpscBounded::new(
            TestComponentId::AcsController as ComponentId,
            request_receiver_acs_ctrl,
        );

        let mut mgm_dev_0 = CommonDevice::new(
            "MGM_0",
            TestComponentId::MagnetometerDevice0 as ComponentId,
            mgm_dev_node_0,
        );
        let mut mgm_dev_1 = CommonDevice::new(
            "MGM_1",
            TestComponentId::MagnetometerDevice1 as ComponentId,
            mgm_dev_node_1,
        );
        let mut mtq_dev = CommonDevice::new(
            "MTQ",
            TestComponentId::MagnetorquerDevice as ComponentId,
            mtq_dev_node,
        );
        let mut mtq_manager = DeviceManager::new(
            "MTQ_MANAGER",
            TestComponentId::MtqDevManager as ComponentId,
            mtq_dev_mgmt_node,
        );
        let mut mgm_assy = MgmAssembly::new(mgm_assy_node);
        let mut acs_subsystem = AcsSubsystem::new(acs_subsystem_node);
        let mut acs_ctrl = AcsController::new(acs_ctrl_node);
        let mut pus_service = PusModeService::new(1, mode_node_pus);

        // ACS subsystem tables
        let mut target_table_safe = TargetTablesMapValue::new("SAFE_TARGET_TBL", None);
        target_table_safe.add_entry(TargetTableEntry::new(
            "CTRL_SAFE",
            TestComponentId::AcsController as ComponentId,
            ModeAndSubmode::new(AcsMode::SAFE as u32, 0),
            // All submodes allowed.
            Some(0xffff),
        ));
        target_table_safe.add_entry(TargetTableEntry::new_with_precise_submode(
            "MGM_A_NML",
            TestComponentId::MagnetometerAssembly as ComponentId,
            ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0),
        ));
        target_table_safe.add_entry(TargetTableEntry::new_with_precise_submode(
            "MTQ_MAN_NML",
            TestComponentId::MtqDevManager as ComponentId,
            ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0),
        ));
        let mut sequence_tbl_safe_0 = SequenceTableMapTable::new("SAFE_SEQ_0_TBL");
        sequence_tbl_safe_0.add_entry(SequenceTableEntry::new(
            "SAFE_SEQ_0_MGM_A",
            TestComponentId::MagnetometerAssembly as ComponentId,
            ModeAndSubmode::new(DefaultMode::NORMAL as Mode, 0),
            false,
        ));
        sequence_tbl_safe_0.add_entry(SequenceTableEntry::new(
            "SAFE_SEQ_0_MTQ_MAN",
            TestComponentId::MtqDevManager as ComponentId,
            ModeAndSubmode::new(DefaultMode::NORMAL as Mode, 0),
            false,
        ));
        let mut sequence_tbl_safe_1 = SequenceTableMapTable::new("SAFE_SEQ_1_TBL");
        sequence_tbl_safe_1.add_entry(SequenceTableEntry::new(
            "SAFE_SEQ_1_ACS_CTRL",
            TestComponentId::AcsController as ComponentId,
            ModeAndSubmode::new(AcsMode::SAFE as Mode, 0),
            false,
        ));
        let mut sequence_tbl_safe = SequenceTablesMapValue::new("SAFE_SEQ_TBL");
        sequence_tbl_safe.add_sequence_table(sequence_tbl_safe_0);
        sequence_tbl_safe.add_sequence_table(sequence_tbl_safe_1);
        acs_subsystem.add_target_and_sequence_table(
            AcsMode::SAFE as u32,
            target_table_safe,
            sequence_tbl_safe,
        );

        // Connect the PUS mode service to all mode objects.
        connect_mode_nodes(
            &mut pus_service,
            request_sender_to_acs_subsystem,
            &mut acs_subsystem,
            reply_sender_to_pus.clone(),
        );
        connect_mode_nodes(
            &mut pus_service,
            request_sender_to_acs_ctrl.clone(),
            &mut acs_ctrl,
            reply_sender_to_pus.clone(),
        );
        connect_mode_nodes(
            &mut pus_service,
            request_sender_to_mgm_dev_0.clone(),
            &mut mgm_dev_0,
            reply_sender_to_pus.clone(),
        );
        connect_mode_nodes(
            &mut pus_service,
            request_sender_to_mgm_dev_1.clone(),
            &mut mgm_dev_1,
            reply_sender_to_pus.clone(),
        );
        connect_mode_nodes(
            &mut pus_service,
            request_sender_to_mgm_assy.clone(),
            &mut mgm_assy,
            reply_sender_to_pus.clone(),
        );
        connect_mode_nodes(
            &mut pus_service,
            request_sender_to_mtq_man.clone(),
            &mut mtq_manager,
            reply_sender_to_pus.clone(),
        );
        connect_mode_nodes(
            &mut pus_service,
            request_sender_to_mtq_dev.clone(),
            &mut mtq_dev,
            reply_sender_to_pus.clone(),
        );

        // Connect the ACS subsystem to all children.
        connect_mode_nodes(
            &mut acs_subsystem,
            request_sender_to_mgm_assy,
            &mut mgm_assy,
            reply_sender_to_acs_subsystem.clone(),
        );
        connect_mode_nodes(
            &mut acs_subsystem,
            request_sender_to_acs_ctrl,
            &mut acs_ctrl,
            reply_sender_to_acs_subsystem.clone(),
        );
        connect_mode_nodes(
            &mut acs_subsystem,
            request_sender_to_mtq_man.clone(),
            &mut mtq_manager,
            reply_sender_to_acs_subsystem.clone(),
        );

        connect_mode_nodes(
            &mut mgm_assy,
            request_sender_to_mgm_dev_0,
            &mut mgm_dev_0,
            reply_sender_to_mgm_assy.clone(),
        );
        connect_mode_nodes(
            &mut mgm_assy,
            request_sender_to_mgm_dev_1,
            &mut mgm_dev_1,
            reply_sender_to_mgm_assy,
        );

        connect_mode_nodes(
            &mut mtq_manager,
            request_sender_to_mtq_dev,
            &mut mtq_dev,
            reply_sender_to_mtq_man,
        );
        Self {
            pus: pus_service,
            subsystem: acs_subsystem,
            ctrl: acs_ctrl,
            mgm_assy,
            mtq_dev,
            mtq_manager,
            mgm_devs: [mgm_dev_0, mgm_dev_1],
        }
    }

    pub fn run(&mut self) {
        self.pus.run();
        self.subsystem.run();
        self.ctrl.run();
        self.mtq_manager.run();
        self.mgm_assy.run();
        self.mgm_devs[0].run();
        self.mgm_devs[1].run();
        self.mtq_dev.run();
    }
}

#[test]
fn announce_recursively() {
    let mut tb = TreeTestbench::new();
    tb.pus.announce_modes_recursively();
    // Run everything twice so the order does not matter.
    tb.run();
    tb.run();
    assert_eq!(tb.subsystem.get_num_mode_requests(), 1);
    let mut announces = tb.subsystem.mode_req_mock.announce_mode_calls.borrow_mut();
    assert_eq!(announces.len(), 1);
    announces = tb.ctrl.mode_req_mock.announce_mode_calls.borrow_mut();
    assert_eq!(tb.ctrl.mode_req_mock.start_transition_calls.len(), 0);
    assert_eq!(tb.ctrl.mode_and_submode(), UNKNOWN_MODE);
    assert_eq!(announces.len(), 1);
    assert_eq!(tb.mgm_assy.get_num_mode_requests(), 1);
    announces = tb.mgm_assy.mode_req_mock.announce_mode_calls.borrow_mut();
    assert_eq!(tb.mgm_assy.mode_req_mock.start_transition_calls.len(), 0);
    assert_eq!(tb.mgm_assy.mode_and_submode(), UNKNOWN_MODE);
    assert_eq!(announces.len(), 1);
    for mgm_dev in &mut tb.mgm_devs {
        assert_eq!(mgm_dev.get_and_clear_num_mode_msgs(), 1);
        announces = mgm_dev.mode_req_mock.announce_mode_calls.borrow_mut();
        assert_eq!(mgm_dev.mode_req_mock.start_transition_calls.len(), 0);
        assert_eq!(mgm_dev.mode_and_submode(), UNKNOWN_MODE);
        assert_eq!(announces.len(), 1);
    }
    assert_eq!(announces.len(), 1);
    assert_eq!(tb.mtq_dev.get_and_clear_num_mode_msgs(), 1);
    announces = tb.mtq_dev.mode_req_mock.announce_mode_calls.borrow_mut();
    assert_eq!(tb.mtq_dev.mode_req_mock.start_transition_calls.len(), 0);
    assert_eq!(tb.mtq_dev.mode_and_submode(), UNKNOWN_MODE);
    assert_eq!(announces.len(), 1);
    assert_eq!(tb.mtq_manager.get_num_mode_requests(), 1);
    announces = tb
        .mtq_manager
        .mode_req_mock
        .announce_mode_calls
        .borrow_mut();
    assert_eq!(tb.mtq_manager.mode_req_mock.start_transition_calls.len(), 0);
    assert_eq!(tb.mtq_manager.mode_and_submode(), UNKNOWN_MODE);
    assert_eq!(announces.len(), 1);
}

fn generic_mode_reply_checker(
    reply_meta: MessageMetadata,
    mode_and_submode: ModeAndSubmode,
    expected_modes: &mut HashMap<ComponentId, ModeAndSubmode>,
) {
    let id = TestComponentId::try_from(reply_meta.sender_id()).expect("invalid sender id");
    if !expected_modes.contains_key(&reply_meta.sender_id()) {
        panic!("received unexpected mode reply from component {:?}", id);
    }
    let expected_mode = expected_modes.get(&reply_meta.sender_id()).unwrap();
    assert_eq!(
        expected_mode, &mode_and_submode,
        "mode mismatch for component {:?}, expected {:?}, got {:?}",
        id, expected_mode, mode_and_submode
    );
    expected_modes.remove(&reply_meta.sender_id());
}

#[test]
fn command_safe_mode() {
    let mut tb = TreeTestbench::new();
    assert_eq!(tb.ctrl.mode_and_submode(), UNKNOWN_MODE);
    assert_eq!(tb.mgm_devs[0].mode_and_submode(), UNKNOWN_MODE);
    assert_eq!(tb.mgm_devs[1].mode_and_submode(), UNKNOWN_MODE);
    let request_id = tb
        .pus
        .send_mode_cmd(ModeAndSubmode::new(AcsMode::SAFE as u32, 0));
    tb.run();
    tb.run();
    tb.run();
    let expected_req_id_not_ctrl = request_id << 8;
    let expected_req_id_ctrl = (request_id << 8) + 1;
    assert_eq!(
        tb.ctrl.mode_and_submode(),
        ModeAndSubmode::new(AcsMode::SAFE as u32, 0)
    );
    assert_eq!(
        tb.mgm_devs[0].mode_and_submode(),
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0)
    );
    assert_eq!(
        tb.mgm_devs[1].mode_and_submode(),
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0)
    );
    assert_eq!(
        tb.mgm_assy.mode_and_submode(),
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0)
    );
    assert_eq!(
        tb.mtq_manager.mode_and_submode(),
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0)
    );
    assert_eq!(
        tb.mtq_dev.mode_and_submode(),
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0)
    );
    assert_eq!(
        tb.subsystem.mode_and_submode(),
        ModeAndSubmode::new(AcsMode::SAFE as u32, 0)
    );

    // Check function calls for subsystem
    let generic_mock_check = |ctx: &str,
                              mock: &mut ModeRequestHandlerMock,
                              expected_mode_for_transition: ModeAndSubmode,
                              expected_req_id: RequestId| {
        assert_eq!(mock.start_transition_calls.borrow().len(), 1);
        let start_transition_call = mock.start_transition_calls.borrow_mut().front().unwrap();
        assert_eq!(
            start_transition_call.0.request_id(),
            expected_req_id,
            "unexpected req ID for component {}",
            ctx
        );
        assert_eq!(start_transition_call.1, expected_mode_for_transition);
        assert_eq!(mock.handle_mode_reached_calls.borrow().len(), 1);

        let handle_mode_reached_ref = mock.handle_mode_reached_calls.borrow();
        let handle_mode_reached_call = handle_mode_reached_ref.front().unwrap();
        assert_eq!(
            handle_mode_reached_call.as_ref().unwrap().request_id(),
            expected_req_id
        );
        drop(handle_mode_reached_ref);

        assert_eq!(mock.send_mode_reply_calls.borrow().len(), 1);
        let mode_reply_call = *mock.send_mode_reply_calls.borrow_mut().front().unwrap();
        assert_eq!(mode_reply_call.0.request_id(), expected_req_id);
        if let ModeReply::ModeReply(mode_and_submode) = mode_reply_call.1 {
            assert_eq!(
                mode_and_submode, expected_mode_for_transition,
                "unexpected mode for component {}",
                mock.id
            );
        } else {
            panic!("Unexpected mode reply for component {}", mock.id);
        }
        // TODO: Check all mode replies
        assert_eq!(mock.mode_messages_received(), 3);
        mock.clear();
    };

    generic_mock_check(
        "subsystem",
        &mut tb.subsystem.mode_req_mock,
        ModeAndSubmode::new(AcsMode::SAFE as u32, 0),
        request_id,
    );
    assert_eq!(
        tb.subsystem.subsystem_helper.request_id().unwrap(),
        request_id
    );
    assert_eq!(
        tb.subsystem.subsystem_helper.state(),
        ModeTreeHelperState::TargetKeeping
    );
    assert_eq!(tb.subsystem.subsystem_helper.mode(), AcsMode::SAFE as Mode);
    assert_eq!(
        tb.subsystem.mode_reply_mock.num_of_received_mode_replies(),
        3
    );
    let mut expected_modes = HashMap::new();
    expected_modes.insert(
        TestComponentId::AcsController as ComponentId,
        ModeAndSubmode::new(AcsMode::SAFE as u32, 0),
    );
    expected_modes.insert(
        TestComponentId::MagnetometerAssembly as ComponentId,
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0),
    );
    expected_modes.insert(
        TestComponentId::MtqDevManager as ComponentId,
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0),
    );
    while let Some(reply) = tb.subsystem.mode_reply_mock.mode_reply_messages.pop_front() {
        generic_mode_reply_checker(reply.0, reply.1, &mut expected_modes);
    }
    assert!(expected_modes.is_empty());

    generic_mock_check(
        "ctrl",
        &mut tb.ctrl.mode_req_mock,
        ModeAndSubmode::new(AcsMode::SAFE as u32, 0),
        expected_req_id_ctrl,
    );
    generic_mock_check(
        "mgm assy",
        &mut tb.mgm_assy.mode_req_mock,
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0),
        expected_req_id_not_ctrl,
    );
    let mut expected_modes = HashMap::new();
    expected_modes.insert(
        TestComponentId::MagnetometerDevice0 as ComponentId,
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0),
    );
    expected_modes.insert(
        TestComponentId::MagnetometerDevice1 as ComponentId,
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0),
    );
    while let Some(reply) = tb.mgm_assy.mode_reply_mock.mode_reply_messages.pop_front() {
        generic_mode_reply_checker(reply.0, reply.1, &mut expected_modes);
    }
    assert!(expected_modes.is_empty());

    generic_mock_check(
        "mtq mgmt",
        &mut tb.mtq_manager.mode_req_mock,
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0),
        expected_req_id_not_ctrl,
    );
    let mut expected_modes = HashMap::new();
    expected_modes.insert(
        TestComponentId::MagnetorquerDevice as ComponentId,
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0),
    );
    let reply = tb
        .mtq_manager
        .mode_reply_mock
        .mode_reply_messages
        .pop_front()
        .unwrap();
    generic_mode_reply_checker(reply.0, reply.1, &mut expected_modes);

    generic_mock_check(
        "mtq dev",
        &mut tb.mtq_dev.mode_req_mock,
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0),
        expected_req_id_not_ctrl,
    );
    generic_mock_check(
        "mgm dev 0",
        &mut tb.mgm_devs[0].mode_req_mock,
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0),
        expected_req_id_not_ctrl,
    );
    generic_mock_check(
        "mgm dev 1",
        &mut tb.mgm_devs[1].mode_req_mock,
        ModeAndSubmode::new(DefaultMode::NORMAL as u32, 0),
        expected_req_id_not_ctrl,
    );
}
*/
