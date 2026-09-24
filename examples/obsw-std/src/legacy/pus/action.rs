use log::warn;
use satrs::action::{ActionRequest, ActionRequestVariant};
use satrs::pus::action::{
    ActionReplyPus, ActionReplyVariant, ActivePusActionRequestStd, DefaultActiveActionRequestMap,
};
use satrs::pus::verification::{
    handle_completion_failure_with_generic_params, handle_step_failure_with_generic_params,
    FailParamHelper, FailParams, TcStateAccepted, TcStateStarted, VerificationReporter,
    VerificationReportingProvider, VerificationToken,
};
use satrs::pus::{
    ActiveRequest, EcssTcAndToken, EcssTcCacher, EcssTmSender, EcssTmtcError,
    GenericConversionError, MpscTcReceiver, PusPacketHandlingError, PusReplyHandler,
    PusServiceHelper, PusTcToRequestConverter,
};
use satrs::request::{GenericMessage, UniqueApidTargetId};
use satrs::spacepackets::ecss::tc::PusTcReader;
use satrs::spacepackets::ecss::{EcssEnumU16, PusPacket, PusServiceId};
use satrs_example::config::tmtc_err;
use satrs_example::ids;
use satrs_example::ids::generic_pus::PUS_ACTION;
use std::sync::mpsc;
use std::time::Duration;

use crate::requests::GenericRequestRouter;
use crate::tmtc::sender::TmTcSender;

use super::{
    create_verification_reporter, generic_pus_request_timeout_handler, HandlingStatus,
    PusTargetedRequestService, TargetedPusService,
};

pub struct ActionReplyHandler {
    fail_data_buf: [u8; 128],
}

impl Default for ActionReplyHandler {
    fn default() -> Self {
        Self {
            fail_data_buf: [0; 128],
        }
    }
}

impl PusReplyHandler<ActivePusActionRequestStd, ActionReplyPus> for ActionReplyHandler {
    type Error = EcssTmtcError;

    fn handle_unrequested_reply(
        &mut self,
        reply: &GenericMessage<ActionReplyPus>,
        _tm_sender: &impl EcssTmSender,
    ) -> Result<(), Self::Error> {
        warn!("received unexpected reply for service 8: {reply:?}");
        Ok(())
    }

    fn handle_reply(
        &mut self,
        reply: &GenericMessage<ActionReplyPus>,
        active_request: &ActivePusActionRequestStd,
        tm_sender: &(impl EcssTmSender + ?Sized),
        verification_handler: &impl VerificationReportingProvider,
        timestamp: &[u8],
    ) -> Result<bool, Self::Error> {
        let verif_token: VerificationToken<TcStateStarted> = active_request
            .token()
            .try_into()
            .expect("invalid token state");
        let remove_entry = match &reply.message.variant {
            ActionReplyVariant::CompletionFailed { error_code, params } => {
                let error_propagated = handle_completion_failure_with_generic_params(
                    tm_sender,
                    verif_token,
                    verification_handler,
                    FailParamHelper {
                        error_code,
                        params: params.as_ref(),
                        timestamp,
                        small_data_buf: &mut self.fail_data_buf,
                    },
                )?;
                if !error_propagated {
                    log::warn!(
                        "error params for completion failure were not propated: {:?}",
                        params.as_ref()
                    );
                }
                true
            }
            ActionReplyVariant::StepFailed {
                error_code,
                step,
                params,
            } => {
                let error_propagated = handle_step_failure_with_generic_params(
                    tm_sender,
                    verif_token,
                    verification_handler,
                    FailParamHelper {
                        error_code,
                        params: params.as_ref(),
                        timestamp,
                        small_data_buf: &mut self.fail_data_buf,
                    },
                    &EcssEnumU16::new(*step),
                )?;
                if !error_propagated {
                    log::warn!(
                        "error params for completion failure were not propated: {:?}",
                        params.as_ref()
                    );
                }
                true
            }
            ActionReplyVariant::Completed => {
                verification_handler.completion_success(tm_sender, verif_token, timestamp)?;
                true
            }
            ActionReplyVariant::StepSuccess { step } => {
                verification_handler.step_success(
                    tm_sender,
                    &verif_token,
                    timestamp,
                    EcssEnumU16::new(*step),
                )?;
                false
            }
            _ => false,
        };
        Ok(remove_entry)
    }

