use crate::config::{PCDU_SWITCH_REQUEST_QUEUE_DEPTH, PCDU_TC_QUEUE_DEPTH};
use crate::event_manager::{EventMessage, EventSender};
use crate::sim_client::{PcduSimReplyReceiver, SIM_AVAILABLE, SimRequestSender};
use crate::tmtc::{TmFunnelSender, pack_ccsds_tm_packet_for_now};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_time::Timer;
use minisim_types::{
    SimRequestWithTime,
    eps::{PcduReply, PcduRequest},
};
use spacepackets::CcsdsPacketIdAndPsc;
use types::{
    ComponentId,
    ccsds::CcsdsTcPacketOwned,
    pcdu::{
        self, SwitchId, SwitchMapBinaryWrapper, SwitchRequest, SwitchSet, SwitchStateBinary,
        SwitchesBitfield,
    },
};

pub type SharedSwitchSet = embassy_sync::mutex::Mutex<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    SwitchSet,
>;

pub fn new_shared_switch_set() -> SharedSwitchSet {
    SharedSwitchSet::new(SwitchSet::new_with_init_switches_unknown())
}

/// Interface for an ideal PCDU which never fails.
#[derive(Default, Debug)]
pub struct DummyInterface {
    switch_map: SwitchMapBinaryWrapper,
    replies: heapless::deque::Deque<PcduReply, 16>,
}

impl DummyInterface {
    fn send(&mut self, request: PcduRequest) {
        match request {
            PcduRequest::SwitchDevice { switch, state } => {
                self.switch_map
                    .0
                    .insert(switch, state)
                    .expect("switch map capacity exceeded");
            }
            PcduRequest::RequestSwitchInfo { id } => {
                match self.replies.push_back(PcduReply::SwitchInfo {
                    id,
                    switch_map: self.switch_map.0.clone(),
                }) {
                    Ok(_) => (),
                    Err(_) => {
                        defmt::error!("dummy reply vector is full");
                    }
                }
            }
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct CommError;

pub struct SimInterface {
    pub sim_request_tx: SimRequestSender,
    pub sim_reply_rx: PcduSimReplyReceiver,
}

impl core::fmt::Debug for SimInterface {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SimInterface").finish_non_exhaustive()
    }
}

impl SimInterface {
    pub fn new(sim_request_tx: SimRequestSender, sim_reply_rx: PcduSimReplyReceiver) -> Self {
        Self {
            sim_request_tx,
            sim_reply_rx,
        }
    }

    fn send(&mut self, request: PcduRequest) -> Result<(), CommError> {
        self.sim_request_tx
            .try_send(SimRequestWithTime::new_with_epoch_time(request))
            .map_err(|_| CommError)
    }

    fn try_recv_reply(&mut self) -> Option<PcduReply> {
        self.sim_reply_rx.try_receive().ok()
    }
}

// Boxing the dummy would only move it to the heap. A static size is preferable here.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum PcduCommunication {
    Dummy(DummyInterface),
    Sim(SimInterface),
}

impl PcduCommunication {
    fn send(&mut self, request: PcduRequest) -> Result<(), CommError> {
        match self {
            PcduCommunication::Dummy(dummy) => {
                dummy.send(request);
                Ok(())
            }
            PcduCommunication::Sim(sim) => sim.send(request),
        }
    }

