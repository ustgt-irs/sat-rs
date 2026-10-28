use super::PusPacketHandlingError;
use super::scheduler::PusScheduler;
use super::verification::{VerificationReporter, VerificationReportingProvider};
use super::{
    CacheAndReadRawEcssTc, DirectPusPacketHandlerResult, EcssTcInSharedPoolCacher, EcssTcReceiver,
    EcssTcVecCacher, EcssTmSender, MpscTcReceiver, PartialPusHandlingError, PusServiceHelper,
};
use crate::HandlingStatus;
use crate::pool::PoolProvider;
use crate::tmtc::{PacketAsVec, PacketSenderWithSharedPool};
use alloc::string::ToString;
use spacepackets::ecss::{PusPacket, scheduling};
use spacepackets::time::cds::CdsTime;
use std::sync::mpsc;

/// This is a helper class for [std] environments to handle generic PUS 11 (scheduling service)
/// packets. This handler is able to handle the most important PUS requests for a scheduling
/// service which provides the [PusScheduler].
///
/// Please note that this class does not do the regular periodic handling like releasing any
/// telecommands inside the scheduler. The user can retrieve the wrapped scheduler via the
/// [Self::scheduler] and [Self::scheduler_mut] function and then use the scheduler API to release
/// telecommands when applicable.
pub struct PusSchedServiceHandler<
    TcReceiver: EcssTcReceiver,
    TmSender: EcssTmSender,
    TcInMemConverter: CacheAndReadRawEcssTc,
    VerificationReporter: VerificationReportingProvider,
    PusSchedulerInstance: PusScheduler,
> {
    pub service_helper:
        PusServiceHelper<TcReceiver, TmSender, TcInMemConverter, VerificationReporter>,
    scheduler: PusSchedulerInstance,
}

impl<
    TcReceiver: EcssTcReceiver,
    TmSender: EcssTmSender,
    TcInMemConverter: CacheAndReadRawEcssTc,
    VerificationReporter: VerificationReportingProvider,
    Scheduler: PusScheduler,
