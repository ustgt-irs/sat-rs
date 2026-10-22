use alloc::vec::Vec;
use arbitrary_int::{u11, u14};
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, NoopRawMutex};
use embassy_sync::channel::{Channel, Receiver, Sender};
use heapless::LinearMap;
use spacepackets::{CcsdsPacketIdAndPsc, CcsdsPacketReader, SpHeader};
use types::ccsds::{CcsdsTcPacketOwned, CcsdsTmPacketOwned};
use types::{Apid, ComponentId, Message, TcHeader, TmHeader, control, led, tmtc};

use crate::config::{TC_QUEUE_DEPTH, TM_FUNNEL_QUEUE_DEPTH, TM_QUEUE_DEPTH};
use crate::event_manager::{EventMessage, EventSender};
use crate::leds::LED_MODE;
use crate::net::LAST_SENDER;
use crate::pcdu::PcduTcSender;
use crate::sim_client;

pub type TcChannel = Channel<NoopRawMutex, Vec<u8>, TC_QUEUE_DEPTH>;
pub type TmChannel = Channel<NoopRawMutex, Vec<u8>, TM_QUEUE_DEPTH>;

pub type TcSender = Sender<'static, NoopRawMutex, Vec<u8>, TC_QUEUE_DEPTH>;
pub type TcReceiver = Receiver<'static, NoopRawMutex, Vec<u8>, TC_QUEUE_DEPTH>;
pub type TmSender = Sender<'static, NoopRawMutex, Vec<u8>, TM_QUEUE_DEPTH>;
pub type TmReceiver = Receiver<'static, NoopRawMutex, Vec<u8>, TM_QUEUE_DEPTH>;

// Critical section mutex, because the producers can run in RTIC tasks with different priorities.
pub type TmFunnelChannel =
    Channel<CriticalSectionRawMutex, CcsdsTmPacketOwned, TM_FUNNEL_QUEUE_DEPTH>;
pub type TmFunnelSender =
    Sender<'static, CriticalSectionRawMutex, CcsdsTmPacketOwned, TM_FUNNEL_QUEUE_DEPTH>;
pub type TmFunnelReceiver =
    Receiver<'static, CriticalSectionRawMutex, CcsdsTmPacketOwned, TM_FUNNEL_QUEUE_DEPTH>;

/// One sender for each component which handles TCs in its own task.
#[derive(Clone, Copy)]
pub struct TcSenders {
    pub pcdu: PcduTcSender,
}

/// Number of APIDs which can send TM.
const MAX_TM_APIDS: usize = 4;

pub async fn tc_handler(
    tc_rx: TcReceiver,
    telemetry: &mut Telemetry,
    event_tx: EventSender,
    tc_senders: TcSenders,
) {
    loop {
        let tc = tc_rx.receive().await;
        let packet = match CcsdsPacketReader::new_with_checksum(&tc) {
            Ok(packet) => packet,
            Err(e) => {
                defmt::warn!("Failed to parse received TC packet: {}", e);
                send_tmtc_event(&event_tx, tmtc::Event::InvalidTcPacket).await;
                continue;
            }
        };
        let tc_id = CcsdsPacketIdAndPsc {
            packet_id: packet.packet_id(),
            psc: packet.psc(),
        };
        let Ok((tc_header, payload)) = postcard::take_from_bytes::<TcHeader>(packet.user_data())
        else {
            defmt::warn!("Failed to deserialize TC header");
            send_tmtc_event(&event_tx, tmtc::Event::InvalidTcHeader).await;
            continue;
        };
        match tc_header.target_id {
            ComponentId::Controller => {
                handle_controller_tc(payload, tc_id, telemetry, &event_tx).await
            }
            ComponentId::Led => handle_led_tc(payload, tc_id, telemetry).await,
            ComponentId::EpsPcdu => {
                let tc = CcsdsTcPacketOwned::new(*packet.sp_header(), tc_header, payload.to_vec());
                // Do not block the TC handler, the PCDU only reads its TCs periodically.
                if tc_senders.pcdu.try_send(tc).is_err() {
                    defmt::warn!("PCDU TC queue full, dropping TC");
                }
            }
            target_id => {
                defmt::warn!("No TC handler for target ID {}", &target_id);
                send_tmtc_event(&event_tx, tmtc::Event::UnknownTargetId(target_id)).await;
            }
        }
    }
}

/// All TCs are received via UDP, so the UDP server is the sender of TMTC events.
async fn send_tmtc_event(event_tx: &EventSender, event: tmtc::Event) {
    event_tx
        .send(EventMessage::TcSource(ComponentId::UdpServer, event))
        .await;
}

