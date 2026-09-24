use crate::requests::GenericRequestRouter;
use crate::tmtc::sender::TmTcSender;
use log::warn;
use satrs::pool::PoolAddr;
use satrs::pus::verification::{
    self, FailParams, TcStateAccepted, TcStateStarted, VerificationReporter,
    VerificationReporterConfig, VerificationReportingProvider, VerificationToken,
};
use satrs::pus::{
    ActiveRequest, ActiveRequestStore, CacheAndReadRawEcssTc, EcssTcAndToken, EcssTcCacher,
    EcssTcReceiver, EcssTmSender, EcssTmtcError, GenericConversionError, GenericRoutingError,
    HandlingStatus, PusPacketHandlingError, PusReplyHandler, PusRequestRouter, PusServiceHelper,
    PusTcToRequestConverter, TcInMemory,
};
use satrs::queue::{GenericReceiveError, GenericSendError};
use satrs::request::{Apid, GenericMessage, MessageMetadata};
use satrs::spacepackets::ecss::tc::PusTcReader;
use satrs::spacepackets::ecss::{PusPacket, PusServiceId};
use satrs::tmtc::{PacketAsVec, PacketInPool};
use satrs::ComponentId;
use satrs_example::config::{tmtc_err, CustomPusServiceId};
use satrs_example::ids::generic_pus::PUS_ROUTING;
use satrs_example::TimestampHelper;
use std::fmt::Debug;
use std::sync::mpsc;

pub mod action;
pub mod event;
pub mod hk;
pub mod mode;
pub mod scheduler;
pub mod stack;
pub mod test;

pub fn create_verification_reporter(owner_id: ComponentId, apid: Apid) -> VerificationReporter {
    let verif_cfg = VerificationReporterConfig::new(apid, 1, 2, 8);
    // Every software component which needs to generate verification telemetry, gets a cloned
    // verification reporter.
    VerificationReporter::new(owner_id, &verif_cfg)
}

