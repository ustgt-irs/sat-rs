use arbitrary_int::{traits::Integer as _, u24};

use crate::{
    ComponentId,
    mode_tree::{
        ModeStoreProvider, ModeStoreVec, SequenceModeTables, SequenceTableEntry,
        SequenceTableProvider, SequenceTablesMapValue, TargetModeTables, TargetNotInModeStoreError,
        TargetTablesMapValue,
    },
    request::RequestId,
};

pub type ModeRaw = u32;

#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum SequenceExecutionHelperState {
    /// The sequence execution is IDLE, no command is loaded or the sequence exection has
    /// finished
    Idle,
    /// The sequence helper is executing a sequence and no replies need to be awaited.
    Busy,
    /// The sequence helper is still awaiting a reply from a mode children. The reply awaition
    /// is a property of a mode commanding sequence
    AwaitingSuccessCheck,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ModeCommandingResult {
    /// The commanding of all children is finished
    Done,
    /// One step of a commanding chain is finished
    StepDone,
    /// Reply awaition is required for some children
    AwaitingSuccessCheck,
}

#[derive(Debug, thiserror::Error)]
#[error("mode {0} does not exist")]
pub struct ModeDoesNotExistError(ModeRaw);

#[derive(Debug, thiserror::Error)]
pub enum StartSequenceError {
    #[error("mode {0} does not exist")]
    ModeDoesNotExist(#[from] ModeDoesNotExistError),
}

#[derive(Debug, thiserror::Error)]
#[error("invalid sequence index")]
pub struct InvalidSequenceIndexError;

#[derive(Debug, Copy, Clone)]
pub struct SequenceExecutionInfo {
    target_mode: ModeRaw,
    current_sequence_index: Option<u8>,
    number_of_sequences: u8,
}

/// This sequence execution helper includes some boilerplate logic to
/// execute mode sequences.
///
/// It contains some boilerplate logic required for child mode commanding as specified in subsystem
/// sequence tables and also includes the
/// states required to track the current progress of a sequence execution and take care of
/// reply and success awaition.
#[derive(Debug)]
pub struct SequenceExecutionHelper {
    state: SequenceExecutionHelperState,
    info: Option<SequenceExecutionInfo>,
}

impl Default for SequenceExecutionHelper {
    fn default() -> Self {
        Self {
            state: SequenceExecutionHelperState::Idle,
            info: None,
        }
    }
}

impl SequenceExecutionHelper {
    pub fn new() -> Self {
        Default::default()
    }

    /// Load a new mode sequence to be executed
    #[cfg(feature = "alloc")]
    pub fn load(
        &mut self,
        mode: ModeRaw,
        sequence_tables: &SequenceModeTables,
    ) -> Result<(), ModeDoesNotExistError> {
        if !sequence_tables.0.contains_key(&mode) {
            return Err(ModeDoesNotExistError(mode));
        }
        self.state = SequenceExecutionHelperState::Busy;
        self.info = Some(SequenceExecutionInfo {
            target_mode: mode,
            current_sequence_index: None,
            number_of_sequences: 0,
        });
        Ok(())
    }

    /// Run the sequence execution helper.
    ///
    /// This function will execute the sequence in the given [SequenceModeTables] based on the
    /// mode loaded in [Self::load]. It calls [Self::execute_sequence_and_map_to_result] and
    /// automatically takes care of state management, including increments of the sequence table
    /// index.
    ///
    /// The returnvalues of the helper have the following meaning.
    ///
    /// * [ModeCommandingResult::AwaitingSuccessCheck] - The sequence is still awaiting a success.
    ///   The user should check whether all children have reached the commanded target mode, for
    ///   example by checking mode replies received by the children components, and
    ///   then calling [Self::confirm_sequence_done] to advance to the sequence or complete the
    ///   sequence.
    /// * [ModeCommandingResult::Done] - The sequence is done. The user can load a new
    ///   sequence now without overwriting the last one. The sequence executor is in
    ///   [SequenceExecutionHelperState::Idle] again.
    /// * [ModeCommandingResult::StepDone] - The sequence has advanced one step. The user
    ///   can now call [Self::run] again to immediately execute the next step in the sequence.
    ///
    /// Generally, periodic execution of the [Self::run] method should be performed while
    /// [Self::state] is not [SequenceExecutionHelperState::Idle].
    ///
    /// # Arguments
    ///
    /// * `sequence_table_provider` - This table contains the sequence tables to reach the mode
    ///   previously loaded with [Self::load]
    /// * `children_mode_store` - The mode store vector to keep track of the mode states of
    ///   children components
    /// * `mode_request_handler` - A function which is called to send out mode requests to the
    ///   children or cache the required requests so they can be sent after the function call.
    pub fn run<F: FnMut(ModeSetRequest)>(
        &mut self,
        sequence_table_provider: &impl SequenceTableProvider,
        children_mode_store: &mut impl ModeStoreProvider,
        mode_request_handler: F,
    ) -> Result<ModeCommandingResult, InvalidSequenceIndexError> {
        // TODO: Check whether sequence table length is larger than 255. Improbable, but let's
        // solve this cleanly.
        if self.state == SequenceExecutionHelperState::Idle {
            return Ok(ModeCommandingResult::Done);
        }
        if self.state == SequenceExecutionHelperState::AwaitingSuccessCheck {
            return Ok(ModeCommandingResult::AwaitingSuccessCheck);
        }
        if self.info.is_none() {
            return Ok(ModeCommandingResult::Done);
        }
        let mut current_info = self.info.unwrap();
        match current_info.current_sequence_index {
            Some(index) => {
                // Execute the sequence.
                Ok(self.execute_sequence_and_map_to_result(
                    index,
                    sequence_table_provider.sequence_at_index(index).unwrap(),
                    current_info.number_of_sequences == index + 1,
                    children_mode_store,
                    mode_request_handler,
                ))
            }
            None => {
                let sequence_index = 0;
                let sequence = sequence_table_provider
                    .sequence_at_index(sequence_index)
                    .unwrap();
                current_info.number_of_sequences =
                    sequence_table_provider.number_of_sequences() as u8;
                // Find the first sequence
                if sequence.is_empty() {
                    Ok(ModeCommandingResult::Done)
                } else {
                    current_info.current_sequence_index = Some(0);
                    // Update state.
                    self.info = Some(current_info);
                    Ok(self.execute_sequence_and_map_to_result(
                        sequence_index,
                        sequence,
                        current_info.number_of_sequences == 1,
                        children_mode_store,
                        mode_request_handler,
                    ))
                }
            }
        }
    }

