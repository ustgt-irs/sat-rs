use arbitrary_int::traits::Integer as _;
use arbitrary_int::u11;
use satrs::event_man_legacy::{EventMessageU32, EventRoutingError};
use satrs::pus::event::EventTmHook;
use satrs::pus::verification::VerificationReporter;
use satrs::pus::EcssTmSender;
use satrs::request::UniqueApidTargetId;
use satrs::{
    event_man_legacy::{EventManagerWithBoundedMpsc, EventSendProvider, EventU32SenderMpscBounded},
    pus::{
        event_man::{
            DefaultPusEventU32TmCreator, EventReporter, EventRequest, EventRequestWithToken,
        },
        verification::{TcStateStarted, VerificationReportingProvider, VerificationToken},
    },
    spacepackets::time::cds::CdsTime,
};
use satrs_example::ids::generic_pus::PUS_EVENT_MANAGEMENT;

use crate::update_time;

// This helper sets the APID of the event sender for the PUS telemetry.
#[derive(Default)]
pub struct EventApidSetter {
    pub next_apid: u11,
}

impl EventTmHook for EventApidSetter {
    fn modify_tm(&self, tm: &mut satrs::spacepackets::ecss::tm::PusTmCreator) {
        tm.set_apid(self.next_apid);
    }
}

