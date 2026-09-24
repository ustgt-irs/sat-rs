use crate::pus::create_verification_reporter;
use crate::tmtc::sender::TmTcSender;
use log::info;
use satrs::event_man_legacy::{EventMessage, EventMessageU32};
use satrs::pus::test::PusService17TestHandler;
use satrs::pus::verification::{FailParams, VerificationReporter, VerificationReportingProvider};
use satrs::pus::PartialPusHandlingError;
use satrs::pus::{
    CacheAndReadRawEcssTc, DirectPusPacketHandlerResult, EcssTcAndToken, EcssTcCacher,
    MpscTcReceiver, PusServiceHelper,
};
use satrs::spacepackets::ecss::tc::PusTcReader;
use satrs::spacepackets::ecss::{PusPacket, PusServiceId};
use satrs_example::config::{tmtc_err, TEST_EVENT};
use satrs_example::ids::generic_pus::PUS_TEST;
use std::sync::mpsc;

use super::{DirectPusService, HandlingStatus};

pub fn create_test_service(
    tm_sender: TmTcSender,
    tc_in_mem_converter: EcssTcCacher,
    event_sender: mpsc::SyncSender<EventMessageU32>,
    pus_test_rx: mpsc::Receiver<EcssTcAndToken>,
) -> TestCustomServiceWrapper {
    let pus17_handler = PusService17TestHandler::new(PusServiceHelper::new(
        PUS_TEST.id(),
        pus_test_rx,
        tm_sender,
        create_verification_reporter(PUS_TEST.id(), PUS_TEST.apid),
        tc_in_mem_converter,
    ));
    TestCustomServiceWrapper {
        handler: pus17_handler,
        event_tx: event_sender,
    }
}

pub struct TestCustomServiceWrapper {
    pub handler:
        PusService17TestHandler<MpscTcReceiver, TmTcSender, EcssTcCacher, VerificationReporter>,
    pub event_tx: mpsc::SyncSender<EventMessageU32>,
}

impl DirectPusService for TestCustomServiceWrapper {
    const SERVICE_ID: u8 = PusServiceId::Test as u8;

    const SERVICE_STR: &'static str = "test";

    fn poll_and_handle_next_tc(&mut self, timestamp: &[u8]) -> HandlingStatus {
        let error_handler = |partial_error: &PartialPusHandlingError| {
            log::warn!(
                "PUS {}({}) partial error: {:?}",
                Self::SERVICE_ID,
                Self::SERVICE_STR,
                partial_error
            );
        };
        let res = self
            .handler
            .poll_and_handle_next_tc(error_handler, timestamp);
        if let Err(e) = res {
            log::warn!(
                "PUS {}({}) error: {:?}",
                Self::SERVICE_ID,
                Self::SERVICE_STR,
                e
            );
            // To avoid permanent loops on continuous errors.
            return HandlingStatus::Empty;
        }
        match res.unwrap() {
            DirectPusPacketHandlerResult::Handled(handling_status) => {
                if handling_status == HandlingStatus::HandledOne {
                    info!("Received PUS ping command TC[17,1]");
                    info!("Sent ping reply PUS TM[17,2]");
                }
                return handling_status;
            }
            DirectPusPacketHandlerResult::SubserviceNotImplemented(subservice, _) => {
                log::warn!(
                    "PUS {}({}) subservice {} not implemented",
                    Self::SERVICE_ID,
                    Self::SERVICE_STR,
                    subservice
                );
            }
            DirectPusPacketHandlerResult::CustomSubservice(subservice, token) => {
                let tc = PusTcReader::new(
                    self.handler
                        .service_helper
                        .tc_in_mem_converter
                        .tc_slice_raw(),
                )
                .unwrap();
                if subservice == 128 {
                    info!("generating test event");
                    self.event_tx
                        .send(EventMessage::new(PUS_TEST.id(), TEST_EVENT.into()))
                        .expect("Sending test event failed");
                    match self.handler.service_helper.verif_reporter().start_success(
                        self.handler.service_helper.tm_sender(),
                        token,
                        timestamp,
                    ) {
                        Ok(started_token) => {
                            if let Err(e) = self
                                .handler
                                .service_helper
                                .verif_reporter()
                                .completion_success(
                                    self.handler.service_helper.tm_sender(),
                                    started_token,
                                    timestamp,
                                )
                            {
                                error_handler(&PartialPusHandlingError::Verification(e));
                            }
                        }
                        Err(e) => {
                            error_handler(&PartialPusHandlingError::Verification(e));
                        }
                    }
                } else {
                    let fail_data = [tc.message_subtype_id()];
                    self.handler
                        .service_helper
                        .verif_reporter()
                        .start_failure(
                            self.handler.service_helper.tm_sender(),
                            token,
                            FailParams::new(
                                timestamp,
                                &tmtc_err::INVALID_PUS_SUBSERVICE,
                                &fail_data,
                            ),
                        )
                        .expect("Sending start failure verification failed");
                }
            }
        }
        HandlingStatus::HandledOne
    }
}