    fn handle_request_timeout(
        &mut self,
        active_request: &ActivePusActionRequestStd,
        tm_sender: &impl EcssTmSender,
        verification_handler: &impl VerificationReportingProvider,
        time_stamp: &[u8],
    ) -> Result<(), Self::Error> {
        generic_pus_request_timeout_handler(
            tm_sender,
            active_request,
            verification_handler,
            time_stamp,
            "action",
        )
    }
}

#[derive(Default)]
pub struct ActionRequestConverter {}

impl PusTcToRequestConverter<ActivePusActionRequestStd, ActionRequest> for ActionRequestConverter {
    type Error = GenericConversionError;

    fn convert(
        &mut self,
        token: VerificationToken<TcStateAccepted>,
        tc: &PusTcReader,
        tm_sender: &(impl EcssTmSender + ?Sized),
        verif_reporter: &impl VerificationReportingProvider,
        time_stamp: &[u8],
    ) -> Result<(ActivePusActionRequestStd, ActionRequest), Self::Error> {
        let subservice = tc.message_subtype_id();
        let user_data = tc.user_data();
        if user_data.len() < 8 {
            verif_reporter
                .start_failure(
                    tm_sender,
                    token,
                    FailParams::new_no_fail_data(time_stamp, &tmtc_err::NOT_ENOUGH_APP_DATA),
                )
                .expect("Sending start failure failed");
            return Err(GenericConversionError::NotEnoughAppData {
                expected: 8,
                found: user_data.len(),
            });
        }
        let target_id_and_apid = UniqueApidTargetId::from_pus_tc(tc).unwrap();
        let action_id = u32::from_be_bytes(user_data[4..8].try_into().unwrap());
        if subservice == 128 {
            let req_variant = if user_data.len() == 8 {
                ActionRequestVariant::NoData
            } else {
                ActionRequestVariant::VecData(user_data[8..].to_vec())
            };
            Ok((
                ActivePusActionRequestStd::new(
                    action_id,
                    target_id_and_apid.into(),
                    token.into(),
                    Duration::from_secs(30),
                ),
                ActionRequest::new(action_id, req_variant),
            ))
        } else {
            verif_reporter
                .start_failure(
                    tm_sender,
                    token,
                    FailParams::new_no_fail_data(time_stamp, &tmtc_err::INVALID_PUS_SUBSERVICE),
                )
                .expect("Sending start failure failed");
            Err(GenericConversionError::InvalidSubservice(subservice))
        }
    }
}

pub fn create_action_service(
    tm_sender: TmTcSender,
    tc_in_mem_converter: EcssTcCacher,
    pus_action_rx: mpsc::Receiver<EcssTcAndToken>,
    action_router: GenericRequestRouter,
    reply_receiver: mpsc::Receiver<GenericMessage<ActionReplyPus>>,
) -> ActionServiceWrapper {
    let action_request_handler = PusTargetedRequestService::new(
        PusServiceHelper::new(
            ids::generic_pus::PUS_ACTION.id(),
            pus_action_rx,
            tm_sender,
            create_verification_reporter(PUS_ACTION.id(), PUS_ACTION.apid),
            tc_in_mem_converter,
        ),
        ActionRequestConverter::default(),
        // TODO: Implementation which does not use run-time allocation? Maybe something like
        // a bounded wrapper which pre-allocates using [HashMap::with_capacity]..
        DefaultActiveActionRequestMap::default(),
        ActionReplyHandler::default(),
        action_router,
        reply_receiver,
    );
    ActionServiceWrapper {
        service: action_request_handler,
    }
}

pub struct ActionServiceWrapper {
    pub(crate) service: PusTargetedRequestService<
        MpscTcReceiver,
        VerificationReporter,
        ActionRequestConverter,
        ActionReplyHandler,
        DefaultActiveActionRequestMap,
        ActivePusActionRequestStd,
        ActionRequest,
        ActionReplyPus,
    >,
}

impl TargetedPusService for ActionServiceWrapper {
    const SERVICE_ID: u8 = PusServiceId::Action as u8;
    const SERVICE_STR: &'static str = "action";

    delegate::delegate! {
        to self.service {
            fn poll_and_handle_next_tc(
                &mut self,
                time_stamp: &[u8],
            ) -> Result<HandlingStatus, PusPacketHandlingError>;

            fn poll_and_handle_next_reply(
                &mut self,
                time_stamp: &[u8],
            ) -> Result<HandlingStatus, EcssTmtcError>;

            fn check_for_request_timeouts(&mut self);
        }
    }
}