/*
/// The PUS event handler subscribes for all events and converts them into ECSS PUS 5 event
/// packets. It also handles the verification completion of PUS event service requests.
pub struct PusEventHandler<TmSender: EcssTmSender> {
    event_request_rx: mpsc::Receiver<EventRequestWithToken>,
    pus_event_tm_creator: DefaultPusEventU32TmCreator<EventApidSetter>,
    pus_event_man_rx: mpsc::Receiver<EventMessageU32>,
    tm_sender: TmSender,
    time_provider: CdsTime,
    timestamp: [u8; 7],
    small_data_buf: [u8; 64],
    verif_handler: VerificationReporter,
}

impl<TmSender: EcssTmSender> PusEventHandler<TmSender> {
    pub fn new(
        tm_sender: TmSender,
        verif_handler: VerificationReporter,
        event_manager: &mut EventManagerWithBoundedMpsc,
        event_request_rx: mpsc::Receiver<EventRequestWithToken>,
    ) -> Self {
        let event_queue_cap = 30;
        let (pus_event_man_tx, pus_event_man_rx) = mpsc::sync_channel(event_queue_cap);

        // All events sent to the manager are routed to the PUS event manager, which generates PUS event
        // telemetry for each event.
        let event_reporter = EventReporter::new_with_hook(
            PUS_EVENT_MANAGEMENT.raw(),
            u11::ZERO,
            0,
            128,
            EventApidSetter::default(),
        );
        let pus_event_dispatcher =
            DefaultPusEventU32TmCreator::new_with_default_backend(event_reporter);
        let pus_event_man_send_provider = EventU32SenderMpscBounded::new(
            PUS_EVENT_MANAGEMENT.raw(),
            pus_event_man_tx,
            event_queue_cap,
        );

        event_manager.subscribe_all(pus_event_man_send_provider.target_id());
        event_manager.add_sender(pus_event_man_send_provider);

        Self {
            event_request_rx,
            pus_event_tm_creator: pus_event_dispatcher,
            pus_event_man_rx,
            time_provider: CdsTime::new_with_u16_days(0, 0),
            timestamp: [0; 7],
            small_data_buf: [0; 64],
            verif_handler,
            tm_sender,
        }
    }

    pub fn handle_event_requests(&mut self) {
        let report_completion = |event_req: EventRequestWithToken, timestamp: &[u8]| {
            let started_token: VerificationToken<TcStateStarted> = event_req
                .token
                .try_into()
                .expect("expected start verification token");
            self.verif_handler
                .completion_success(&self.tm_sender, started_token, timestamp)
                .expect("Sending completion success failed");
        };
        loop {
            // handle event requests
            match self.event_request_rx.try_recv() {
                Ok(event_req) => match event_req.request {
                    EventRequest::Enable(event) => {
                        self.pus_event_tm_creator
                            .enable_tm_for_event(&event)
                            .expect("Enabling TM failed");
                        update_time(&mut self.time_provider, &mut self.timestamp);
                        report_completion(event_req, &self.timestamp);
                    }
                    EventRequest::Disable(event) => {
                        self.pus_event_tm_creator
                            .disable_tm_for_event(&event)
                            .expect("Disabling TM failed");
                        update_time(&mut self.time_provider, &mut self.timestamp);
                        report_completion(event_req, &self.timestamp);
                    }
                },
                Err(e) => match e {
                    mpsc::TryRecvError::Empty => break,
                    mpsc::TryRecvError::Disconnected => {
                        log::warn!("all event request senders have disconnected");
                        break;
                    }
                },
            }
        }
    }

    pub fn generate_pus_event_tm(&mut self) {
        loop {
            // Perform the generation of PUS event packets
            match self.pus_event_man_rx.try_recv() {
                Ok(event_msg) => {
                    // We use the TM modification hook to set the sender APID for each event.
                    self.pus_event_tm_creator.reporter.tm_hook.next_apid =
                        UniqueApidTargetId::from(event_msg.sender_id()).apid;
                    update_time(&mut self.time_provider, &mut self.timestamp);
                    let generation_result = self
                        .pus_event_tm_creator
                        .generate_pus_event_tm_generic_with_generic_params(
                            &self.tm_sender,
                            &self.timestamp,
                            event_msg.event(),
                            &mut self.small_data_buf,
                            event_msg.params(),
                        )
                        .expect("Sending TM as event failed");
                    if !generation_result.params_were_propagated {
                        log::warn!(
                            "Event TM parameters were not propagated: {:?}",
                            event_msg.params()
                        );
                    }
                }
                Err(e) => match e {
                    mpsc::TryRecvError::Empty => break,
                    mpsc::TryRecvError::Disconnected => {
                        log::warn!("All event senders have disconnected");
                        break;
                    }
                },
            }
        }
    }
}

pub struct EventHandler<TmSender: EcssTmSender> {
    pub pus_event_handler: PusEventHandler<TmSender>,
    event_manager: EventManagerWithBoundedMpsc,
}

impl<TmSender: EcssTmSender> EventHandler<TmSender> {
    pub fn new(
        tm_sender: TmSender,
        event_rx: mpsc::Receiver<EventMessageU32>,
        event_request_rx: mpsc::Receiver<EventRequestWithToken>,
    ) -> Self {
        let mut event_manager = EventManagerWithBoundedMpsc::new(event_rx);
        let pus_event_handler = PusEventHandler::new(
            tm_sender,
            create_verification_reporter(PUS_EVENT_MANAGEMENT.id(), PUS_EVENT_MANAGEMENT.apid),
            &mut event_manager,
            event_request_rx,
        );

        Self {
            pus_event_handler,
            event_manager,
        }
    }

    #[allow(dead_code)]
    pub fn event_manager(&mut self) -> &mut EventManagerWithBoundedMpsc {
        &mut self.event_manager
    }

    pub fn periodic_operation(&mut self) {
        self.pus_event_handler.handle_event_requests();
        self.try_event_routing();
        self.pus_event_handler.generate_pus_event_tm();
    }

    pub fn try_event_routing(&mut self) {
        let error_handler = |event_msg: &EventMessageU32, error: EventRoutingError| {
            self.routing_error_handler(event_msg, error)
        };
        // Perform the event routing.
        self.event_manager.try_event_handling(error_handler);
    }

    pub fn routing_error_handler(&self, event_msg: &EventMessageU32, error: EventRoutingError) {
        log::warn!("event routing error for event {event_msg:?}: {error:?}");
    }
}

#[cfg(test)]
mod tests {
    use arbitrary_int::u21;
    use satrs::{
        events_legacy::EventU32,
        pus::verification::VerificationReporterConfig,
        spacepackets::ecss::{tm::PusTmReader, PusPacket},
        tmtc::PacketAsVec,
    };

    use super::*;

    const TEST_CREATOR_ID: UniqueApidTargetId = UniqueApidTargetId::new(u11::new(1), u21::new(2));
    const TEST_EVENT: EventU32 = EventU32::new(satrs::events_legacy::Severity::Info, 1, 1);

    pub struct EventManagementTestbench {
        pub event_tx: mpsc::SyncSender<EventMessageU32>,
        pub event_manager: EventManagerWithBoundedMpsc,
        pub tm_receiver: mpsc::Receiver<PacketAsVec>,
        pub pus_event_handler: PusEventHandler<mpsc::Sender<PacketAsVec>>,
    }

    impl EventManagementTestbench {
        pub fn new() -> Self {
            let (event_tx, event_rx) = mpsc::sync_channel(10);
            let (_event_req_tx, event_req_rx) = mpsc::sync_channel(10);
            let (tm_sender, tm_receiver) = mpsc::channel();
            let verif_reporter_cfg = VerificationReporterConfig::new(u11::new(0x05), 2, 2, 128);
            let verif_reporter =
                VerificationReporter::new(PUS_EVENT_MANAGEMENT.id(), &verif_reporter_cfg);
            let mut event_manager = EventManagerWithBoundedMpsc::new(event_rx);
            let pus_event_handler = PusEventHandler::<mpsc::Sender<PacketAsVec>>::new(
                tm_sender,
                verif_reporter,
                &mut event_manager,
                event_req_rx,
            );
            Self {
                event_tx,
                tm_receiver,
                event_manager,
                pus_event_handler,
            }
        }
    }

    #[test]
    fn test_basic_event_generation() {
        let mut testbench = EventManagementTestbench::new();
        testbench
            .event_tx
            .send(EventMessageU32::new(
                TEST_CREATOR_ID.id(),
                EventU32::new(satrs::events_legacy::Severity::Info, 1, 1),
            ))
            .expect("failed to send event");
        testbench.pus_event_handler.handle_event_requests();
        testbench.event_manager.try_event_handling(|_, _| {});
        testbench.pus_event_handler.generate_pus_event_tm();
        let tm_packet = testbench
            .tm_receiver
            .try_recv()
            .expect("failed to receive TM packet");
        assert_eq!(tm_packet.sender_id, PUS_EVENT_MANAGEMENT.id());
        let tm_reader = PusTmReader::new(&tm_packet.packet, 7).expect("failed to create TM reader");
        assert_eq!(tm_reader.apid(), TEST_CREATOR_ID.apid);
        assert_eq!(tm_reader.user_data().len(), 4);
        let event_read_back = EventU32::from_be_bytes(tm_reader.user_data().try_into().unwrap());
        assert_eq!(event_read_back, TEST_EVENT);
    }

    #[test]
    fn test_basic_event_disabled() {
        // TODO: Add test.
    }
}
*/