    /// Retrieve the currently loaded target mode
    pub fn target_mode(&self) -> Option<ModeRaw> {
        Some(self.info?.target_mode)
    }

    /// Confirm that a sequence which is awaiting a success check is done
    pub fn confirm_sequence_done(&mut self) {
        if let SequenceExecutionHelperState::AwaitingSuccessCheck = self.state
            && let Some(info) = &mut self.info
            && let Some(current_sequence_index) = info.current_sequence_index
        {
            self.state = SequenceExecutionHelperState::Busy;
            if current_sequence_index + 1 == info.number_of_sequences {
                self.state = SequenceExecutionHelperState::Idle;
            }
            info.current_sequence_index = Some(current_sequence_index + 1);
        }
    }

    /// Internal state of the execution helper.
    #[inline]
    pub fn state(&self) -> SequenceExecutionHelperState {
        self.state
    }

    #[inline]
    pub fn awaiting_success_check(&self) -> bool {
        self.state == SequenceExecutionHelperState::AwaitingSuccessCheck
    }

    #[inline]
    pub fn current_sequence_index(&self) -> Option<u8> {
        self.info?.current_sequence_index
    }

    /// Execute a sequence at the given sequence index for a given sequence.
    ///
    /// The sequence is identifier by a sequence index. It is represented by a list of
    /// [SequenceTableEntry] values.
    ///
    /// This method calls [Self::execute_sequence] and maps the result to a [ModeCommandingResult].
    /// It is also called by the [Self::run] method of this helper.
    pub fn execute_sequence_and_map_to_result(
        &mut self,
        sequence_index: u8,
        commands_for_sequence: &[SequenceTableEntry],
        is_last_sequence: bool,
        children_mode_store: &mut impl ModeStoreProvider,
        mode_request_handler: impl FnMut(ModeSetRequest),
    ) -> ModeCommandingResult {
        if self.state() == SequenceExecutionHelperState::Idle {
            return ModeCommandingResult::Done;
        }
        if Self::execute_sequence(
            commands_for_sequence,
            children_mode_store,
            mode_request_handler,
        ) {
            self.state = SequenceExecutionHelperState::AwaitingSuccessCheck;
            ModeCommandingResult::AwaitingSuccessCheck
        } else if is_last_sequence {
            self.state = SequenceExecutionHelperState::Idle;
            ModeCommandingResult::Done
        } else {
            if let Some(info) = &mut self.info {
                info.current_sequence_index = Some(sequence_index + 1);
            }
            ModeCommandingResult::StepDone
        }
    }