#[cfg(test)]
mod tests {
    use arbitrary_int::traits::Integer as _;
    use satrs::pus::test_util::{
        TEST_APID, TEST_COMPONENT_ID_0, TEST_COMPONENT_ID_1, TEST_UNIQUE_ID_0, TEST_UNIQUE_ID_1,
    };
    use satrs::pus::verification::test_util::TestVerificationReporter;
    use satrs::pus::{verification, EcssTcVecCacher};
    use satrs::request::MessageMetadata;
    use satrs::spacepackets::ecss::{CreatorConfig, MessageTypeId};
    use satrs::tmtc::PacketAsVec;
    use satrs::ComponentId;
    use satrs::{
        res_code::ResultU16,
        spacepackets::{
            ecss::{
                tc::{PusTcCreator, PusTcSecondaryHeader},
                tm::PusTmReader,
                WritablePusPacket,
            },
            SpHeader,
        },
    };

    use crate::{
        pus::tests::{PusConverterTestbench, ReplyHandlerTestbench, TargetedPusRequestTestbench},
        requests::CompositeRequest,
    };

    use super::*;

    impl
        TargetedPusRequestTestbench<
            ActionRequestConverter,
            ActionReplyHandler,
            DefaultActiveActionRequestMap,
            ActivePusActionRequestStd,
            ActionRequest,
            ActionReplyPus,
        >
    {
        pub fn new_for_action(owner_id: ComponentId, target_id: ComponentId) -> Self {
            let _ = env_logger::builder().is_test(true).try_init();
            let (tm_funnel_tx, tm_funnel_rx) = mpsc::sync_channel(5);
            let (pus_action_tx, pus_action_rx) = mpsc::channel();
            let (action_reply_tx, action_reply_rx) = mpsc::channel();
            let (action_req_tx, action_req_rx) = mpsc::sync_channel(10);
            let verif_reporter = TestVerificationReporter::new(owner_id);
            let mut generic_req_router = GenericRequestRouter::default();
            generic_req_router
                .composite_router_map
                .insert(target_id, action_req_tx);
            Self {
                service: PusTargetedRequestService::new(
                    PusServiceHelper::new(
                        owner_id,
                        pus_action_rx,
                        TmTcSender::Heap(tm_funnel_tx.clone()),
                        verif_reporter,
                        EcssTcCacher::Heap(EcssTcVecCacher::default()),
                    ),
                    ActionRequestConverter::default(),
                    DefaultActiveActionRequestMap::default(),
                    ActionReplyHandler::default(),
                    generic_req_router,
                    action_reply_rx,
                ),
                request_id: None,
                pus_packet_tx: pus_action_tx,
                tm_funnel_rx,
                reply_tx: action_reply_tx,
                request_rx: action_req_rx,
            }
        }

        pub fn verify_packet_started(&self) {
            self.service
                .service_helper
                .common
                .verif_reporter
                .check_next_is_started_success(
                    self.service.service_helper.id(),
                    self.request_id.expect("request ID not set").into(),
                );
        }

        pub fn verify_packet_completed(&self) {
            self.service
                .service_helper
                .common
                .verif_reporter
                .check_next_is_completion_success(
                    self.service.service_helper.id(),
                    self.request_id.expect("request ID not set").into(),
                );
        }

        pub fn verify_tm_empty(&self) {
            let packet = self.tm_funnel_rx.try_recv();
            if let Err(mpsc::TryRecvError::Empty) = packet {
            } else {
                let tm = packet.unwrap();
                let unexpected_tm = PusTmReader::new(&tm.packet, 7).unwrap();
                panic!("unexpected TM packet {unexpected_tm:?}");
            }
        }

        pub fn verify_next_tc_is_handled_properly(&mut self, time_stamp: &[u8]) {
            let result = self.service.poll_and_handle_next_tc(time_stamp);
            if let Err(e) = result {
                panic!("unexpected error {:?}", e);
            }
            let result = result.unwrap();
            match result {
                HandlingStatus::HandledOne => (),
                _ => panic!("unexpected result {result:?}"),
            }
        }

        pub fn verify_all_tcs_handled(&mut self, time_stamp: &[u8]) {
            let result = self.service.poll_and_handle_next_tc(time_stamp);
            if let Err(e) = result {
                panic!("unexpected error {:?}", e);
            }
            let result = result.unwrap();
            match result {
                HandlingStatus::Empty => (),
                _ => panic!("unexpected result {result:?}"),
            }
        }

        pub fn verify_next_reply_is_handled_properly(&mut self, time_stamp: &[u8]) {
            let result = self.service.poll_and_handle_next_reply(time_stamp);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), HandlingStatus::HandledOne);
        }

        pub fn verify_all_replies_handled(&mut self, time_stamp: &[u8]) {
            let result = self.service.poll_and_handle_next_reply(time_stamp);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), HandlingStatus::Empty);
        }

        pub fn add_tc(&mut self, tc: &PusTcCreator) {
            self.request_id = Some(verification::RequestId::new(tc).into());
            let token = self
                .service
                .service_helper
                .verif_reporter_mut()
                .start_verification(tc);
            let accepted_token = self
                .service
                .service_helper
                .verif_reporter()
                .acceptance_success(self.service.service_helper.tm_sender(), token, &[0; 7])
                .expect("TC acceptance failed");
            self.service
                .service_helper
                .verif_reporter()
                .check_next_was_added(accepted_token.request_id());
            let id = self.service.service_helper.id();
            self.service
                .service_helper
                .verif_reporter()
                .check_next_is_acceptance_success(id, accepted_token.request_id());
            self.pus_packet_tx
                .send(EcssTcAndToken::new(
                    PacketAsVec::new(self.service.service_helper.id(), tc.to_vec().unwrap()),
                    accepted_token,
                ))
                .unwrap();
        }
    }

    #[test]
    fn basic_request() {
        let mut testbench = TargetedPusRequestTestbench::new_for_action(
            TEST_COMPONENT_ID_0.id(),
            TEST_COMPONENT_ID_1.id(),
        );
        // Create a basic action request and verify forwarding.
        let sp_header = SpHeader::new_from_apid(TEST_APID);
        let sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(8, 128));
        let action_id = 5_u32;
        let mut app_data: [u8; 8] = [0; 8];
        app_data[0..4].copy_from_slice(&TEST_UNIQUE_ID_1.as_u32().to_be_bytes());
        app_data[4..8].copy_from_slice(&action_id.to_be_bytes());
        let pus8_packet =
            PusTcCreator::new(sp_header, sec_header, &app_data, CreatorConfig::default());
        testbench.add_tc(&pus8_packet);
        let time_stamp: [u8; 7] = [0; 7];
        testbench.verify_next_tc_is_handled_properly(&time_stamp);
        testbench.verify_all_tcs_handled(&time_stamp);

        testbench.verify_packet_started();

        let possible_req = testbench.request_rx.try_recv();
        assert!(possible_req.is_ok());
        let req = possible_req.unwrap();
        if let CompositeRequest::Action(action_req) = req.message {
            assert_eq!(action_req.action_id, action_id);
            assert_eq!(action_req.variant, ActionRequestVariant::NoData);
            let action_reply = ActionReplyPus::new(action_id, ActionReplyVariant::Completed);
            testbench
                .reply_tx
                .send(GenericMessage::new(req.requestor_info, action_reply))
                .unwrap();
        } else {
            panic!("unexpected request type");
        }
        testbench.verify_next_reply_is_handled_properly(&time_stamp);
        testbench.verify_all_replies_handled(&time_stamp);

        testbench.verify_packet_completed();
        testbench.verify_tm_empty();
    }

    #[test]
    fn basic_request_routing_error() {
        let mut testbench = TargetedPusRequestTestbench::new_for_action(
            TEST_COMPONENT_ID_0.id(),
            TEST_COMPONENT_ID_1.id(),
        );
        // Create a basic action request and verify forwarding.
        let sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(8, 128));
        let action_id = 5_u32;
        let mut app_data: [u8; 8] = [0; 8];
        // Invalid ID, routing should fail.
        app_data[0..4].copy_from_slice(&0_u32.to_be_bytes());
        app_data[4..8].copy_from_slice(&action_id.to_be_bytes());
        let pus8_packet = PusTcCreator::new(
            SpHeader::new_from_apid(TEST_APID),
            sec_header,
            &app_data,
            CreatorConfig::default(),
        );
        testbench.add_tc(&pus8_packet);
        let time_stamp: [u8; 7] = [0; 7];

        let result = testbench.service.poll_and_handle_next_tc(&time_stamp);
        assert!(result.is_err());
        // Verify the correct result and completion failure.
    }

    #[test]
    fn converter_action_req_no_data() {
        let mut testbench = PusConverterTestbench::new(
            TEST_COMPONENT_ID_0.raw(),
            ActionRequestConverter::default(),
        );
        let sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(8, 128));
        let action_id = 5_u32;
        let mut app_data: [u8; 8] = [0; 8];
        // Invalid ID, routing should fail.
        app_data[0..4].copy_from_slice(&TEST_UNIQUE_ID_0.as_u32().to_be_bytes());
        app_data[4..8].copy_from_slice(&action_id.to_be_bytes());
        let pus8_packet = PusTcCreator::new(
            SpHeader::new_from_apid(TEST_APID),
            sec_header,
            &app_data,
            CreatorConfig::default(),
        );
        let token = testbench.add_tc(&pus8_packet);
        let result = testbench.convert(token, &[], TEST_APID, TEST_UNIQUE_ID_0);
        assert!(result.is_ok());
        let (active_req, request) = result.unwrap();
        if let ActionRequestVariant::NoData = request.variant {
            assert_eq!(request.action_id, action_id);
            assert_eq!(active_req.action_id, action_id);
            assert_eq!(
                active_req.target_id(),
                UniqueApidTargetId::new(TEST_APID, TEST_UNIQUE_ID_0).raw()
            );
            assert_eq!(
                active_req.token().request_id(),
                testbench.request_id().unwrap()
            );
        } else {
            panic!("unexpected action request variant");
        }
    }

    #[test]
    fn converter_action_req_with_data() {
        let mut testbench =
            PusConverterTestbench::new(TEST_COMPONENT_ID_0.id(), ActionRequestConverter::default());
        let sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(8, 128));
        let action_id = 5_u32;
        let mut app_data: [u8; 16] = [0; 16];
        // Invalid ID, routing should fail.
        app_data[0..4].copy_from_slice(&TEST_UNIQUE_ID_0.as_u32().to_be_bytes());
        app_data[4..8].copy_from_slice(&action_id.to_be_bytes());
        for i in 0..8 {
            app_data[i + 8] = i as u8;
        }
        let pus8_packet = PusTcCreator::new(
            SpHeader::new_from_apid(TEST_APID),
            sec_header,
            &app_data,
            CreatorConfig::default(),
        );
        let token = testbench.add_tc(&pus8_packet);
        let result = testbench.convert(token, &[], TEST_APID, TEST_UNIQUE_ID_0);
        assert!(result.is_ok());
        let (active_req, request) = result.unwrap();
        if let ActionRequestVariant::VecData(vec) = request.variant {
            assert_eq!(request.action_id, action_id);
            assert_eq!(active_req.action_id, action_id);
            assert_eq!(vec, app_data[8..].to_vec());
        } else {
            panic!("unexpected action request variant");
        }
    }

    #[test]
    fn reply_handling_completion_success() {
        let mut testbench =
            ReplyHandlerTestbench::new(TEST_COMPONENT_ID_0.id(), ActionReplyHandler::default());
        let action_id = 5_u32;
        let (req_id, active_req) = testbench.add_tc(TEST_APID, TEST_UNIQUE_ID_0, &[]);
        let active_action_req =
            ActivePusActionRequestStd::new_from_common_req(action_id, active_req);
        let reply = ActionReplyPus::new(action_id, ActionReplyVariant::Completed);
        let generic_reply = GenericMessage::new(MessageMetadata::new(req_id.into(), 0), reply);
        let result = testbench.handle_reply(&generic_reply, &active_action_req, &[]);
        assert!(result.is_ok());
        assert!(result.unwrap());
        testbench.verif_reporter.assert_full_completion_success(
            TEST_COMPONENT_ID_0.id(),
            req_id,
            None,
        );
    }

    #[test]
    fn reply_handling_completion_failure() {
        let mut testbench =
            ReplyHandlerTestbench::new(TEST_COMPONENT_ID_0.id(), ActionReplyHandler::default());
        let action_id = 5_u32;
        let (req_id, active_req) = testbench.add_tc(TEST_APID, TEST_UNIQUE_ID_0, &[]);
        let active_action_req =
            ActivePusActionRequestStd::new_from_common_req(action_id, active_req);
        let error_code = ResultU16::new(2, 3);
        let reply = ActionReplyPus::new(
            action_id,
            ActionReplyVariant::CompletionFailed {
                error_code,
                params: None,
            },
        );
        let generic_reply = GenericMessage::new(MessageMetadata::new(req_id.into(), 0), reply);
        let result = testbench.handle_reply(&generic_reply, &active_action_req, &[]);
        assert!(result.is_ok());
        assert!(result.unwrap());
        testbench.verif_reporter.assert_completion_failure(
            TEST_COMPONENT_ID_0.into(),
            req_id,
            None,
            error_code.raw() as u64,
        );
    }

    #[test]
    fn reply_handling_step_success() {
        let mut testbench =
            ReplyHandlerTestbench::new(TEST_COMPONENT_ID_0.id(), ActionReplyHandler::default());
        let action_id = 5_u32;
        let (req_id, active_req) = testbench.add_tc(TEST_APID, TEST_UNIQUE_ID_0, &[]);
        let active_action_req =
            ActivePusActionRequestStd::new_from_common_req(action_id, active_req);
        let reply = ActionReplyPus::new(action_id, ActionReplyVariant::StepSuccess { step: 1 });
        let generic_reply = GenericMessage::new(MessageMetadata::new(req_id.into(), 0), reply);
        let result = testbench.handle_reply(&generic_reply, &active_action_req, &[]);
        assert!(result.is_ok());
        // Entry should not be removed, completion not done yet.
        assert!(!result.unwrap());
        testbench.verif_reporter.check_next_was_added(req_id);
        testbench
            .verif_reporter
            .check_next_is_acceptance_success(TEST_COMPONENT_ID_0.raw(), req_id);
        testbench
            .verif_reporter
            .check_next_is_started_success(TEST_COMPONENT_ID_0.raw(), req_id);
        testbench
            .verif_reporter
            .check_next_is_step_success(TEST_COMPONENT_ID_0.raw(), req_id, 1);
    }

    #[test]
    fn reply_handling_step_failure() {
        let mut testbench =
            ReplyHandlerTestbench::new(TEST_COMPONENT_ID_0.id(), ActionReplyHandler::default());
        let action_id = 5_u32;
        let (req_id, active_req) = testbench.add_tc(TEST_APID, TEST_UNIQUE_ID_0, &[]);
        let active_action_req =
            ActivePusActionRequestStd::new_from_common_req(action_id, active_req);
        let error_code = ResultU16::new(2, 3);
        let reply = ActionReplyPus::new(
            action_id,
            ActionReplyVariant::StepFailed {
                error_code,
                step: 1,
                params: None,
            },
        );
        let generic_reply = GenericMessage::new(MessageMetadata::new(req_id.into(), 0), reply);
        let result = testbench.handle_reply(&generic_reply, &active_action_req, &[]);
        assert!(result.is_ok());
        assert!(result.unwrap());
        testbench.verif_reporter.check_next_was_added(req_id);
        testbench
            .verif_reporter
            .check_next_is_acceptance_success(TEST_COMPONENT_ID_0.id(), req_id);
        testbench
            .verif_reporter
            .check_next_is_started_success(TEST_COMPONENT_ID_0.id(), req_id);
        testbench.verif_reporter.check_next_is_step_failure(
            TEST_COMPONENT_ID_0.id(),
            req_id,
            error_code.raw().into(),
        );
    }

    #[test]
    fn reply_handling_unrequested_reply() {
        let mut testbench =
            ReplyHandlerTestbench::new(TEST_COMPONENT_ID_0.id(), ActionReplyHandler::default());
        let action_reply = ActionReplyPus::new(5_u32, ActionReplyVariant::Completed);
        let unrequested_reply =
            GenericMessage::new(MessageMetadata::new(10_u32, 15_u32), action_reply);
        // Right now this function does not do a lot. We simply check that it does not panic or do
        // weird stuff.
        let result = testbench.handle_unrequested_reply(&unrequested_reply);
        assert!(result.is_ok());
    }

    #[test]
    fn reply_handling_reply_timeout() {
        let mut testbench =
            ReplyHandlerTestbench::new(TEST_COMPONENT_ID_0.id(), ActionReplyHandler::default());
        let action_id = 5_u32;
        let (req_id, active_request) = testbench.add_tc(TEST_APID, TEST_UNIQUE_ID_0, &[]);
        let result = testbench.handle_request_timeout(
            &ActivePusActionRequestStd::new_from_common_req(action_id, active_request),
            &[],
        );
        assert!(result.is_ok());
        testbench.verif_reporter.assert_completion_failure(
            TEST_COMPONENT_ID_0.raw(),
            req_id,
            None,
            tmtc_err::REQUEST_TIMEOUT.raw() as u64,
        );
    }
}
