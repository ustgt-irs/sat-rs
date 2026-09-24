use std::sync::mpsc;

use crate::pus::create_verification_reporter;
use crate::tmtc::sender::TmTcSender;
use satrs::pus::event_man::EventRequestWithToken;
use satrs::pus::event_srv::PusEventServiceHandler;
use satrs::pus::verification::VerificationReporter;
use satrs::pus::{
    DirectPusPacketHandlerResult, EcssTcAndToken, EcssTcCacher, MpscTcReceiver,
    PartialPusHandlingError, PusServiceHelper,
};
use satrs::spacepackets::ecss::PusServiceId;
use satrs_example::ids::generic_pus::PUS_EVENT_MANAGEMENT;

use super::{DirectPusService, HandlingStatus};

pub fn create_event_service(
    tm_sender: TmTcSender,
    tm_in_pool_converter: EcssTcCacher,
    pus_event_rx: mpsc::Receiver<EcssTcAndToken>,
    event_request_tx: mpsc::Sender<EventRequestWithToken>,
) -> EventServiceWrapper {
    let pus_5_handler = PusEventServiceHandler::new(
        PusServiceHelper::new(
            PUS_EVENT_MANAGEMENT.id(),
            pus_event_rx,
            tm_sender,
            create_verification_reporter(PUS_EVENT_MANAGEMENT.id(), PUS_EVENT_MANAGEMENT.apid),
            tm_in_pool_converter,
        ),
        event_request_tx,
    );
    EventServiceWrapper {
        handler: pus_5_handler,
    }
}

pub struct EventServiceWrapper {
    pub handler:
        PusEventServiceHandler<MpscTcReceiver, TmTcSender, EcssTcCacher, VerificationReporter>,
}

impl DirectPusService for EventServiceWrapper {
    const SERVICE_ID: u8 = PusServiceId::Event as u8;

    const SERVICE_STR: &'static str = "events";

    fn poll_and_handle_next_tc(&mut self, time_stamp: &[u8]) -> HandlingStatus {
        let error_handler = |partial_error: &PartialPusHandlingError| {
            log::warn!(
                "PUS {}({}) partial error: {:?}",
                Self::SERVICE_ID,
                Self::SERVICE_STR,
                partial_error
            );
        };
        let result = self
            .handler
            .poll_and_handle_next_tc(error_handler, time_stamp);
        if let Err(e) = result {
            log::warn!(
                "PUS {}({}) error: {:?}",
                Self::SERVICE_ID,
                Self::SERVICE_STR,
                e
            );
            // To avoid permanent loops on continuous errors.
            return HandlingStatus::Empty;
        }
        match result.unwrap() {
            DirectPusPacketHandlerResult::Handled(handling_status) => return handling_status,
            DirectPusPacketHandlerResult::CustomSubservice(subservice, _) => {
                log::warn!(
                    "PUS {}({}) subservice {} not implemented",
                    Self::SERVICE_ID,
                    Self::SERVICE_STR,
                    subservice
                );
            }
            DirectPusPacketHandlerResult::SubserviceNotImplemented(subservice, _) => {
                log::warn!(
                    "PUS {}({}) subservice {} not implemented",
                    Self::SERVICE_ID,
                    Self::SERVICE_STR,
                    subservice
                );
            }
        }
        HandlingStatus::HandledOne
    }
}
