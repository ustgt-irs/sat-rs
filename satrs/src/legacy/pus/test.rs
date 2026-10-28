use super::{
    DirectPusPacketHandlerResult, PartialPusHandlingError, PusPacketHandlingError, PusTmVariant,
};
use crate::tmtc::{PacketAsVec, PacketSenderWithSharedPool};
use arbitrary_int::traits::Integer as _;
use arbitrary_int::u14;
use spacepackets::SpHeader;
use spacepackets::ecss::tm::{PusTmCreator, PusTmSecondaryHeader};
use spacepackets::ecss::{CreatorConfig, MessageTypeId, PusPacket};
use std::sync::mpsc;

use super::verification::{VerificationReporter, VerificationReportingProvider};
use super::{
    CacheAndReadRawEcssTc, EcssTcInSharedPoolCacher, EcssTcReceiver, EcssTcVecCacher, EcssTmSender,
    GenericConversionError, MpscTcReceiver, PusServiceHelper,
};
use crate::HandlingStatus;

/// This is a helper class for [std] environments to handle generic PUS 17 (test service) packets.
/// This handler only processes ping requests and generates a ping reply for them accordingly.
pub struct PusService17TestHandler<
    TcReceiver: EcssTcReceiver,
    TmSender: EcssTmSender,
    TcInMemConverter: CacheAndReadRawEcssTc,
    VerificationReporter: VerificationReportingProvider,
> {
    pub service_helper:
        PusServiceHelper<TcReceiver, TmSender, TcInMemConverter, VerificationReporter>,
}

impl<
    TcReceiver: EcssTcReceiver,
    TmSender: EcssTmSender,
    TcInMemConverter: CacheAndReadRawEcssTc,
    VerificationReporter: VerificationReportingProvider,
> PusService17TestHandler<TcReceiver, TmSender, TcInMemConverter, VerificationReporter>
{
    pub fn new(
        service_helper: PusServiceHelper<
            TcReceiver,
            TmSender,
            TcInMemConverter,
            VerificationReporter,
        >,
    ) -> Self {
        Self { service_helper }
    }

    pub fn poll_and_handle_next_tc<ErrorCb: FnMut(&PartialPusHandlingError)>(
        &mut self,
        mut error_callback: ErrorCb,
        time_stamp: &[u8],
    ) -> Result<DirectPusPacketHandlerResult, PusPacketHandlingError> {
        let possible_packet = self.service_helper.retrieve_and_accept_next_packet()?;
        if possible_packet.is_none() {
            return Ok(HandlingStatus::Empty.into());
        }
        let ecss_tc_and_token = possible_packet.unwrap();
        self.service_helper
            .tc_in_mem_converter_mut()
            .cache(&ecss_tc_and_token.tc_in_memory)?;
        let tc = self.service_helper.tc_in_mem_converter().convert()?;
        if tc.service_type_id() != 17 {
            return Err(GenericConversionError::WrongService(tc.service_type_id()).into());
        }
        if tc.message_subtype_id() == 1 {
            let opt_started_token = match self.service_helper.verif_reporter().start_success(
                &self.service_helper.common.tm_sender,
                ecss_tc_and_token.token,
                time_stamp,
            ) {
                Ok(token) => Some(token),
                Err(e) => {
                    error_callback(&PartialPusHandlingError::Verification(e));
                    None
                }
            };
            // Sequence count will be handled centrally in TM funnel.
            // It is assumed that the verification reporter was built with a valid APID, so we use
            // the unchecked API here.
            let reply_header = SpHeader::new_for_unseg_tm(
                self.service_helper.verif_reporter().apid(),
                u14::ZERO,
                0,
            );
            let tc_header = PusTmSecondaryHeader::new_simple(MessageTypeId::new(17, 2), time_stamp);
            let ping_reply =
                PusTmCreator::new(reply_header, tc_header, &[], CreatorConfig::default());
            if let Err(e) = self
                .service_helper
                .common
                .tm_sender
                .send_tm(self.service_helper.id(), PusTmVariant::Direct(ping_reply))
            {
                error_callback(&PartialPusHandlingError::TmSend(e));
            }
            if let Some(start_token) = opt_started_token
                && let Err(e) = self.service_helper.verif_reporter().completion_success(
                    &self.service_helper.common.tm_sender,
                    start_token,
                    time_stamp,
                )
            {
                error_callback(&PartialPusHandlingError::Verification(e));
            }
        } else {
            return Ok(DirectPusPacketHandlerResult::CustomSubservice(
                tc.message_subtype_id(),
                ecss_tc_and_token.token,
            ));
        }
        Ok(HandlingStatus::HandledOne.into())
    }
}

/// Helper type definition for a PUS 17 handler with a dynamic TMTC memory backend and regular
/// mpsc queues.
pub type PusService17TestHandlerDynWithMpsc = PusService17TestHandler<
    MpscTcReceiver,
    mpsc::Sender<PacketAsVec>,
    EcssTcVecCacher,
    VerificationReporter,