> PusSchedServiceHandler<TcReceiver, TmSender, TcInMemConverter, VerificationReporter, Scheduler>
{
    pub fn new(
        service_helper: PusServiceHelper<
            TcReceiver,
            TmSender,
            TcInMemConverter,
            VerificationReporter,
        >,
        scheduler: Scheduler,
    ) -> Self {
        Self {
            service_helper,
            scheduler,
        }
    }

    pub fn scheduler_mut(&mut self) -> &mut Scheduler {
        &mut self.scheduler
    }

    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    pub fn poll_and_handle_next_tc<ErrorCb: FnMut(&PartialPusHandlingError)>(
        &mut self,
        mut error_callback: ErrorCb,
        time_stamp: &[u8],
        sched_tc_pool: &mut (impl PoolProvider + ?Sized),
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
        let subservice = PusPacket::message_subtype_id(&tc);
        let standard_subservice = scheduling::MessageSubtypeId::try_from(subservice);
        if standard_subservice.is_err() {
            return Ok(DirectPusPacketHandlerResult::CustomSubservice(
                subservice,
                ecss_tc_and_token.token,
            ));
        }
        match standard_subservice.unwrap() {
            scheduling::MessageSubtypeId::TcEnableScheduling => {
                let opt_started_token = match self.service_helper.verif_reporter().start_success(
                    &self.service_helper.common.tm_sender,
                    ecss_tc_and_token.token,
                    time_stamp,
                ) {
                    Ok(started_token) => Some(started_token),
                    Err(e) => {
                        error_callback(&PartialPusHandlingError::Verification(e));
                        None
                    }
                };
                self.scheduler.enable();
                if self.scheduler.is_enabled()
                    && let Some(started_token) = opt_started_token
                {
                    if let Err(e) = self.service_helper.verif_reporter().completion_success(
                        &self.service_helper.common.tm_sender,
                        started_token,
                        time_stamp,
                    ) {
                        error_callback(&PartialPusHandlingError::Verification(e));
                    }
                } else {
                    return Err(PusPacketHandlingError::Other(
                        "failed to enabled scheduler".to_string(),
                    ));
                }
            }
            scheduling::MessageSubtypeId::TcDisableScheduling => {
                let opt_started_token = match self.service_helper.verif_reporter().start_success(
                    &self.service_helper.common.tm_sender,
                    ecss_tc_and_token.token,
                    time_stamp,
                ) {
                    Ok(started_token) => Some(started_token),
                    Err(e) => {
                        error_callback(&PartialPusHandlingError::Verification(e));
                        None
                    }
                };

                self.scheduler.disable();
                if !self.scheduler.is_enabled()
                    && let Some(started_token) = opt_started_token
                {
                    if let Err(e) = self.service_helper.verif_reporter().completion_success(
                        &self.service_helper.common.tm_sender,
                        started_token,
                        time_stamp,
                    ) {
                        error_callback(&PartialPusHandlingError::Verification(e));
                    }
                } else {
                    return Err(PusPacketHandlingError::Other(
                        "failed to disable scheduler".to_string(),
                    ));
                }
            }
            scheduling::MessageSubtypeId::TcResetScheduling => {
                let start_token = self
                    .service_helper
                    .verif_reporter()
                    .start_success(
                        &self.service_helper.common.tm_sender,
                        ecss_tc_and_token.token,
                        time_stamp,
                    )
                    .expect("Error sending start success");

                self.scheduler
                    .reset(sched_tc_pool)
                    .expect("Error resetting TC Pool");

                self.service_helper
                    .verif_reporter()
                    .completion_success(
                        &self.service_helper.common.tm_sender,
                        start_token,
                        time_stamp,
                    )
                    .expect("Error sending completion success");
            }
            scheduling::MessageSubtypeId::TcInsertActivity => {
                let start_token = self
                    .service_helper
                    .common
                    .verif_reporter
                    .start_success(
                        &self.service_helper.common.tm_sender,
                        ecss_tc_and_token.token,
                        time_stamp,
                    )
                    .expect("error sending start success");

                // let mut pool = self.sched_tc_pool.write().expect("locking pool failed");
                self.scheduler
                    .insert_wrapped_tc::<CdsTime>(&tc, sched_tc_pool)
                    .expect("insertion of activity into pool failed");

                self.service_helper
                    .verif_reporter()
                    .completion_success(
                        &self.service_helper.common.tm_sender,
                        start_token,
                        time_stamp,
                    )
                    .expect("sending completion success failed");
            }
            _ => {
                // Treat unhandled standard subservices as custom subservices for now.
                return Ok(DirectPusPacketHandlerResult::CustomSubservice(
                    subservice,
                    ecss_tc_and_token.token,
                ));
            }
        }
        Ok(HandlingStatus::HandledOne.into())
    }
}
/// Helper type definition for a PUS 11 handler with a dynamic TMTC memory backend and regular
/// mpsc queues.
pub type PusService11SchedHandlerDynWithMpsc<PusScheduler> = PusSchedServiceHandler<
    MpscTcReceiver,
    mpsc::Sender<PacketAsVec>,
    EcssTcVecCacher,
    VerificationReporter,
    PusScheduler,
>;
/// Helper type definition for a PUS 11 handler with a dynamic TMTC memory backend and bounded MPSC
/// queues.
pub type PusService11SchedHandlerDynWithBoundedMpsc<PusScheduler> = PusSchedServiceHandler<
    MpscTcReceiver,
    mpsc::SyncSender<PacketAsVec>,
    EcssTcVecCacher,
    VerificationReporter,
    PusScheduler,
>;
/// Helper type definition for a PUS 11 handler with a shared store TMTC memory backend and regular
/// mpsc queues.
pub type PusService11SchedHandlerStaticWithMpsc<PusScheduler> = PusSchedServiceHandler<
    MpscTcReceiver,
    PacketSenderWithSharedPool,
    EcssTcInSharedPoolCacher,
    VerificationReporter,
    PusScheduler,
>;
/// Helper type definition for a PUS 11 handler with a shared store TMTC memory backend and bounded
/// mpsc queues.
pub type PusService11SchedHandlerStaticWithBoundedMpsc<PusScheduler> = PusSchedServiceHandler<
    MpscTcReceiver,
    PacketSenderWithSharedPool,
    EcssTcInSharedPoolCacher,
    VerificationReporter,
    PusScheduler,