/*
/// Simple router structure which forwards PUS telecommands to dedicated handlers.
pub struct PusTcMpscRouter {
    pub test_tc_sender: mpsc::SyncSender<EcssTcAndToken>,
    pub event_tc_sender: mpsc::SyncSender<EcssTcAndToken>,
    pub sched_tc_sender: mpsc::SyncSender<EcssTcAndToken>,
    pub hk_tc_sender: mpsc::SyncSender<EcssTcAndToken>,
    #[allow(dead_code)]
    pub action_tc_sender: mpsc::SyncSender<EcssTcAndToken>,
    pub mode_tc_sender: mpsc::SyncSender<EcssTcAndToken>,
}

pub struct PusTcDistributor {
    #[allow(dead_code)]
    pub id: ComponentId,
    pub tm_sender: TmTcSender,
    pub verif_reporter: VerificationReporter,
    pub pus_router: PusTcMpscRouter,
    stamp_helper: TimestampHelper,
}

impl PusTcDistributor {
    pub fn new(tm_sender: TmTcSender, pus_router: PusTcMpscRouter) -> Self {
        Self {
            id: PUS_ROUTING.raw(),
            tm_sender,
            verif_reporter: create_verification_reporter(PUS_ROUTING.id(), PUS_ROUTING.apid),
            pus_router,
            stamp_helper: TimestampHelper::default(),
        }
    }

    pub fn handle_tc_packet_vec(
        &mut self,
        packet_as_vec: PacketAsVec,
    ) -> Result<HandlingStatus, GenericSendError> {
        self.handle_tc_generic(packet_as_vec.sender_id, None, &packet_as_vec.packet)
    }

    pub fn handle_tc_packet_in_store(
        &mut self,
        packet_in_pool: PacketInPool,
        pus_tc_copy: &[u8],
    ) -> Result<HandlingStatus, GenericSendError> {
        self.handle_tc_generic(
            packet_in_pool.sender_id,
            Some(packet_in_pool.store_addr),
            pus_tc_copy,
        )
    }

    pub fn handle_tc_generic(
        &mut self,
        sender_id: ComponentId,
        addr_opt: Option<PoolAddr>,
        raw_tc: &[u8],
    ) -> Result<HandlingStatus, GenericSendError> {
        let pus_tc_result = PusTcReader::new(raw_tc);
        if pus_tc_result.is_err() {
            log::warn!(
                "error creating PUS TC from raw data received from {}: {}",
                sender_id,
                pus_tc_result.unwrap_err()
            );
            log::warn!("raw data: {raw_tc:x?}");
            // TODO: Shouldn't this be an error?
            return Ok(HandlingStatus::HandledOne);
        }
        let pus_tc = pus_tc_result.unwrap();
        let init_token = self.verif_reporter.start_verification(&pus_tc);
        self.stamp_helper.update_from_now();
        let accepted_token = self
            .verif_reporter
            .acceptance_success(&self.tm_sender, init_token, self.stamp_helper.stamp())
            .expect("Acceptance success failure");
        let service = PusServiceId::try_from(pus_tc.service_type_id());
        let tc_in_memory: TcInMemory = if let Some(store_addr) = addr_opt {
            PacketInPool::new(sender_id, store_addr).into()
        } else {
            PacketAsVec::new(sender_id, Vec::from(raw_tc)).into()
        };
        match service {
            Ok(standard_service) => match standard_service {
                PusServiceId::Test => self.pus_router.test_tc_sender.send(EcssTcAndToken {
                    tc_in_memory,
                    token: Some(accepted_token.into()),
                })?,
                PusServiceId::Housekeeping => {
                    self.pus_router.hk_tc_sender.send(EcssTcAndToken {
                        tc_in_memory,
                        token: Some(accepted_token.into()),
                    })?
                }
                PusServiceId::Event => self.pus_router.event_tc_sender.send(EcssTcAndToken {
                    tc_in_memory,
                    token: Some(accepted_token.into()),
                })?,
                PusServiceId::Scheduling => {
                    self.pus_router.sched_tc_sender.send(EcssTcAndToken {
                        tc_in_memory,
                        token: Some(accepted_token.into()),
                    })?
                }
                _ => {
                    let result = self.verif_reporter.start_failure(
                        &self.tm_sender,
                        accepted_token,
                        FailParams::new(
                            self.stamp_helper.stamp(),
                            &tmtc_err::PUS_SERVICE_NOT_IMPLEMENTED,
                            &[standard_service as u8],
                        ),
                    );
                    if result.is_err() {
                        warn!("Sending verification failure failed");
                    }
                }
            },
            Err(e) => {
                if let Ok(custom_service) = CustomPusServiceId::try_from(e.number) {
                    match custom_service {
                        CustomPusServiceId::Mode => self
                            .pus_router
                            .mode_tc_sender
                            .send(EcssTcAndToken {
                                tc_in_memory,
                                token: Some(accepted_token.into()),
                            })
                            .map_err(|_| GenericSendError::RxDisconnected)?,
                        CustomPusServiceId::Health => {}
                    }
                } else {
                    self.verif_reporter
                        .start_failure(
                            &self.tm_sender,
                            accepted_token,
                            FailParams::new(
                                self.stamp_helper.stamp(),
                                &tmtc_err::INVALID_PUS_SUBSERVICE,
                                &[e.number],
                            ),
                        )
                        .expect("Start failure verification failed")
                }
            }
        }
        Ok(HandlingStatus::HandledOne)
    }
}
*/

pub trait TargetedPusService {
    const SERVICE_ID: u8;
    const SERVICE_STR: &'static str;

    fn poll_and_handle_next_tc_default_handler(&mut self, time_stamp: &[u8]) -> HandlingStatus {
        let result = self.poll_and_handle_next_tc(time_stamp);
        if let Err(e) = result {
            log::error!(
                "PUS service {}({})packet handling error: {:?}",
                Self::SERVICE_ID,
                Self::SERVICE_STR,
                e
            );
            // To avoid permanent loops on error cases.
            return HandlingStatus::Empty;
        }
        result.unwrap()
    }