    /// Generic stateless execution helper method.
    ///
    /// The [RequestId] and the [SequenceTableEntry] list to be executed are passed explicitely
    /// here. This method is called by [Self::execute_sequence_and_map_to_result].
    ///
    /// This method itereates through the entries of the given sequence table, creates
    /// mode requests to set the modes of the children according to the table entries and passes.
    /// them to the provided `mode_request_handler` closure.
    ///
    /// It also sets the reply awaition field in the children mode store where a success
    /// check is required to true.
    ///
    /// It returns whether any commanding success check is required by any entry in the table.
    pub fn execute_sequence(
        table_entries: &[SequenceTableEntry],
        children_mode_store: &mut impl ModeStoreProvider,
        mut mode_request_handler: impl FnMut(ModeSetRequest),
    ) -> bool {
        let mut some_succes_check_required = false;
        for entry in table_entries {
            let mode_set_request = ModeSetRequest {
                target_id: entry.common.target_id,
                mode: entry.common.mode,
            };
            mode_request_handler(mode_set_request);
            if entry.check_success {
                children_mode_store.set_reply_awaition_flag(entry.common.target_id);
                some_succes_check_required = true;
            }
        }
        some_succes_check_required
    }
}

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub enum ModeTreeHelperState {
    #[default]
    Idle,
    /// The helper is currently trying to keep a target mode.
    TargetKeeping,
    /// The helper is currently busy to command a mode.
    ModeCommanding,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub enum SubsystemHelperResult {
    #[default]
    Idle,
    /// Busy with target keeping.
    TargetKeeping,
    /// Result of a mode commanding operation
    ModeCommanding(ModeCommandingResult),
}

impl From<ModeCommandingResult> for SubsystemHelperResult {
    fn from(value: ModeCommandingResult) -> Self {
        Self::ModeCommanding(value)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModeTreeHelperError {
    #[error("current mode {0} is not contained in target table")]
    CurrentModeNotInTargetTable(ModeRaw),
    #[error("No sequence table found for mode {0:?}")]
    NoSequenceTableFound(Option<ModeRaw>),
    #[error("invalid sequence index")]
    InvalidSequenceIndex(#[from] InvalidSequenceIndexError),
    /// Mode command has failed, for example while executing a mode table.
    #[error("mode command failed")]
    ModeCommmandFailure {
        /// Table index of the sequence table entry which failed.
        seq_table_index: Option<u8>,
    },
    /// Target mode keeping violation.
    #[error("target keeping violation")]
    TargetKeepingViolation {
        /// Table index of the sequence table entry which failed.
        fallback_mode: Option<ModeRaw>,
    },
}

/// This is a helper object which can be used by a subsystem component to execute mode sequences
/// and perform target keeping.
///
/// It currently only works on systems with allocation support and it will also allocate at
/// run-time.
///
/// This helper object tries to compose as much data and state information as possible which is
/// required for this process.
#[derive(Debug)]
pub struct SubsystemCommandingHelper {
    /// State of the helper.
    state: ModeTreeHelperState,
    /// Current mode of the owner subsystem.
    current_mode: ModeRaw,
    /// This data structure is used to track all mode children.
    pub children_mode_store: ModeStoreVec,
    /// Sequence counter used to generate unique request IDs.
    sequence_counter: u24,
    // Active internal request ID, whic his built from the sequence counter and sequence index.
    //active_internal_request_id: Option<RequestId>,
    /// The primary data structure to keep the target state information for subsystem
    /// [modes][ModeRaw]. it specifies the mode each child should have for a certain subsystem mode
    /// and is relevant for target keeping.
    pub target_tables: TargetModeTables,
    /// The primary data structure to keep the sequence commanding information for commanded
    /// subsystem [modes][ModeRaw]. It specifies the actual commands and the order they should be
    /// sent in to reach a certain [mode][ModeRaw].
    pub sequence_tables: SequenceModeTables,
    /// The sequence execution helper is used to execute sequences in the [Self::sequence_tables].
    pub seq_exec_helper: SequenceExecutionHelper,
}

impl Default for SubsystemCommandingHelper {
    fn default() -> Self {
        Self {
            current_mode: u32::MAX,
            sequence_counter: u24::ZERO,
            state: Default::default(),
            children_mode_store: Default::default(),
            target_tables: Default::default(),
            sequence_tables: Default::default(),
            seq_exec_helper: Default::default(),
        }
    }
}

#[derive(Debug, Copy, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[non_exhaustive]
pub struct ModeSetRequest {
    pub target_id: ComponentId,
    pub mode: ModeRaw,
}

#[derive(Debug, Copy, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ModeResponse {
    pub request_id: RequestId,
    pub sender_id: ComponentId,
    pub reported_mode: ModeRaw,
    pub success: bool,
}

impl SubsystemCommandingHelper {
    /// Create a new substem commanding helper with an intial [ModeTreeHelperState::Idle] state,
    /// an empty mode children store and empty target and sequence mode tables.
    pub fn new(
        children_mode_store: ModeStoreVec,
        target_tables: TargetModeTables,
        sequence_tables: SequenceModeTables,
    ) -> Self {
        Self {
            current_mode: 0,
            state: ModeTreeHelperState::Idle,
            children_mode_store,
            sequence_counter: u24::ZERO,
            target_tables,
            sequence_tables,
            seq_exec_helper: Default::default(),
        }
    }

    pub fn state(&self) -> ModeTreeHelperState {
        self.state
    }

    pub fn mode(&self) -> ModeRaw {
        self.current_mode
    }

    /// Retrieve the fallback mode for the current mode of the subsystem by trying to retrieve
    /// it from the target table.
    ///
    /// If the current mode does not have a fallback mode, returns [None].
    /// If the current mode is not inside the target table, returns a [ModeDoesNotExistError].
    /// The fallback mode can and should be commanded when a target keeping violation was detected
    /// or after self-commanding to the current mode has failed, which can happen after a failed
    /// mode table execution.
    pub fn fallback_mode(&self) -> Result<Option<ModeRaw>, ModeDoesNotExistError> {
        self.target_tables
            .0
            .get(&self.current_mode)
            .ok_or(ModeDoesNotExistError(self.current_mode))
            .map(|v| v.fallback_mode)
    }

    /// Add a mode child to the internal [Self::children_mode_store].
    pub fn add_mode_child(&mut self, child: ComponentId, mode: ModeRaw) {
        // Can not fail for regular vector.
        self.children_mode_store.add_component(child, mode).unwrap();
    }

    /// Add a target mode table and an associated sequence mode table.
    pub fn add_target_and_sequence_table(
        &mut self,
        mode: ModeRaw,
        target_table_val: TargetTablesMapValue,
        sequence_table_val: SequenceTablesMapValue,
    ) {
        self.target_tables.0.insert(mode, target_table_val);
        self.sequence_tables.0.insert(mode, sequence_table_val);
    }

    /// Starts a command sequence for a given [mode][ModeRaw].
    ///
    /// # Arguments
    ///
    /// - `mode` - The mode to command
    pub fn start_command_sequence(&mut self, mode: ModeRaw) -> Result<(), StartSequenceError> {
        self.sequence_counter = self.sequence_counter.wrapping_add(u24::new(1));
        self.seq_exec_helper.load(mode, &self.sequence_tables)?;
        self.state = ModeTreeHelperState::ModeCommanding;
        Ok(())
    }

    /// In mode commanding mode, returns the current sequence index.
    pub fn current_sequence_index(&self) -> Option<u8> {
        if self.state != ModeTreeHelperState::ModeCommanding {
            return None;
        }
        self.seq_exec_helper.current_sequence_index()
    }

    /// State machine which can be used to drive the susbystem helper.
    ///
    /// During mode commanding, it is the responsibility of the user to discard mode responses
    /// not related to the current transition. One way to do this is to assign unique IDs to all
    /// requests sent to the device and only handle responses related to those IDs.
    pub fn state_machine<F: FnMut(ModeSetRequest)>(
        &mut self,
        opt_mode_reponse: Option<ModeResponse>,
        mode_request_handler: F,
    ) -> Result<SubsystemHelperResult, ModeTreeHelperError> {
        if let Some(reply) = opt_mode_reponse
            && self.handle_children_mode_changed(reply)?
        {
            if self.seq_exec_helper.state() == SequenceExecutionHelperState::Idle {
                self.transition_to_target_keeping();
                return Ok(SubsystemHelperResult::ModeCommanding(
                    ModeCommandingResult::Done,
                ));
            }
            return Ok(SubsystemHelperResult::ModeCommanding(
                ModeCommandingResult::StepDone,
            ));
        }
        match self.state {
            ModeTreeHelperState::Idle => Ok(SubsystemHelperResult::Idle),
            ModeTreeHelperState::TargetKeeping => {
                // We check whether the current mode is modelled by a target table first.
                if let Some(target_table) = self.target_tables.0.get(&self.current_mode) {
                    self.perform_target_keeping(target_table)?;
                }
                Ok(SubsystemHelperResult::TargetKeeping)
            }
            ModeTreeHelperState::ModeCommanding => {
                if self.seq_exec_helper.target_mode().is_none() {
                    return Err(ModeTreeHelperError::NoSequenceTableFound(None));
                }
                let seq_table = self
                    .sequence_tables
                    .0
                    .get(&self.seq_exec_helper.target_mode().unwrap());
                if seq_table.is_none() {
                    return Err(ModeTreeHelperError::NoSequenceTableFound(
                        self.seq_exec_helper.target_mode(),
                    ));
                }
                let result = self.seq_exec_helper.run(
                    seq_table.unwrap(),
                    &mut self.children_mode_store,
                    mode_request_handler,
                )?;
                match result {
                    ModeCommandingResult::Done => {
                        // By default, the helper will automatically transition into the target keeping
                        // mode after an executed sequence.
                        self.transition_to_target_keeping();
                    }
                    ModeCommandingResult::StepDone => {
                        // Normally, this step is done after all replies were received, but if no
                        // reply checking is required for a command sequence, the step would never
                        // be performed, so this function needs to be called here as well.
                        //self.update_internal_req_id();
                    }
                    ModeCommandingResult::AwaitingSuccessCheck => (),
                }
                Ok(result.into())
            }
        }
    }

    fn transition_to_target_keeping(&mut self) {
        self.state = ModeTreeHelperState::TargetKeeping;
        self.current_mode = self.seq_exec_helper.target_mode().unwrap();
    }

    fn perform_target_keeping(
        &self,
        target_table: &TargetTablesMapValue,
    ) -> Result<(), ModeTreeHelperError> {
        for entry in &target_table.entries {
            if !entry.monitor_state {
                continue;
            }
            let target_mode_violated = false;
            self.children_mode_store.0.iter().for_each(|val| {
                if val.id() == entry.common.target_id {
                    if let Some(ignored_bits) = entry.ignored_bits {
                        let mode_for_check = val.mode & !ignored_bits;
                        let entry_mode_for_check = entry.common.mode & !ignored_bits;
                        mode_for_check != entry_mode_for_check
                    } else {
                        val.mode != entry.common.mode
                    };
                }
            });
            if target_mode_violated {
                // Target keeping violated. Report violation and fallback mode to user.
                return Err(ModeTreeHelperError::TargetKeepingViolation {
                    fallback_mode: target_table.fallback_mode,
                });
            }
        }
        Ok(())
    }

    // Handles a mode reply message and returns whether the reply completes a step of sequence
    // commanding.
    //
    // During mode commanding, it is the responsibility of the user to discard mode responses
    // not related to the current transition. One way to do this is to assigned unique IDs to all
    // requests sent to the device and only handle responses related to those IDs.
    fn handle_children_mode_changed(
        &mut self,
        mode_response: ModeResponse,
    ) -> Result<bool, ModeTreeHelperError> {
        if !self
            .children_mode_store
            .has_component(mode_response.sender_id)
        {
            return Ok(false);
        }
        let mut partial_step_done = false;
        let mut handle_awaition = false;
        if self.state == ModeTreeHelperState::ModeCommanding
            && self.seq_exec_helper.awaiting_success_check()
        {
            handle_awaition = true;
        }
        let still_awating_replies = self.children_mode_store.mode_reply_handler(
            mode_response.sender_id,
            Some(mode_response.reported_mode),
            handle_awaition,
        );
        if self.state == ModeTreeHelperState::ModeCommanding
            && handle_awaition
            && !still_awating_replies.unwrap_or(false)
        {
            self.seq_exec_helper.confirm_sequence_done();
            partial_step_done = true;
        }
        if !mode_response.success && self.state == ModeTreeHelperState::ModeCommanding {
            // The user has to decide how to proceed.
            self.state = ModeTreeHelperState::Idle;
            return Err(ModeTreeHelperError::ModeCommmandFailure {
                seq_table_index: self.seq_exec_helper.current_sequence_index(),
            });
        }
        Ok(partial_step_done)
    }

    pub fn update_child_mode(
        &mut self,
        child: ComponentId,
        mode: ModeRaw,
    ) -> Result<(), TargetNotInModeStoreError> {
        let val_mut = self
            .children_mode_store
            .get_mut(child)
            .ok_or(TargetNotInModeStoreError(child))?;
        val_mut.mode = mode;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::*;

    use crate::{
        ComponentId,
        mode_tree::{
            ModeStoreProvider, ModeStoreVec, SequenceModeTables, SequenceTableEntry,
            SequenceTableMapTable, SequenceTablesMapValue, TargetModeTables,
        },
        subsystem::{ModeCommandingResult, SequenceExecutionHelperState},
    };

    #[derive(Debug)]
    pub enum ExampleTargetId {
        Target0 = 1,
        Target1 = 2,
        Target2 = 3,
    }

    #[derive(Debug)]
    pub enum ExampleMode {
        Mode0 = 1,
        Mode1 = 2,
        Mode2 = 3,
    }

    pub struct SequenceExecutorTestbench {
        pub mode_store: ModeStoreVec,
        pub seq_tables: SequenceModeTables,
        pub execution_helper: SequenceExecutionHelper,
    }

    impl SequenceExecutorTestbench {
        pub fn new() -> Self {
            let mode_store = create_default_mode_store();
            let (seq_tables, _) = create_simple_sample_seq_tables();
            Self {
                mode_store,
                seq_tables,
                execution_helper: SequenceExecutionHelper::new(),
            }
        }

        pub fn get_mode_table(&mut self, mode: ExampleMode) -> &mut SequenceTablesMapValue {
            self.seq_tables.0.get_mut(&(mode as ModeRaw)).unwrap()
        }

        pub fn run(
            &mut self,
            command_list: &mut Vec<ModeSetRequest>,
        ) -> Result<ModeCommandingResult, InvalidSequenceIndexError> {
            self.execution_helper.run(
                self.seq_tables
                    .0
                    .get(&self.execution_helper.target_mode().unwrap())
                    .unwrap(),
                &mut self.mode_store,
                |request| {
                    command_list.push(request);
                },
            )
        }

        fn check_run_is_no_op(&mut self) {
            // Assure that no unexpected behaviour occurs.
            assert_eq!(
                self.execution_helper
                    .run(
                        self.seq_tables
                            .0
                            .get(&self.execution_helper.target_mode().unwrap())
                            .unwrap(),
                        &mut self.mode_store,
                        |_| { panic!("should not have been called") }
                    )
                    .unwrap(),
                ModeCommandingResult::Done
            );
            assert_eq!(
                self.execution_helper.state(),
                SequenceExecutionHelperState::Idle
            );
        }

        fn generic_checks_subsystem_md1_step0(&mut self, mode_set_requests: &[ModeSetRequest]) {
            assert_eq!(
                self.execution_helper.target_mode().unwrap(),
                ExampleMode::Mode1 as ModeRaw
            );
            assert_eq!(mode_set_requests.len(), 2);
            let req_0 = mode_set_requests[0];
            assert_eq!(req_0.target_id, ExampleTargetId::Target0 as ComponentId);
            assert_eq!(req_0.mode, SUBSYSTEM_MD1_ST0_TGT0_MODE);
            let req_1 = mode_set_requests[1];
            assert_eq!(req_1.target_id, ExampleTargetId::Target1 as ComponentId);
            assert_eq!(req_1.mode, SUBSYSTEM_MD1_ST0_TGT1_MODE);
        }
        fn generic_checks_subsystem_md1_step1(&mut self, mode_set_requests: &[ModeSetRequest]) {
            assert_eq!(
                self.execution_helper.target_mode().unwrap(),
                ExampleMode::Mode1 as ModeRaw
            );
            assert_eq!(mode_set_requests.len(), 1);
            let req_0 = mode_set_requests[0];
            assert_eq!(req_0.target_id, ExampleTargetId::Target2 as ComponentId);
            assert_eq!(req_0.mode, SUBSYSTEM_MD1_ST1_TGT2_MODE,);
        }

        fn generic_checks_subsystem_md0(&mut self, mode_set_requests: &[ModeSetRequest]) {
            assert_eq!(
                self.execution_helper.target_mode().unwrap(),
                ExampleMode::Mode0 as ModeRaw
            );
            assert_eq!(self.execution_helper.current_sequence_index().unwrap(), 0);
            assert_eq!(mode_set_requests.len(), 2);
            let req_0 = mode_set_requests[0];
            assert_eq!(req_0.target_id, ExampleTargetId::Target0 as ComponentId);
            assert_eq!(req_0.mode, SUBSYSTEM_MD0_TGT0_MODE);
            let req_1 = mode_set_requests[1];
            assert_eq!(req_1.target_id, ExampleTargetId::Target1 as ComponentId);
            assert_eq!(req_1.mode, SUBSYSTEM_MD0_TGT1_MODE,);
        }
    }

    fn create_default_mode_store() -> ModeStoreVec {
        let mut mode_store = ModeStoreVec::default();
        mode_store
            .add_component(ExampleTargetId::Target0 as ComponentId, 0)
            .unwrap();
        mode_store
            .add_component(ExampleTargetId::Target1 as ComponentId, 0)
            .unwrap();
        mode_store
            .add_component(ExampleTargetId::Target2 as ComponentId, 0)
            .unwrap();
        mode_store
    }

    const SUBSYSTEM_MD0_TGT0_MODE: ModeRaw = ExampleMode::Mode0 as u32;
    const SUBSYSTEM_MD0_TGT1_MODE: ModeRaw = ExampleMode::Mode1 as u32;

    const SUBSYSTEM_MD1_ST0_TGT0_MODE: ModeRaw = ExampleMode::Mode2 as u32;
    const SUBSYSTEM_MD1_ST0_TGT1_MODE: ModeRaw = ExampleMode::Mode0 as u32;
    const SUBSYSTEM_MD1_ST1_TGT2_MODE: ModeRaw = ExampleMode::Mode1 as u32;

    fn create_simple_sample_seq_tables() -> (SequenceModeTables, TargetModeTables) {
        let mut seq_tables = SequenceModeTables::default();
        // Mode 0 - One step command
        let mut table_val = SequenceTablesMapValue::new("MODE_0");
        let mut table_seq_0 = SequenceTableMapTable::new("MODE_0_SEQ_0");
        table_seq_0.add_entry(SequenceTableEntry::new(
            "TARGET_0",
            ExampleTargetId::Target0 as ComponentId,
            SUBSYSTEM_MD0_TGT0_MODE,
            false,
        ));
        table_seq_0.add_entry(SequenceTableEntry::new(
            "TARGET_1",
            ExampleTargetId::Target1 as ComponentId,
            SUBSYSTEM_MD0_TGT1_MODE,
            false,
        ));
        table_val.add_sequence_table(table_seq_0);
        seq_tables.0.insert(ExampleMode::Mode0 as u32, table_val);

        // Mode 1 - Multi Step command
        let mut table_val = SequenceTablesMapValue::new("MODE_1");
        let mut table_seq_0 = SequenceTableMapTable::new("MODE_1_SEQ_0");
        table_seq_0.add_entry(SequenceTableEntry::new(
            "MD1_SEQ0_TGT0",
            ExampleTargetId::Target0 as ComponentId,
            SUBSYSTEM_MD1_ST0_TGT0_MODE,
            false,
        ));
        table_seq_0.add_entry(SequenceTableEntry::new(
            "MD1_SEQ0_TGT1",
            ExampleTargetId::Target1 as ComponentId,
            SUBSYSTEM_MD1_ST0_TGT1_MODE,
            false,
        ));
        table_val.add_sequence_table(table_seq_0);
        let mut table_seq_1 = SequenceTableMapTable::new("MODE_1_SEQ_1");
        table_seq_1.add_entry(SequenceTableEntry::new(
            "MD1_SEQ1_TGT2",
            ExampleTargetId::Target2 as ComponentId,
            SUBSYSTEM_MD1_ST1_TGT2_MODE,
            false,
        ));
        table_val.add_sequence_table(table_seq_1);
        seq_tables.0.insert(ExampleMode::Mode1 as u32, table_val);

        let mode_tables = TargetModeTables::default();
        // TODO: Write mode tables.
        (seq_tables, mode_tables)
    }

    pub struct SubsystemHelperTestbench {
        pub mode_request_queue: Vec<ModeSetRequest>,
        pub helper: SubsystemCommandingHelper,
    }

    impl SubsystemHelperTestbench {
        pub fn new() -> Self {
            let (sequence_tables, target_tables) = create_simple_sample_seq_tables();
            Self {
                helper: SubsystemCommandingHelper::new(
                    create_default_mode_store(),
                    target_tables,
                    sequence_tables,
                ),
                mode_request_queue: Vec::new(),
            }
        }

        pub fn start_command_sequence(
            &mut self,
            mode: ExampleMode,
        ) -> Result<(), StartSequenceError> {
            self.helper.start_command_sequence(mode as ModeRaw)
        }

        #[allow(dead_code)]
        pub fn get_sequence_tables(&mut self, mode: ExampleMode) -> &mut SequenceTablesMapValue {
            self.helper
                .sequence_tables
                .0
                .get_mut(&(mode as ModeRaw))
                .unwrap()
        }

        pub fn state_machine(
            &mut self,
            opt_reply: Option<ModeResponse>,
        ) -> Result<SubsystemHelperResult, ModeTreeHelperError> {
            self.helper.state_machine(opt_reply, |val| {
                self.mode_request_queue.push(val);
            })
        }

        pub fn generic_checks_subsystem_md0(&mut self) {
            assert_eq!(self.mode_request_queue.len(), 2);
            let req0 = self.mode_request_queue[0];
            assert_eq!(req0.target_id, ExampleTargetId::Target0 as ComponentId);
            assert_eq!(req0.mode, SUBSYSTEM_MD0_TGT0_MODE);

            let req1 = self.mode_request_queue[1];
            assert_eq!(req1.target_id, ExampleTargetId::Target1 as ComponentId);
            assert_eq!(req1.mode, SUBSYSTEM_MD0_TGT1_MODE);
        }

        pub fn generic_checks_subsystem_md1_step0(&mut self) {
            assert_eq!(self.mode_request_queue.len(), 2);
            let req0 = self.mode_request_queue[0];
            assert_eq!(req0.target_id, ExampleTargetId::Target0 as ComponentId);
            assert_eq!(req0.mode, SUBSYSTEM_MD1_ST0_TGT0_MODE);

            let req1 = self.mode_request_queue[1];
            assert_eq!(req1.target_id, ExampleTargetId::Target1 as ComponentId);
            assert_eq!(req1.mode, SUBSYSTEM_MD1_ST0_TGT1_MODE);

            self.mode_request_queue.clear();
        }

        pub fn generic_checks_subsystem_md1_step1(&mut self) {
            assert_eq!(self.mode_request_queue.len(), 1);
            let req0 = self.mode_request_queue[0];
            assert_eq!(req0.target_id, ExampleTargetId::Target2 as ComponentId);
            assert_eq!(req0.mode, SUBSYSTEM_MD1_ST1_TGT2_MODE);
            self.mode_request_queue.clear();
        }
    }

    #[test]
    fn test_init_state() {
        let execution_helper = SequenceExecutionHelper::new();
        assert_eq!(execution_helper.state(), SequenceExecutionHelperState::Idle);
        assert!(!execution_helper.awaiting_success_check());
        assert!(execution_helper.target_mode().is_none());
        assert!(execution_helper.current_sequence_index().is_none());
    }

    #[test]
    fn test_sequence_execution_helper_no_success_check() {
        let mut tb = SequenceExecutorTestbench::new();
        tb.execution_helper
            .load(ExampleMode::Mode0 as u32, &tb.seq_tables)
            .unwrap();
        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::Busy
        );
        assert!(!tb.execution_helper.awaiting_success_check());
        assert_eq!(
            tb.execution_helper.target_mode().unwrap(),
            ExampleMode::Mode0 as ModeRaw
        );
        let mut list = Vec::new();
        assert_eq!(
            tb.run(&mut list)
                .expect("sequence exeecution helper run failure"),
            ModeCommandingResult::Done
        );
        assert!(!tb.execution_helper.awaiting_success_check());
        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::Idle
        );
        assert!(!tb.execution_helper.awaiting_success_check());
        tb.generic_checks_subsystem_md0(&list);
        tb.check_run_is_no_op();
    }

    #[test]
    fn test_sequence_execution_helper_with_success_check() {
        let mut tb = SequenceExecutorTestbench::new();
        let mode0_table = tb.get_mode_table(ExampleMode::Mode0);
        mode0_table.entries[0].entries[0].check_success = true;
        mode0_table.entries[0].entries[1].check_success = true;
        tb.execution_helper
            .load(ExampleMode::Mode0 as u32, &tb.seq_tables)
            .unwrap();

        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::Busy
        );
        assert!(!tb.execution_helper.awaiting_success_check());
        assert_eq!(
            tb.execution_helper.target_mode().unwrap(),
            ExampleMode::Mode0 as ModeRaw
        );
        let mut list = Vec::new();
        assert_eq!(
            tb.run(&mut list)
                .expect("sequence exeecution helper run failure"),
            ModeCommandingResult::AwaitingSuccessCheck
        );
        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::AwaitingSuccessCheck
        );
        // These are not cleared, even if the execution helper is already IDLE. This is okay for
        // now.
        assert!(tb.execution_helper.awaiting_success_check());
        tb.generic_checks_subsystem_md0(&list);
        tb.execution_helper.confirm_sequence_done();
        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::Idle
        );

        tb.check_run_is_no_op();
    }