>;

#[cfg(test)]
mod tests {
    use crate::legacy::pus::test_util::{PusTestHarness, TEST_APID};
    use crate::legacy::pus::verification::{VerificationReporter, VerificationReportingProvider};
    use crate::pool::{StaticMemoryPool, StaticPoolConfig};

    use crate::legacy::pus::{
        DirectPusPacketHandlerResult, MpscTcReceiver, PusPacketHandlingError,
    };
    use crate::legacy::pus::{
        EcssTcInSharedPoolCacher,
        scheduler::{self, PusScheduler, TcInfo},
        tests::PusServiceHandlerWithSharedStoreCommon,
        verification::{RequestId, TcStateAccepted, VerificationToken},
    };
    use crate::tmtc::PacketSenderWithSharedPool;
    use alloc::collections::VecDeque;
    use arbitrary_int::traits::Integer as _;
    use arbitrary_int::u14;
    use delegate::delegate;
    use spacepackets::SpHeader;
    use spacepackets::ecss::scheduling::MessageSubtypeId;
    use spacepackets::ecss::tc::PusTcSecondaryHeader;
    use spacepackets::ecss::{CreatorConfig, MessageTypeId};
    use spacepackets::{
        ecss::{tc::PusTcCreator, tm::PusTmReader},
        time::cds,
    };

    use super::PusSchedServiceHandler;

    struct Pus11HandlerWithStoreTester {
        common: PusServiceHandlerWithSharedStoreCommon,
        handler: PusSchedServiceHandler<
            MpscTcReceiver,
            PacketSenderWithSharedPool,
            EcssTcInSharedPoolCacher,
            VerificationReporter,
            TestScheduler,
        >,
        sched_tc_pool: StaticMemoryPool,
    }

    impl Pus11HandlerWithStoreTester {
        pub fn new() -> Self {
            let test_scheduler = TestScheduler::default();
            let pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(
                alloc::vec![(16, 16), (8, 32), (4, 64)],
                false,
            );
            let sched_tc_pool = StaticMemoryPool::new(pool_cfg.clone());
            let (common, srv_handler) = PusServiceHandlerWithSharedStoreCommon::new(0);
            Self {
                common,
                handler: PusSchedServiceHandler::new(srv_handler, test_scheduler),
                sched_tc_pool,
            }
        }

        pub fn handle_one_tc(
            &mut self,
        ) -> Result<DirectPusPacketHandlerResult, PusPacketHandlingError> {
            let time_stamp = cds::CdsTime::new_with_u16_days(0, 0).to_vec().unwrap();
            self.handler
                .poll_and_handle_next_tc(|_| {}, &time_stamp, &mut self.sched_tc_pool)
        }
    }