    fn poll_and_handle_next_reply_default_handler(&mut self, time_stamp: &[u8]) -> HandlingStatus {
        // This only fails if all senders disconnected. Treat it like an empty queue.
        self.poll_and_handle_next_reply(time_stamp)
            .unwrap_or_else(|e| {
                warn!(
                    "PUS servce {}({}): Handling reply failed with error {:?}",
                    Self::SERVICE_ID,
                    Self::SERVICE_STR,
                    e
                );
                HandlingStatus::Empty
            })
    }

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

/// Generic trait for services which handle packets directly. Kept minimal right now because
/// of the difficulty to allow flexible user code for these services..
pub trait DirectPusService {
    const SERVICE_ID: u8;
    const SERVICE_STR: &'static str;

    fn poll_and_handle_next_tc(&mut self, timestamp: &[u8]) -> HandlingStatus;
}

/// This is a generic handler class for all PUS services where a PUS telecommand is converted
/// to a targeted request.
///
/// The generic steps for this process are the following
///
///  1. Poll for TC packets
///  2. Convert the raw packets to a [PusTcReader].
///  3. Convert the PUS TC to a typed request using the [PusTcToRequestConverter].
///  4. Route the requests using the [GenericRequestRouter].
///  5. Add the request to the active request map using the [ActiveRequestMapProvider] abstraction.
///  6. Check for replies which complete the forwarded request. The handler takes care of
///     the verification process.
///  7. Check for timeouts of active requests. Generally, the timeout on the service level should
///     be highest expected timeout for the given target.
///
/// The handler exposes the following API:
///
///  1. [Self::poll_and_handle_next_tc] which tries to poll and handle one TC packet, covering
///     steps 1-5.
///  2. [Self::poll_and_check_next_reply] which tries to poll and handle one reply, covering step 6.
///  3. [Self::check_for_request_timeouts] which checks for request timeouts, covering step 7.
pub struct PusTargetedRequestService<
    TcReceiver: EcssTcReceiver,
    VerificationReporter: VerificationReportingProvider,
    RequestConverter: PusTcToRequestConverter<ActiveRequestInfo, RequestType, Error = GenericConversionError>,
    ReplyHandler: PusReplyHandler<ActiveRequestInfo, ReplyType, Error = EcssTmtcError>,
    ActiveRequestMapInstance: ActiveRequestStore<ActiveRequestInfo>,
    ActiveRequestInfo: ActiveRequest,
    RequestType,
    ReplyType,
> {
    pub service_helper:
        PusServiceHelper<TcReceiver, TmTcSender, EcssTcCacher, VerificationReporter>,
    pub request_router: GenericRequestRouter,
    pub request_converter: RequestConverter,
    pub active_request_map: ActiveRequestMapInstance,
    pub reply_handler: ReplyHandler,
    pub reply_receiver: mpsc::Receiver<GenericMessage<ReplyType>>,
    phantom: std::marker::PhantomData<(RequestType, ActiveRequestInfo, ReplyType)>,
}

impl<
        TcReceiver: EcssTcReceiver,
        VerificationReporter: VerificationReportingProvider,
        RequestConverter: PusTcToRequestConverter<ActiveRequestInfo, RequestType, Error = GenericConversionError>,
        ReplyHandler: PusReplyHandler<ActiveRequestInfo, ReplyType, Error = EcssTmtcError>,
        ActiveRequestMapInstance: ActiveRequestStore<ActiveRequestInfo>,
        ActiveRequestInfo: ActiveRequest,
        RequestType,
        ReplyType,
    >
    PusTargetedRequestService<
        TcReceiver,
        VerificationReporter,
        RequestConverter,
        ReplyHandler,
        ActiveRequestMapInstance,
        ActiveRequestInfo,
        RequestType,
        ReplyType,
    >
where
    GenericRequestRouter: PusRequestRouter<RequestType, Error = GenericRoutingError>,
{
    pub fn new(
        service_helper: PusServiceHelper<
            TcReceiver,
            TmTcSender,
            EcssTcCacher,
            VerificationReporter,
        >,
        request_converter: RequestConverter,
        active_request_map: ActiveRequestMapInstance,
        reply_hook: ReplyHandler,
        request_router: GenericRequestRouter,
        reply_receiver: mpsc::Receiver<GenericMessage<ReplyType>>,
    ) -> Self {
        Self {
            service_helper,
            request_converter,
            active_request_map,
            reply_handler: reply_hook,
            request_router,
            reply_receiver,
            phantom: std::marker::PhantomData,
        }
    }

    pub fn poll_and_handle_next_tc(
        &mut self,
        time_stamp: &[u8],
    ) -> Result<HandlingStatus, PusPacketHandlingError> {
        let possible_packet = self.service_helper.retrieve_and_accept_next_packet()?;
        if possible_packet.is_none() {
            return Ok(HandlingStatus::Empty);
        }
        let ecss_tc_and_token = possible_packet.unwrap();
        self.service_helper
            .tc_in_mem_converter_mut()
            .cache(&ecss_tc_and_token.tc_in_memory)?;
        let tc = self.service_helper.tc_in_mem_converter().convert()?;
        let (mut request_info, request) = match self.request_converter.convert(
            ecss_tc_and_token.token,
            &tc,
            self.service_helper.tm_sender(),
            &self.service_helper.common.verif_reporter,
            time_stamp,
        ) {
            Ok((info, req)) => (info, req),
            Err(e) => {
                self.handle_conversion_to_request_error(&e, ecss_tc_and_token.token, time_stamp);
                return Err(e.into());
            }
        };
        let accepted_token: VerificationToken<TcStateAccepted> = request_info
            .token()
            .try_into()
            .expect("token not in expected accepted state");
        let verif_request_id = verification::RequestId::new(&tc).raw();
        match self.request_router.route(
            MessageMetadata::new(verif_request_id, self.service_helper.id()),
            request_info.target_id(),
            request,
        ) {
            Ok(()) => {
                let started_token = self
                    .service_helper
                    .verif_reporter()
                    .start_success(
                        &self.service_helper.common.tm_sender,
                        accepted_token,
                        time_stamp,
                    )
                    .expect("Start success failure");
                request_info.set_token(started_token.into());
                self.active_request_map
                    .insert(&verif_request_id, request_info);
            }
            Err(e) => {
                self.request_router.handle_error_generic(
                    &request_info,
                    &tc,
                    e.clone(),
                    self.service_helper.tm_sender(),
                    self.service_helper.verif_reporter(),
                    time_stamp,
                );
                return Err(e.into());
            }
        }
        Ok(HandlingStatus::HandledOne)
    }

    fn handle_conversion_to_request_error(
        &mut self,
        error: &GenericConversionError,
        token: VerificationToken<TcStateAccepted>,
        time_stamp: &[u8],
    ) {
        match error {
            GenericConversionError::WrongService(service) => {
                let service_slice: [u8; 1] = [*service];
                self.service_helper
                    .verif_reporter()
                    .completion_failure(
                        self.service_helper.tm_sender(),
                        token,
                        FailParams::new(time_stamp, &tmtc_err::INVALID_PUS_SERVICE, &service_slice),
                    )
                    .expect("Sending completion failure failed");
            }
            GenericConversionError::InvalidSubservice(subservice) => {
                let subservice_slice: [u8; 1] = [*subservice];
                self.service_helper
                    .verif_reporter()
                    .completion_failure(
                        self.service_helper.tm_sender(),
                        token,
                        FailParams::new(
                            time_stamp,
                            &tmtc_err::INVALID_PUS_SUBSERVICE,
                            &subservice_slice,
                        ),
                    )
                    .expect("Sending completion failure failed");
            }
            GenericConversionError::NotEnoughAppData { expected, found } => {
                let mut context_info = (*found as u32).to_be_bytes().to_vec();
                context_info.extend_from_slice(&(*expected as u32).to_be_bytes());
                self.service_helper
                    .verif_reporter()
                    .completion_failure(
                        self.service_helper.tm_sender(),
                        token,
                        FailParams::new(time_stamp, &tmtc_err::NOT_ENOUGH_APP_DATA, &context_info),
                    )
                    .expect("Sending completion failure failed");
            }
            // Do nothing.. this is service-level and can not be handled generically here.
            GenericConversionError::InvalidAppData(_) => (),
        }
    }

    pub fn poll_and_handle_next_reply(
        &mut self,
        time_stamp: &[u8],
    ) -> Result<HandlingStatus, EcssTmtcError> {
        match self.reply_receiver.try_recv() {
            Ok(reply) => {
                self.handle_reply(&reply, time_stamp)?;
                Ok(HandlingStatus::HandledOne)
            }
            Err(e) => match e {
                mpsc::TryRecvError::Empty => Ok(HandlingStatus::Empty),
                mpsc::TryRecvError::Disconnected => Err(EcssTmtcError::Receive(
                    GenericReceiveError::TxDisconnected(None),
                )),
            },
        }
    }

    pub fn handle_reply(
        &mut self,
        reply: &GenericMessage<ReplyType>,
        time_stamp: &[u8],
    ) -> Result<(), EcssTmtcError> {
        let active_req_opt = self.active_request_map.get(reply.request_id());
        if active_req_opt.is_none() {
            self.reply_handler
                .handle_unrequested_reply(reply, &self.service_helper.common.tm_sender)?;
            return Ok(());
        }
        let active_request = active_req_opt.unwrap();
        let result = self.reply_handler.handle_reply(
            reply,
            active_request,
            &self.service_helper.common.tm_sender,
            &self.service_helper.common.verif_reporter,
            time_stamp,
        );
        if result.is_err() || (result.is_ok() && *result.as_ref().unwrap()) {
            self.active_request_map.remove(reply.request_id());
        }
        result.map(|_| ())
    }

    pub fn check_for_request_timeouts(&mut self) {
        let mut requests_to_delete = Vec::new();
        self.active_request_map
            .for_each(|request_id, request_info| {
                if request_info.has_timed_out() {
                    requests_to_delete.push(*request_id);
                }
            });
        if !requests_to_delete.is_empty() {
            for request_id in requests_to_delete {
                self.active_request_map.remove(request_id);
            }
        }
    }
}

/// Generic timeout handling: Handle the verification failure with a dedicated return code
/// and also log the error.
pub fn generic_pus_request_timeout_handler(
    sender: &(impl EcssTmSender + ?Sized),
    active_request: &(impl ActiveRequest + Debug),
    verification_handler: &impl VerificationReportingProvider,
    time_stamp: &[u8],
    service_str: &'static str,
) -> Result<(), EcssTmtcError> {
    log::warn!("timeout for active request {active_request:?} on {service_str} service");
    let started_token: VerificationToken<TcStateStarted> = active_request
        .token()
        .try_into()
        .expect("token not in expected started state");
    verification_handler.completion_failure(
        sender,
        started_token,
        FailParams::new(time_stamp, &tmtc_err::REQUEST_TIMEOUT, &[]),
    )?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::time::Duration;

    use arbitrary_int::{u11, u21};
    use satrs::pus::test_util::TEST_COMPONENT_ID_0;
    use satrs::pus::{MpscTmAsVecSender, PusTmVariant};
    use satrs::request::RequestId;
    use satrs::spacepackets::ecss::{CreatorConfig, MessageTypeId};
    use satrs::{
        pus::{
            verification::test_util::TestVerificationReporter, ActivePusRequestStd,
            ActiveRequestStore, MpscTcReceiver,
        },
        request::UniqueApidTargetId,
        spacepackets::{
            ecss::{
                tc::{PusTcCreator, PusTcSecondaryHeader},
                WritablePusPacket,
            },
            SpHeader,
        },
    };

    use crate::requests::CompositeRequest;

    use super::*;

    // Testbench dedicated to the testing of [PusReplyHandler]s
    pub struct ReplyHandlerTestbench<
        ReplyHandler: PusReplyHandler<ActiveRequestInfo, Reply, Error = EcssTmtcError>,
        ActiveRequestInfo: ActiveRequest,
        Reply,
    > {
        pub id: ComponentId,
        pub verif_reporter: TestVerificationReporter,
        pub reply_handler: ReplyHandler,
        pub tm_receiver: mpsc::Receiver<PacketAsVec>,
        pub default_timeout: Duration,
        tm_sender: MpscTmAsVecSender,
        phantom: std::marker::PhantomData<(ActiveRequestInfo, Reply)>,
    }

    impl<
            ReplyHandler: PusReplyHandler<ActiveRequestInfo, Reply, Error = EcssTmtcError>,
            ActiveRequestInfo: ActiveRequest,
            Reply,
        > ReplyHandlerTestbench<ReplyHandler, ActiveRequestInfo, Reply>
    {
        pub fn new(owner_id: ComponentId, reply_handler: ReplyHandler) -> Self {
            let test_verif_reporter = TestVerificationReporter::new(owner_id);
            let (tm_sender, tm_receiver) = mpsc::channel();
            Self {
                id: TEST_COMPONENT_ID_0.raw(),
                verif_reporter: test_verif_reporter,
                reply_handler,
                default_timeout: Duration::from_secs(30),
                tm_sender,
                tm_receiver,
                phantom: std::marker::PhantomData,
            }
        }

        pub fn add_tc(
            &mut self,
            apid: u11,
            apid_target: u21,
            time_stamp: &[u8],
        ) -> (verification::RequestId, ActivePusRequestStd) {
            let sp_header = SpHeader::new_from_apid(apid);
            let sec_header_dummy = PusTcSecondaryHeader::new_simple(MessageTypeId::new(0, 0));
            let init = self.verif_reporter.start_verification(&PusTcCreator::new(
                sp_header,
                sec_header_dummy,
                &[],
                CreatorConfig::default(),
            ));
            let accepted = self
                .verif_reporter
                .acceptance_success(&self.tm_sender, init, time_stamp)
                .expect("acceptance failed");
            let started = self
                .verif_reporter
                .start_success(&self.tm_sender, accepted, time_stamp)
                .expect("start failed");
            (
                started.request_id(),
                ActivePusRequestStd::new(
                    UniqueApidTargetId::new(apid, apid_target).raw(),
                    started,
                    self.default_timeout,
                ),
            )
        }

        pub fn handle_reply(
            &mut self,
            reply: &GenericMessage<Reply>,
            active_request: &ActiveRequestInfo,
            time_stamp: &[u8],
        ) -> Result<bool, ReplyHandler::Error> {
            self.reply_handler.handle_reply(
                reply,
                active_request,
                &self.tm_sender,
                &self.verif_reporter,
                time_stamp,
            )
        }

        pub fn handle_unrequested_reply(
            &mut self,
            reply: &GenericMessage<Reply>,
        ) -> Result<(), ReplyHandler::Error> {
            self.reply_handler
                .handle_unrequested_reply(reply, &self.tm_sender)
        }
        pub fn handle_request_timeout(
            &mut self,
            active_request_info: &ActiveRequestInfo,
            time_stamp: &[u8],
        ) -> Result<(), ReplyHandler::Error> {
            self.reply_handler.handle_request_timeout(
                active_request_info,
                &self.tm_sender,
                &self.verif_reporter,
                time_stamp,
            )
        }
    }

    #[derive(Default)]
    pub struct DummySender {}

    /// Dummy sender component which does nothing on the [Self::send_tm] call.
    ///
    /// Useful for unit tests.
    impl EcssTmSender for DummySender {
        fn send_tm(&self, _source_id: ComponentId, _tm: PusTmVariant) -> Result<(), EcssTmtcError> {
            Ok(())
        }
    }

    // Testbench dedicated to the testing of [PusTcToRequestConverter]s
    pub struct PusConverterTestbench<
        Converter: PusTcToRequestConverter<ActiveRequestInfo, Request, Error = GenericConversionError>,
        ActiveRequestInfo: ActiveRequest,
        Request,
    > {
        pub id: ComponentId,
        pub verif_reporter: TestVerificationReporter,
        pub converter: Converter,
        dummy_sender: DummySender,
        current_request_id: Option<verification::RequestId>,
        current_packet: Option<Vec<u8>>,
        phantom: std::marker::PhantomData<(ActiveRequestInfo, Request)>,
    }

    impl<
            Converter: PusTcToRequestConverter<ActiveRequestInfo, Request, Error = GenericConversionError>,
            ActiveRequestInfo: ActiveRequest,
            Request,
        > PusConverterTestbench<Converter, ActiveRequestInfo, Request>
    {
        pub fn new(owner_id: ComponentId, converter: Converter) -> Self {
            let test_verif_reporter = TestVerificationReporter::new(owner_id);
            Self {
                id: owner_id,
                verif_reporter: test_verif_reporter,
                converter,
                dummy_sender: DummySender::default(),
                current_request_id: None,
                current_packet: None,
                phantom: std::marker::PhantomData,
            }
        }

        pub fn add_tc(&mut self, tc: &PusTcCreator) -> VerificationToken<TcStateAccepted> {
            let token = self.verif_reporter.start_verification(tc);
            self.current_request_id = Some(verification::RequestId::new(tc));
            self.current_packet = Some(tc.to_vec().unwrap());
            self.verif_reporter
                .acceptance_success(&self.dummy_sender, token, &[])
                .expect("acceptance failed")
        }

        pub fn request_id(&self) -> Option<verification::RequestId> {
            self.current_request_id
        }

        pub fn convert(
            &mut self,
            token: VerificationToken<TcStateAccepted>,
            time_stamp: &[u8],
            expected_apid: u11,
            expected_apid_target: u21,
        ) -> Result<(ActiveRequestInfo, Request), Converter::Error> {
            if self.current_packet.is_none() {
                return Err(GenericConversionError::InvalidAppData(
                    "call add_tc first".to_string(),
                ));
            }
            let current_packet = self.current_packet.take().unwrap();
            let tc_reader = PusTcReader::new(&current_packet).unwrap();
            let (active_info, request) = self.converter.convert(
                token,
                &tc_reader,
                &self.dummy_sender,
                &self.verif_reporter,
                time_stamp,
            )?;
            assert_eq!(
                active_info.token().request_id(),
                self.request_id().expect("no request id is set")
            );
            assert_eq!(
                active_info.target_id(),
                UniqueApidTargetId::new(expected_apid, expected_apid_target).raw()
            );
            Ok((active_info, request))
        }
    }

    pub struct TargetedPusRequestTestbench<
        RequestConverter: PusTcToRequestConverter<ActiveRequestInfo, RequestType, Error = GenericConversionError>,
        ReplyHandler: PusReplyHandler<ActiveRequestInfo, ReplyType, Error = EcssTmtcError>,
        ActiveRequestMapInstance: ActiveRequestStore<ActiveRequestInfo>,
        ActiveRequestInfo: ActiveRequest,
        RequestType,
        ReplyType,
    > {
        pub service: PusTargetedRequestService<
            MpscTcReceiver,
            TestVerificationReporter,
            RequestConverter,
            ReplyHandler,
            ActiveRequestMapInstance,
            ActiveRequestInfo,
            RequestType,
            ReplyType,
        >,
        pub request_id: Option<RequestId>,
        pub tm_funnel_rx: mpsc::Receiver<PacketAsVec>,
        pub pus_packet_tx: mpsc::Sender<EcssTcAndToken>,
        pub reply_tx: mpsc::Sender<GenericMessage<ReplyType>>,
        pub request_rx: mpsc::Receiver<GenericMessage<CompositeRequest>>,
    }
}