    #[test]
    fn test_sequence_execution_helper_with_partial_check() {
        let mut tb = SequenceExecutorTestbench::new();
        let mode0_table = tb.get_mode_table(ExampleMode::Mode0);
        mode0_table.entries[0].entries[0].check_success = true;
        tb.execution_helper
            .load(ExampleMode::Mode0 as u32, &tb.seq_tables)
            .unwrap();

        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::Busy
        );
        assert!(!tb.execution_helper.awaiting_success_check());
        let mut list = Vec::new();
        assert_eq!(
            tb.run(&mut list)
                .expect("sequence execution helper run failure"),
            ModeCommandingResult::AwaitingSuccessCheck
        );
        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::AwaitingSuccessCheck
        );
        // These are not cleared, even if the execution helper is already IDLE. This is okay for
        // now.
        assert!(tb.execution_helper.awaiting_success_check());
        tb.generic_checks_subsystem_md0(&list);
        tb.execution_helper.confirm_sequence_done();
        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::Idle
        );
        tb.check_run_is_no_op();
    }

    #[test]
    fn test_sequence_execution_helper_multi_step_no_success_check() {
        let mut tb = SequenceExecutorTestbench::new();
        tb.execution_helper
            .load(ExampleMode::Mode1 as u32, &tb.seq_tables)
            .unwrap();
        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::Busy
        );
        assert!(!tb.execution_helper.awaiting_success_check());
        assert_eq!(
            tb.execution_helper.target_mode().unwrap(),
            ExampleMode::Mode1 as ModeRaw
        );
        let mut list = Vec::new();
        assert_eq!(
            tb.run(&mut list)
                .expect("sequence execution helper run failure"),
            ModeCommandingResult::StepDone
        );
        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::Busy
        );
        assert!(!tb.execution_helper.awaiting_success_check());
        tb.generic_checks_subsystem_md1_step0(&list);
        assert_eq!(tb.execution_helper.current_sequence_index().unwrap(), 1);

        list.clear();
        assert_eq!(
            tb.run(&mut list)
                .expect("sequence execution helper run failure"),
            ModeCommandingResult::Done
        );
        tb.generic_checks_subsystem_md1_step1(&list);
        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::Idle
        );
        tb.check_run_is_no_op();
    }

    #[test]
    fn test_sequence_execution_helper_multi_step_full_success_check() {
        let mut tb = SequenceExecutorTestbench::new();
        tb.execution_helper
            .load(ExampleMode::Mode1 as u32, &tb.seq_tables)
            .unwrap();
        let mode1_table = tb.get_mode_table(ExampleMode::Mode1);
        mode1_table.entries[0].entries[0].check_success = true;
        mode1_table.entries[0].entries[1].check_success = true;
        mode1_table.entries[1].entries[0].check_success = true;

        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::Busy
        );
        assert!(!tb.execution_helper.awaiting_success_check());
        assert_eq!(
            tb.execution_helper.target_mode().unwrap(),
            ExampleMode::Mode1 as ModeRaw
        );
        let mut list = Vec::new();
        assert_eq!(
            tb.run(&mut list)
                .expect("sequence execution helper run failure"),
            ModeCommandingResult::AwaitingSuccessCheck
        );
        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::AwaitingSuccessCheck
        );
        assert!(tb.execution_helper.awaiting_success_check());
        tb.generic_checks_subsystem_md1_step0(&list);
        assert_eq!(tb.execution_helper.current_sequence_index().unwrap(), 0);
        tb.execution_helper.confirm_sequence_done();

        list.clear();
        assert_eq!(
            tb.run(&mut list)
                .expect("sequence execution helper run failure"),
            ModeCommandingResult::AwaitingSuccessCheck
        );
        assert_eq!(
            tb.execution_helper.state(),
            SequenceExecutionHelperState::AwaitingSuccessCheck
        );
        assert!(tb.execution_helper.awaiting_success_check());
        assert_eq!(tb.execution_helper.current_sequence_index().unwrap(), 1);
        tb.generic_checks_subsystem_md1_step1(&list);
        tb.execution_helper.confirm_sequence_done();
        tb.check_run_is_no_op();
    }

    #[test]
    fn test_subsystem_helper_basic_state() {
        let tb = SubsystemHelperTestbench::new();
        assert_eq!(tb.helper.state(), ModeTreeHelperState::Idle);
        assert_eq!(tb.helper.mode(), 0);
    }

    #[test]
    fn test_subsystem_helper_cmd_mode0_no_success_checks() {
        let mut tb = SubsystemHelperTestbench::new();
        tb.start_command_sequence(ExampleMode::Mode0).unwrap();
        assert_eq!(tb.helper.state(), ModeTreeHelperState::ModeCommanding);
        assert_eq!(tb.mode_request_queue.len(), 0);
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::Done)
        );
        assert_eq!(tb.helper.state(), ModeTreeHelperState::TargetKeeping);
        assert_eq!(tb.helper.mode(), ExampleMode::Mode0 as ModeRaw);
        tb.generic_checks_subsystem_md0();
        // FSM call should be a no-op.
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::TargetKeeping
        );
        assert_eq!(tb.helper.state(), ModeTreeHelperState::TargetKeeping);
        assert_eq!(tb.helper.mode(), ExampleMode::Mode0 as ModeRaw);
    }

    #[test]
    fn test_subsystem_helper_cmd_mode1_no_success_checks() {
        let mut tb = SubsystemHelperTestbench::new();
        tb.start_command_sequence(ExampleMode::Mode1).unwrap();
        assert_eq!(tb.helper.state(), ModeTreeHelperState::ModeCommanding);
        assert!(tb.mode_request_queue.is_empty());
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::StepDone)
        );
        // Assert that this was already incremented because no reply checking is necessary.
        assert_eq!(tb.helper.state(), ModeTreeHelperState::ModeCommanding);
        assert_eq!(tb.helper.mode(), 0);
        tb.generic_checks_subsystem_md1_step0();
        // Second commanding step.
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::Done)
        );
        assert_eq!(tb.helper.state(), ModeTreeHelperState::TargetKeeping);
        assert_eq!(tb.helper.mode(), ExampleMode::Mode1 as ModeRaw);
        tb.generic_checks_subsystem_md1_step1();

        // FSM call should be a no-op.
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::TargetKeeping
        );
        assert_eq!(tb.helper.state(), ModeTreeHelperState::TargetKeeping);
        assert_eq!(tb.helper.mode(), ExampleMode::Mode1 as ModeRaw);
    }

    #[test]
    fn test_subsystem_helper_cmd_mode0_with_success_checks() {
        let mut tb = SubsystemHelperTestbench::new();
        let seq_tables = tb.get_sequence_tables(ExampleMode::Mode0);
        seq_tables.entries[0].entries[0].check_success = true;
        seq_tables.entries[0].entries[1].check_success = true;
        tb.start_command_sequence(ExampleMode::Mode0).unwrap();
        assert_eq!(tb.helper.state(), ModeTreeHelperState::ModeCommanding);
        assert!(tb.mode_request_queue.is_empty());
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::AwaitingSuccessCheck)
        );
        assert_eq!(tb.helper.state(), ModeTreeHelperState::ModeCommanding);
        assert_eq!(tb.helper.mode(), 0);
        tb.generic_checks_subsystem_md0();
        let mode_reply_ok_0 = ModeResponse {
            request_id: 0,
            sender_id: ExampleTargetId::Target0 as ComponentId,
            reported_mode: SUBSYSTEM_MD0_TGT0_MODE,
            success: true,
        };
        let mode_reply_ok_1 = ModeResponse {
            request_id: 1,
            sender_id: ExampleTargetId::Target1 as ComponentId,
            reported_mode: SUBSYSTEM_MD0_TGT1_MODE,
            success: true,
        };
        // One success reply still expected.
        assert_eq!(
            tb.state_machine(Some(mode_reply_ok_0)).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::AwaitingSuccessCheck)
        );
        assert_eq!(
            tb.state_machine(Some(mode_reply_ok_1)).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::Done)
        );

        // FSM call should be a no-op.
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::TargetKeeping
        );
        assert_eq!(tb.helper.state(), ModeTreeHelperState::TargetKeeping);
        assert_eq!(tb.helper.mode(), ExampleMode::Mode0 as ModeRaw);
    }

    #[test]
    fn test_subsystem_helper_cmd_mode1_with_success_checks() {
        let mut tb = SubsystemHelperTestbench::new();
        let seq_tables = tb.get_sequence_tables(ExampleMode::Mode1);
        seq_tables.entries[0].entries[0].check_success = true;
        seq_tables.entries[0].entries[1].check_success = true;
        seq_tables.entries[1].entries[0].check_success = true;
        tb.start_command_sequence(ExampleMode::Mode1).unwrap();
        assert_eq!(tb.helper.state(), ModeTreeHelperState::ModeCommanding);
        assert!(tb.mode_request_queue.is_empty());
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::AwaitingSuccessCheck)
        );
        assert_eq!(tb.helper.state(), ModeTreeHelperState::ModeCommanding);
        assert_eq!(tb.helper.mode(), 0);
        tb.generic_checks_subsystem_md1_step0();
        let mode_reply_ok_0 = ModeResponse {
            request_id: 0,
            sender_id: ExampleTargetId::Target0 as ComponentId,
            reported_mode: SUBSYSTEM_MD0_TGT0_MODE,
            success: true,
        };
        let mode_reply_ok_1 = ModeResponse {
            request_id: 1,
            sender_id: ExampleTargetId::Target1 as ComponentId,
            reported_mode: SUBSYSTEM_MD0_TGT1_MODE,
            success: true,
        };
        // One success reply still expected.
        assert_eq!(
            tb.state_machine(Some(mode_reply_ok_0)).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::AwaitingSuccessCheck)
        );
        assert_eq!(
            tb.state_machine(Some(mode_reply_ok_1)).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::StepDone)
        );

        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::AwaitingSuccessCheck)
        );
        let mode_reply_ok = ModeResponse {
            request_id: 2,
            sender_id: ExampleTargetId::Target2 as ComponentId,
            reported_mode: SUBSYSTEM_MD1_ST1_TGT2_MODE,
            success: true,
        };
        assert_eq!(
            tb.state_machine(Some(mode_reply_ok)).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::Done)
        );

        // FSM call should be a no-op.
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::TargetKeeping
        );
        assert_eq!(tb.helper.state(), ModeTreeHelperState::TargetKeeping);
        assert_eq!(tb.helper.mode(), ExampleMode::Mode1 as ModeRaw);
    }

    #[test]
    fn test_subsystem_helper_cmd_mode1_with_partial_success_checks_0() {
        let mut tb = SubsystemHelperTestbench::new();
        let seq_tables = tb.get_sequence_tables(ExampleMode::Mode1);
        seq_tables.entries[0].entries[0].check_success = true;
        seq_tables.entries[0].entries[1].check_success = false;
        seq_tables.entries[1].entries[0].check_success = false;
        tb.start_command_sequence(ExampleMode::Mode1).unwrap();
        assert_eq!(tb.helper.state(), ModeTreeHelperState::ModeCommanding);
        assert!(tb.mode_request_queue.is_empty());
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::AwaitingSuccessCheck)
        );
        assert_eq!(tb.helper.state(), ModeTreeHelperState::ModeCommanding);
        assert_eq!(tb.helper.mode(), 0);
        tb.generic_checks_subsystem_md1_step0();
        let mode_reply_ok_0 = ModeResponse {
            request_id: 0,
            sender_id: ExampleTargetId::Target0 as ComponentId,
            reported_mode: SUBSYSTEM_MD0_TGT0_MODE,
            success: true,
        };
        let mode_reply_ok_1 = ModeResponse {
            request_id: 1,
            sender_id: ExampleTargetId::Target1 as ComponentId,
            reported_mode: SUBSYSTEM_MD0_TGT1_MODE,
            success: true,
        };
        // One success reply still expected.
        assert_eq!(
            tb.state_machine(Some(mode_reply_ok_1)).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::AwaitingSuccessCheck)
        );
        assert_eq!(
            tb.state_machine(Some(mode_reply_ok_0)).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::StepDone)
        );

        // Inserting the reply makes no difference: This call completes the sequence commanding.
        let mode_reply_ok = ModeResponse {
            request_id: 2,
            sender_id: ExampleTargetId::Target2 as ComponentId,
            reported_mode: SUBSYSTEM_MD1_ST1_TGT2_MODE,
            success: true,
        };
        assert_eq!(
            tb.state_machine(Some(mode_reply_ok)).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::Done)
        );
        // The internal request ID is still cached.
        tb.generic_checks_subsystem_md1_step1();

        // FSM call should be a no-op.
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::TargetKeeping
        );
        assert_eq!(tb.helper.state(), ModeTreeHelperState::TargetKeeping);
        assert_eq!(tb.helper.mode(), ExampleMode::Mode1 as ModeRaw);
    }

    #[test]
    fn test_subsystem_helper_cmd_mode1_with_partial_success_checks_1() {
        let mut tb = SubsystemHelperTestbench::new();
        let seq_tables = tb.get_sequence_tables(ExampleMode::Mode1);
        seq_tables.entries[0].entries[0].check_success = true;
        seq_tables.entries[0].entries[1].check_success = false;
        seq_tables.entries[1].entries[0].check_success = false;
        tb.start_command_sequence(ExampleMode::Mode1).unwrap();
        assert_eq!(tb.helper.state(), ModeTreeHelperState::ModeCommanding);
        assert!(tb.mode_request_queue.is_empty());
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::AwaitingSuccessCheck)
        );
        assert_eq!(tb.helper.state(), ModeTreeHelperState::ModeCommanding);
        assert_eq!(tb.helper.mode(), 0);
        tb.generic_checks_subsystem_md1_step0();
        let mode_reply_ok_0 = ModeResponse {
            request_id: 0,
            sender_id: ExampleTargetId::Target0 as ComponentId,
            reported_mode: SUBSYSTEM_MD0_TGT0_MODE,
            success: true,
        };
        let mode_reply_ok_1 = ModeResponse {
            request_id: 0,
            sender_id: ExampleTargetId::Target1 as ComponentId,
            reported_mode: SUBSYSTEM_MD0_TGT1_MODE,
            success: true,
        };
        // This completes the step, so the next FSM call will perform the next step
        // in sequence commanding.
        assert_eq!(
            tb.state_machine(Some(mode_reply_ok_0)).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::StepDone)
        );
        assert_eq!(
            tb.state_machine(Some(mode_reply_ok_1)).unwrap(),
            SubsystemHelperResult::ModeCommanding(ModeCommandingResult::Done)
        );

        // Inserting the reply makes no difference: Sequence command is done and target keeping
        // is performed.
        let mode_reply_ok = ModeResponse {
            request_id: 0,
            sender_id: ExampleTargetId::Target2 as ComponentId,
            reported_mode: SUBSYSTEM_MD1_ST1_TGT2_MODE,
            success: true,
        };
        assert_eq!(
            tb.state_machine(Some(mode_reply_ok)).unwrap(),
            SubsystemHelperResult::TargetKeeping
        );
        // The internal request ID is still cached.
        tb.generic_checks_subsystem_md1_step1();

        // FSM call should be a no-op.
        assert_eq!(
            tb.state_machine(None).unwrap(),
            SubsystemHelperResult::TargetKeeping
        );
        assert_eq!(tb.helper.state(), ModeTreeHelperState::TargetKeeping);
        assert_eq!(tb.helper.mode(), ExampleMode::Mode1 as ModeRaw);
    }
}