    fn try_recv_reply(&mut self) -> Option<PcduReply> {
        match self {
            PcduCommunication::Dummy(dummy) => dummy.replies.pop_front(),
            PcduCommunication::Sim(sim) => sim.try_recv_reply(),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum OpCode {
    RegularOp = 0,
    PollAndRecvReplies = 1,
}

pub type PcduTcChannel = Channel<CriticalSectionRawMutex, CcsdsTcPacketOwned, PCDU_TC_QUEUE_DEPTH>;
pub type PcduTcSender =
    Sender<'static, CriticalSectionRawMutex, CcsdsTcPacketOwned, PCDU_TC_QUEUE_DEPTH>;
pub type PcduTcReceiver =
    Receiver<'static, CriticalSectionRawMutex, CcsdsTcPacketOwned, PCDU_TC_QUEUE_DEPTH>;

pub type PcduSwitchRequestChannel =
    Channel<CriticalSectionRawMutex, SwitchRequest, PCDU_SWITCH_REQUEST_QUEUE_DEPTH>;
pub type PcduSwitchRequestReceiver =
    Receiver<'static, CriticalSectionRawMutex, SwitchRequest, PCDU_SWITCH_REQUEST_QUEUE_DEPTH>;

/// Runs the PCDU handler with the same timing as the hosted example. The simulator can connect
/// or disconnect at any time, so the COM interface follows [SIM_AVAILABLE].
pub async fn pcdu_task(
    handler: &mut PcduHandler,
    sim_request_tx: SimRequestSender,
    sim_reply_rx: PcduSimReplyReceiver,
) {
    let Some(mut sim_available) = SIM_AVAILABLE.receiver() else {
        defmt::error!("No receiver left for the simulator connection state");
        return;
    };
    loop {
        if let Some(available) = sim_available.try_changed() {
            let using_sim = matches!(handler.com, PcduCommunication::Sim(_));
            if available != using_sim {
                defmt::info!("PCDU uses the simulator: {}", available);
                let com = if available {
                    // Replies from an earlier simulator connection are outdated.
                    while sim_reply_rx.try_receive().is_ok() {}
                    PcduCommunication::Sim(SimInterface::new(sim_request_tx, sim_reply_rx))
                } else {
                    PcduCommunication::Dummy(DummyInterface::default())
                };
                handler.set_com(com).await;
            }
        }
        handler.periodic_operation(OpCode::RegularOp).await;
        Timer::after_millis(50).await;
        handler.periodic_operation(OpCode::PollAndRecvReplies).await;
        Timer::after_millis(50).await;
        handler.periodic_operation(OpCode::PollAndRecvReplies).await;
        Timer::after_millis(300).await;
    }
}

/// TC which is answered with the next switch info reply.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum SwitchInfoRecipient {
    Ping(CcsdsPacketIdAndPsc),
    GetSwitches(CcsdsPacketIdAndPsc),
}

const MAX_SWITCH_INFO_RECIPIENTS: usize = 8;

/// Example PCDU device handler.
pub struct PcduHandler {
    switch_request_rx: PcduSwitchRequestReceiver,
    tc_rx: PcduTcReceiver,
    tm_tx: TmFunnelSender,
    com: PcduCommunication,
    shared_switch_map: &'static SharedSwitchSet,
    /// ID of the switch info request in flight. Only the reply with this ID is accepted, so a late
    /// reply can not answer a newer request.
    switch_info_pending: Option<u16>,
    next_switch_info_id: u16,
    switch_info_recipients: heapless::Vec<SwitchInfoRecipient, MAX_SWITCH_INFO_RECIPIENTS>,
    event_tx: EventSender,
}

impl PcduHandler {
    pub fn new(
        tc_rx: PcduTcReceiver,
        tm_tx: TmFunnelSender,
        switch_request_rx: PcduSwitchRequestReceiver,
        com: PcduCommunication,
        shared_switch_map: &'static SharedSwitchSet,
        event_tx: EventSender,
    ) -> Self {
        Self {
            tc_rx,
            switch_request_rx,
            tm_tx,
            com,
            shared_switch_map,
            switch_info_pending: None,
            next_switch_info_id: 0,
            switch_info_recipients: heapless::Vec::new(),
            event_tx,
        }
    }

    pub async fn periodic_operation(&mut self, op_code: OpCode) {
        match op_code {
            OpCode::RegularOp => {
                // A late reply still answers the request of the last cycle.
                self.poll_and_handle_replies().await;
                self.drop_unanswered_switch_info_request().await;
                // Handle requests.
                self.handle_telecommands().await;
                self.handle_switch_requests();
                // Sent after all switch commands, so the reply is fresh for all recipients.
                self.request_switch_info().await;
            }
            OpCode::PollAndRecvReplies => {
                self.poll_and_handle_replies().await;
            }
        }
    }

    pub async fn handle_telecommands(&mut self) {
        while let Ok(packet) = self.tc_rx.try_receive() {
            let tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&packet.sp_header);
            match postcard::from_bytes::<pcdu::request::Request>(&packet.payload) {
                Ok(request) => {
                    defmt::info!(
                        "received request {:?} with TC ID {:#010x}",
                        request,
                        tc_id.raw()
                    );
                    match request {
                        pcdu::request::Request::Ping => {
                            self.add_switch_info_recipient(SwitchInfoRecipient::Ping(tc_id))
                        }
                        pcdu::request::Request::GetSwitches => {
                            self.add_switch_info_recipient(SwitchInfoRecipient::GetSwitches(tc_id))
                        }
                        pcdu::request::Request::EnableSwitches(switches) => {
                            self.handle_switches_bitfield_request(switches, SwitchStateBinary::On);
                        }
                        pcdu::request::Request::DisableSwitches(switches) => {
                            self.handle_switches_bitfield_request(switches, SwitchStateBinary::Off);
                        }
                    };
                }
                Err(e) => {
                    defmt::warn!("failed to deserialize request: {}", e);
                }
            }
        }
    }

    /// Pings are answered with the switch info reply as well, so they cover the round trip to the
    /// PCDU.
    pub async fn request_switch_info(&mut self) {
        let id = self.next_switch_info_id;
        self.next_switch_info_id = id.wrapping_add(1);
        if self
            .com
            .send(PcduRequest::RequestSwitchInfo { id })
            .is_err()
        {
            defmt::warn!("requesting PCDU switch info failed");
            self.invalidate_switch_set().await;
            // Do not block the handler, a full event queue already holds comm errors.
            if self
                .event_tx
                .try_send(EventMessage::Pcdu(pcdu::Event::SerialCommError))
                .is_err()
            {
                defmt::warn!("event queue full, dropping comm error event");
            }
            return;
        }
        self.switch_info_pending = Some(id);
    }

    /// The dummy and the simulator each have their own switch states, so the switch set is
    /// invalid until the new interface reported them.
    pub async fn set_com(&mut self, com: PcduCommunication) {
        self.com = com;
        self.switch_info_pending = None;
        self.invalidate_switch_set().await;
    }

    fn add_switch_info_recipient(&mut self, recipient: SwitchInfoRecipient) {
        if self.switch_info_recipients.push(recipient).is_err() {
            defmt::warn!("too many TCs waiting for switch info, dropping TC");
        }
    }

    async fn drop_unanswered_switch_info_request(&mut self) {
        if self.switch_info_pending.take().is_some() {
            defmt::warn!("switch info request timed out");
            self.invalidate_switch_set().await;
        }
        for recipient in self.switch_info_recipients.drain(..) {
            let (SwitchInfoRecipient::Ping(tc_id) | SwitchInfoRecipient::GetSwitches(tc_id)) =
                recipient;
            defmt::warn!("no switch info reply for TC ID {:#010x}", tc_id.raw());
        }
    }

    /// The switch set is only valid while the PCDU answers.
    async fn invalidate_switch_set(&self) {
        self.shared_switch_map.lock().await.valid = false;
    }

    pub fn handle_switches_bitfield_request(
        &mut self,
        switches: SwitchesBitfield,
        state: SwitchStateBinary,
    ) {
        if switches.mgm0() {
            self.handle_device_switching(SwitchId::Mgm0, state);
        }
        if switches.mgm1() {
            self.handle_device_switching(SwitchId::Mgm1, state);
        }
        if switches.magnetorquer() {
            self.handle_device_switching(SwitchId::Mgt, state);
        }
    }

    pub async fn send_tm(
        &self,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        response: pcdu::response::Response,
    ) {
        match pack_ccsds_tm_packet_for_now(ComponentId::EpsPcdu, tc_id, &response) {
            Ok(packet) => {
                self.tm_tx.send(packet).await;
            }
            Err(e) => {
                defmt::warn!("failed to pack TM packet: {}", e);
            }
        }
    }

    pub fn handle_device_switching(&mut self, switch_id: SwitchId, state: SwitchStateBinary) {
        let pcdu_req = PcduRequest::SwitchDevice {
            switch: switch_id,
            state,
        };
        if self.com.send(pcdu_req).is_err() {
            defmt::warn!("sending switch request to PCDU failed");
            if self
                .event_tx
                .try_send(EventMessage::Pcdu(pcdu::Event::SerialCommError))
                .is_err()
            {
                defmt::warn!("event queue full, dropping comm error event");
            }
        }
    }

    pub fn handle_switch_requests(&mut self) {
        while let Ok(switch_req) = self.switch_request_rx.try_receive() {
            self.handle_device_switching(switch_req.switch_id(), switch_req.target_state());
        }
    }

    pub async fn poll_and_handle_replies(&mut self) {
        while let Some(reply) = self.com.try_recv_reply() {
            match reply {
                PcduReply::SwitchInfo { id, switch_map } => {
                    if self.switch_info_pending != Some(id) {
                        defmt::warn!("ignoring switch info reply with unexpected ID {}", id);
                        continue;
                    }
                    // Treated like a missing reply, so the request times out.
                    let Some((switch_set, switches)) =
                        SwitchSet::from_binary_switch_map(&switch_map)
                    else {
                        defmt::warn!("ignoring switch info reply with missing switches");
                        continue;
                    };
                    self.switch_info_pending = None;
                    *self.shared_switch_map.lock().await = switch_set;
                    for recipient in core::mem::take(&mut self.switch_info_recipients) {
                        match recipient {
                            SwitchInfoRecipient::Ping(tc_id) => {
                                self.send_tm(Some(tc_id), pcdu::response::Response::Ok)
                                    .await
                            }
                            SwitchInfoRecipient::GetSwitches(tc_id) => {
                                self.send_tm(
                                    Some(tc_id),
                                    pcdu::response::Response::Switches(switches),
                                )
                                .await
                            }
                        }
                    }
                }
            }
        }
    }
}