    impl PusTestHarness for Pus11HandlerWithStoreTester {
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
                fn check_next_verification_tm(&self, subservice: u8, expected_request_id: RequestId);
            }
        }
    }

    #[derive(Default)]
    pub struct TestScheduler {
        reset_count: u32,
        enabled: bool,
        enabled_count: u32,
        disabled_count: u32,
        inserted_tcs: VecDeque<TcInfo>,
    }

    impl PusScheduler for TestScheduler {
        type TimeProvider = cds::CdsTime;

        fn reset(
            &mut self,
            _store: &mut (impl crate::pool::PoolProvider + ?Sized),
        ) -> Result<(), crate::pool::PoolError> {
            self.reset_count += 1;
            Ok(())
        }

        fn is_enabled(&self) -> bool {
            self.enabled
        }

        fn enable(&mut self) {
            self.enabled_count += 1;
            self.enabled = true;
        }

        fn disable(&mut self) {
            self.disabled_count += 1;
            self.enabled = false;
        }

        fn insert_unwrapped_and_stored_tc(
            &mut self,
            _time_stamp: spacepackets::time::UnixTime,
            info: crate::legacy::pus::scheduler::TcInfo,
        ) -> Result<(), crate::legacy::pus::scheduler::ScheduleError> {
            self.inserted_tcs.push_back(info);
            Ok(())
        }
    }

    fn generic_subservice_send(
        test_harness: &mut Pus11HandlerWithStoreTester,
        subservice: MessageSubtypeId,
    ) {
        let reply_header = SpHeader::new_for_unseg_tm(TEST_APID, u14::ZERO, 0);
        let tc_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(11, subservice as u8));
        let enable_scheduling =
            PusTcCreator::new(reply_header, tc_header, &[0; 7], CreatorConfig::default());
        let token = test_harness.start_verification(&enable_scheduling);
        test_harness.send_tc(&token, &enable_scheduling);

        let request_id = token.request_id();
        let time_stamp = cds::CdsTime::new_with_u16_days(0, 0).to_vec().unwrap();
        test_harness
            .handler
            .poll_and_handle_next_tc(|_| {}, &time_stamp, &mut test_harness.sched_tc_pool)
            .unwrap();
        test_harness.check_next_verification_tm(1, request_id);
        test_harness.check_next_verification_tm(3, request_id);
        test_harness.check_next_verification_tm(7, request_id);
    }

    #[test]
    fn test_scheduling_enabling_tc() {
        let mut test_harness = Pus11HandlerWithStoreTester::new();
        test_harness.handler.scheduler_mut().disable();
        assert!(!test_harness.handler.scheduler().is_enabled());
        generic_subservice_send(&mut test_harness, MessageSubtypeId::TcEnableScheduling);
        assert!(test_harness.handler.scheduler().is_enabled());
        assert_eq!(test_harness.handler.scheduler().enabled_count, 1);
    }

    #[test]
    fn test_scheduling_disabling_tc() {
        let mut test_harness = Pus11HandlerWithStoreTester::new();
        test_harness.handler.scheduler_mut().enable();
        assert!(test_harness.handler.scheduler().is_enabled());
        generic_subservice_send(&mut test_harness, MessageSubtypeId::TcDisableScheduling);
        assert!(!test_harness.handler.scheduler().is_enabled());
        assert_eq!(test_harness.handler.scheduler().disabled_count, 1);
    }

    #[test]
    fn test_reset_scheduler_tc() {
        let mut test_harness = Pus11HandlerWithStoreTester::new();
        generic_subservice_send(&mut test_harness, MessageSubtypeId::TcResetScheduling);
        assert_eq!(test_harness.handler.scheduler().reset_count, 1);
    }

    #[test]
    fn test_insert_activity_tc() {
        let mut test_harness = Pus11HandlerWithStoreTester::new();
        let mut reply_header = SpHeader::new_for_unseg_tc(TEST_APID, u14::ZERO, 0);
        let mut sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(17, 1));
        let ping_tc = PusTcCreator::new(reply_header, sec_header, &[], CreatorConfig::default());
        let req_id_ping_tc = scheduler::RequestId::from_tc(&ping_tc);
        let stamper = cds::CdsTime::now_with_u16_days().expect("time provider failed");
        let mut sched_app_data: [u8; 64] = [0; 64];
        let mut written_len = stamper.write_to_slice(&mut sched_app_data).unwrap();
        let ping_raw = ping_tc.to_vec().expect("generating raw tc failed");
        sched_app_data[written_len..written_len + ping_raw.len()].copy_from_slice(&ping_raw);
        written_len += ping_raw.len();
        reply_header = SpHeader::new_for_unseg_tc(TEST_APID, u14::ZERO, 0);
        sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(
            11,
            MessageSubtypeId::TcInsertActivity as u8,
        ));
        let enable_scheduling = PusTcCreator::new(
            reply_header,
            sec_header,
            &sched_app_data[..written_len],
            CreatorConfig::default(),
        );
        let token = test_harness.start_verification(&enable_scheduling);
        test_harness.send_tc(&token, &enable_scheduling);

        let request_id = token.request_id();
        test_harness.handle_one_tc().unwrap();
        test_harness.check_next_verification_tm(1, request_id);
        test_harness.check_next_verification_tm(3, request_id);
        test_harness.check_next_verification_tm(7, request_id);
        let tc_info = test_harness
            .handler
            .scheduler_mut()
            .inserted_tcs
            .pop_front()
            .unwrap();
        assert_eq!(tc_info.request_id(), req_id_ping_tc);
    }
}