>;
/// Helper type definition for a PUS 17 handler with a dynamic TMTC memory backend and bounded MPSC
/// queues.
pub type PusService17TestHandlerDynWithBoundedMpsc = PusService17TestHandler<
    MpscTcReceiver,
    mpsc::SyncSender<PacketAsVec>,
    EcssTcVecCacher,
    VerificationReporter,
>;
/// Helper type definition for a PUS 17 handler with a shared store TMTC memory backend and bounded
/// mpsc queues.
pub type PusService17TestHandlerStaticWithBoundedMpsc = PusService17TestHandler<
    MpscTcReceiver,
    PacketSenderWithSharedPool,
    EcssTcInSharedPoolCacher,
    VerificationReporter,
>;

#[cfg(test)]
mod tests {
    use crate::ComponentId;
    use crate::HandlingStatus;
    use crate::legacy::pus::test_util::{PusTestHarness, SimplePusPacketHandler, TEST_APID};
    use crate::legacy::pus::tests::{
        PusServiceHandlerWithSharedStoreCommon, PusServiceHandlerWithVecCommon,
    };
    use crate::legacy::pus::verification::{
        RequestId, VerificationReporter, VerificationReportingProvider,
    };
    use crate::legacy::pus::verification::{TcStateAccepted, VerificationToken};
    use crate::legacy::pus::{
        DirectPusPacketHandlerResult, EcssTcInSharedPoolCacher, EcssTcVecCacher,
        GenericConversionError, MpscTcReceiver, MpscTmAsVecSender, PartialPusHandlingError,
        PusPacketHandlingError,
    };
    use crate::tmtc::PacketSenderWithSharedPool;
    use arbitrary_int::traits::Integer as _;
    use arbitrary_int::u14;
    use delegate::delegate;
    use spacepackets::SpHeader;
    use spacepackets::ecss::tc::{PusTcCreator, PusTcSecondaryHeader};
    use spacepackets::ecss::tm::PusTmReader;
    use spacepackets::ecss::{CreatorConfig, MessageTypeId, PusPacket};
    use spacepackets::time::cds;

    use super::PusService17TestHandler;

    struct Pus17HandlerWithStoreTester {
        common: PusServiceHandlerWithSharedStoreCommon,
        handler: PusService17TestHandler<
            MpscTcReceiver,
            PacketSenderWithSharedPool,
            EcssTcInSharedPoolCacher,
            VerificationReporter,
        >,
    }

    impl Pus17HandlerWithStoreTester {
        pub fn new(id: ComponentId) -> Self {
            let (common, srv_handler) = PusServiceHandlerWithSharedStoreCommon::new(id);
            let pus_17_handler = PusService17TestHandler::new(srv_handler);
            Self {
                common,
                handler: pus_17_handler,
            }
        }
    }

    impl PusTestHarness for Pus17HandlerWithStoreTester {
        fn start_verification(&mut self, tc: &PusTcCreator) -> VerificationToken<TcStateAccepted> {
            let init_token = self
                .handler
                .service_helper
                .verif_reporter_mut()
                .start_verification(tc);
            self.handler
                .service_helper
                .verif_reporter()
                .acceptance_success(self.handler.service_helper.tm_sender(), init_token, &[0; 7])
                .expect("acceptance success failure")
        }

        fn send_tc(&self, token: &VerificationToken<TcStateAccepted>, tc: &PusTcCreator) {
            self.common
                .send_tc(self.handler.service_helper.id(), token, tc);
        }

        delegate! {
            to self.common {
                fn read_next_tm(&mut self) -> PusTmReader<'_>;
                fn check_no_tm_available(&self) -> bool;
                fn check_next_verification_tm(
                    &self,
                    subservice: u8,
                    expected_request_id: RequestId
                );
            }
        }
    }
    impl SimplePusPacketHandler for Pus17HandlerWithStoreTester {
        fn handle_one_tc(
            &mut self,
        ) -> Result<DirectPusPacketHandlerResult, PusPacketHandlingError> {
            let time_stamp = cds::CdsTime::new_with_u16_days(0, 0).to_vec().unwrap();
            self.handler
                .poll_and_handle_next_tc(|_partial_error: &PartialPusHandlingError| {}, &time_stamp)
        }
    }

    struct Pus17HandlerWithVecTester {
        common: PusServiceHandlerWithVecCommon,
        handler: PusService17TestHandler<
            MpscTcReceiver,
            MpscTmAsVecSender,
            EcssTcVecCacher,
            VerificationReporter,
        >,
    }

    impl Pus17HandlerWithVecTester {
        pub fn new(id: ComponentId) -> Self {
            let (common, srv_handler) =
                PusServiceHandlerWithVecCommon::new_with_standard_verif_reporter(id);
            Self {
                common,
                handler: PusService17TestHandler::new(srv_handler),
            }
        }
    }