/// The controller does not control anything yet, but handles generic requests like pings.
async fn handle_controller_tc(
    payload: &[u8],
    tc_id: CcsdsPacketIdAndPsc,
    telemetry: &mut Telemetry,
    event_tx: &EventSender,
) {
    let Ok(request) = postcard::from_bytes::<control::request::Request>(payload) else {
        defmt::warn!("Failed to deserialize controller request");
        return;
    };
    match request {
        control::request::Request::Ping => defmt::info!("Received controller ping request"),
        control::request::Request::TestEvent => {
            defmt::info!("Received test event request");
            event_tx
                .send(EventMessage::Controller(control::Event::TestEvent))
                .await;
        }
        control::request::Request::SimConnect(opt_ipv4_addr) => match opt_ipv4_addr {
            Some(ipv4_addr) => sim_client::SIM_HOST.signal(ipv4_addr),
            None => {
                // If the UDP socket has received anything, the last sender should be stored and
                // we use that IP address.
                match LAST_SENDER.lock(|val| val.get()) {
                    Some(last_sender) => sim_client::SIM_HOST.signal(last_sender),
                    None => defmt::warn!("SimConnect without IP and no known sender"),
                }
            }
        },
    }
    telemetry
        .send(
            ComponentId::Controller,
            Some(tc_id),
            &control::response::Response::Ok,
        )
        .await;
}

async fn handle_led_tc(payload: &[u8], tc_id: CcsdsPacketIdAndPsc, telemetry: &mut Telemetry) {
    let Ok(request) = postcard::from_bytes::<led::request::Request>(payload) else {
        defmt::warn!("Failed to deserialize LED request");
        return;
    };
    match request {
        led::request::Request::Ping => defmt::info!("Received LED ping request"),
        led::request::Request::SetMode(mode) => {
            defmt::info!("Received LED mode request: {}", &mode);
            LED_MODE.signal(mode);
        }
    }
    telemetry
        .send(ComponentId::Led, Some(tc_id), &led::response::Response::Ok)
        .await;
}

/// There is no time source yet, so the TM is sent without a timestamp. The TM funnel sets the
/// sequence count.
pub fn pack_ccsds_tm_packet_for_now(
    sender_id: ComponentId,
    tc_id: Option<CcsdsPacketIdAndPsc>,
    payload: &(impl serde::Serialize + Message),
) -> Result<CcsdsTmPacketOwned, postcard::Error> {
    let sp_header = SpHeader::new_for_unseg_tm(Apid::Tmtc.raw_value(), u14::new(0), 0);
    let tm_header = TmHeader::new_without_timestamp(
        sender_id,
        ComponentId::Ground,
        payload.message_type(),
        tc_id,
    );
    CcsdsTmPacketOwned::new_with_serde_payload(sp_header, &tm_header, payload)
}

/// Packs TM and passes it to the TM funnel.
pub struct Telemetry {
    tx: TmFunnelSender,
}

impl Telemetry {
    pub fn new(tx: TmFunnelSender) -> Self {
        Self { tx }
    }

    /// TM without a TC ID is sent unsolicited, for example events.
    async fn send(
        &mut self,
        sender_id: ComponentId,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        payload: &(impl serde::Serialize + Message),
    ) {
        match pack_ccsds_tm_packet_for_now(sender_id, tc_id, payload) {
            Ok(packet) => self.tx.send(packet).await,
            Err(e) => defmt::warn!("Failed to create TM packet: {}", &e),
        }
    }
}

#[derive(Default)]
struct SeqCounterMap(LinearMap<u11, u14, MAX_TM_APIDS>);

impl SeqCounterMap {
    fn get_and_increment(&mut self, apid: u11) -> u14 {
        if let Some(seq_count) = self.0.get_mut(&apid) {
            let current = *seq_count;
            *seq_count = current.wrapping_add(u14::new(1));
            return current;
        }
        if self.0.insert(apid, u14::new(1)).is_err() {
            defmt::warn!("No sequence counter left for APID {}", apid.value());
        }
        u14::new(0)
    }
}

/// All TM passes through here, so each APID gets one gapless sequence count.
pub async fn tm_funnel_task(funnel_rx: TmFunnelReceiver, tm_tx: TmSender) {
    let mut seq_counts = SeqCounterMap::default();
    loop {
        let mut packet = funnel_rx.receive().await;
        packet
            .sp_header
            .set_seq_count(seq_counts.get_and_increment(packet.sp_header.apid()));
        match packet.try_to_vec() {
            Ok(raw_packet) => tm_tx.send(raw_packet).await,
            Err(e) => defmt::warn!("Failed to serialize TM packet: {}", &e),
        }
    }
}