    impl PusTestHarness for Pus17HandlerWithVecTester {
        fn start_verification(&mut self, tc: &PusTcCreator) -> VerificationToken<TcStateAccepted> {
            let init_token = self
                .handler
                .service_helper
                .verif_reporter_mut()
                .start_verification(tc);
            self.handler
                .service_helper
                .verif_reporter()
                .acceptance_success(self.handler.service_helper.tm_sender(), init_token, &[0; 7])
                .expect("acceptance success failure")
        }

        fn send_tc(&self, token: &VerificationToken<TcStateAccepted>, tc: &PusTcCreator) {
            self.common
                .send_tc(self.handler.service_helper.id(), token, tc);
        }

        delegate! {
            to self.common {
                fn read_next_tm(&mut self) -> PusTmReader<'_>;
                fn check_no_tm_available(&self) -> bool;
                fn check_next_verification_tm(
                    &self,
                    subservice: u8,
                    expected_request_id: RequestId,
                );
            }
        }
    }
    impl SimplePusPacketHandler for Pus17HandlerWithVecTester {
        fn handle_one_tc(
            &mut self,
        ) -> Result<DirectPusPacketHandlerResult, PusPacketHandlingError> {
            let time_stamp = cds::CdsTime::new_with_u16_days(0, 0).to_vec().unwrap();
            self.handler
                .poll_and_handle_next_tc(|_partial_error: &PartialPusHandlingError| {}, &time_stamp)
        }
    }

    fn ping_test(test_harness: &mut (impl PusTestHarness + SimplePusPacketHandler)) {
        // Create a ping TC, verify acceptance.
        let sp_header = SpHeader::new_for_unseg_tc(TEST_APID, u14::ZERO, 0);
        let sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(17, 1));
        let ping_tc =
            PusTcCreator::new_no_app_data(sp_header, sec_header, CreatorConfig::default());
        let token = test_harness.start_verification(&ping_tc);
        test_harness.send_tc(&token, &ping_tc);
        let request_id = token.request_id();
        let result = test_harness.handle_one_tc();
        assert!(result.is_ok());
        // We should see 4 replies in the TM queue now: Acceptance TM, Start TM, ping reply and
        // Completion TM

        // Acceptance TM
        test_harness.check_next_verification_tm(1, request_id);

        // Start TM
        test_harness.check_next_verification_tm(3, request_id);

        // Ping reply
        let tm = test_harness.read_next_tm();
        assert_eq!(tm.service_type_id(), 17);
        assert_eq!(tm.message_subtype_id(), 2);
        assert!(tm.user_data().is_empty());

        // TM completion
        test_harness.check_next_verification_tm(7, request_id);
    }

    #[test]
    fn test_basic_ping_processing_using_store() {
        let mut test_harness = Pus17HandlerWithStoreTester::new(0);
        ping_test(&mut test_harness);
    }

    #[test]
    fn test_basic_ping_processing_using_vec() {
        let mut test_harness = Pus17HandlerWithVecTester::new(0);
        ping_test(&mut test_harness);
    }

    #[test]
    fn test_empty_tc_queue() {
        let mut test_harness = Pus17HandlerWithStoreTester::new(0);
        let result = test_harness.handle_one_tc();
        assert!(result.is_ok());
        match result.unwrap() {
            DirectPusPacketHandlerResult::Handled(handled) => {
                assert_eq!(handled, HandlingStatus::Empty);
            }
            _ => panic!("unexpected result"),
        }
    }

    #[test]
    fn test_sending_unsupported_service() {
        let mut test_harness = Pus17HandlerWithStoreTester::new(0);
        let sp_header = SpHeader::new_for_unseg_tc(TEST_APID, u14::ZERO, 0);
        let sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(3, 1));
        let ping_tc =
            PusTcCreator::new_no_app_data(sp_header, sec_header, CreatorConfig::default());
        let token = test_harness.start_verification(&ping_tc);
        test_harness.send_tc(&token, &ping_tc);
        let result = test_harness.handle_one_tc();
        assert!(result.is_err());
        let error = result.unwrap_err();
        if let PusPacketHandlingError::RequestConversion(GenericConversionError::WrongService(
            num,
        )) = error
        {
            assert_eq!(num, 3);
        } else {
            panic!("unexpected error type {error}")
        }
    }

    #[test]
    fn test_sending_custom_subservice() {
        let mut test_harness = Pus17HandlerWithStoreTester::new(0);
        let sp_header = SpHeader::new_for_unseg_tc(TEST_APID, u14::ZERO, 0);
        let sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(17, 200));
        let ping_tc =
            PusTcCreator::new_no_app_data(sp_header, sec_header, CreatorConfig::default());
        let token = test_harness.start_verification(&ping_tc);
        test_harness.send_tc(&token, &ping_tc);
        let result = test_harness.handle_one_tc();
        assert!(result.is_ok());
        let result = result.unwrap();
        if let DirectPusPacketHandlerResult::CustomSubservice(subservice, _) = result {
            assert_eq!(subservice, 200);
        } else {
            panic!("unexpected result type {result:?}")
        }
    }
}
