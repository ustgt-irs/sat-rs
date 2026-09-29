use alloc::vec::Vec;
use arbitrary_int::u14;
use defmt::Debug2Format;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use spacepackets::{CcsdsPacketIdAndPsc, CcsdsPacketReader, SpHeader};
use types::ccsds::{CcsdsCreationError, CcsdsTmPacketOwned};
use types::{Apid, ComponentId, Message, TcHeader, TmHeader, control, led, tmtc};

use crate::leds::LED_MODE;
use crate::net::LAST_SENDER;
use crate::sim_client;

pub const TC_QUEUE_DEPTH: usize = 32;
pub const TM_QUEUE_DEPTH: usize = 32;

pub type TcChannel = Channel<NoopRawMutex, Vec<u8>, TC_QUEUE_DEPTH>;
pub type TmChannel = Channel<NoopRawMutex, Vec<u8>, TM_QUEUE_DEPTH>;

pub type TcSender = Sender<'static, NoopRawMutex, Vec<u8>, TC_QUEUE_DEPTH>;
pub type TcReceiver = Receiver<'static, NoopRawMutex, Vec<u8>, TC_QUEUE_DEPTH>;
pub type TmSender = Sender<'static, NoopRawMutex, Vec<u8>, TM_QUEUE_DEPTH>;
pub type TmReceiver = Receiver<'static, NoopRawMutex, Vec<u8>, TM_QUEUE_DEPTH>;

pub async fn tc_handler(tc_rx: TcReceiver, telemetry: &mut Telemetry) {
    loop {
        let tc = tc_rx.receive().await;
        let packet = match CcsdsPacketReader::new_with_checksum(&tc) {
            Ok(packet) => packet,
            Err(e) => {
                defmt::warn!("Failed to parse received TC packet: {}", e);
                send_tmtc_event(telemetry, tmtc::Event::InvalidTcPacket).await;
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
            send_tmtc_event(telemetry, tmtc::Event::InvalidTcHeader).await;
            continue;
        };
        match tc_header.target_id {
            ComponentId::Controller => handle_controller_tc(payload, tc_id, telemetry).await,
            ComponentId::Led => handle_led_tc(payload, tc_id, telemetry).await,
            target_id => {
                defmt::warn!("No TC handler for target ID {}", Debug2Format(&target_id));
                send_tmtc_event(telemetry, tmtc::Event::UnknownTargetId(target_id)).await;
            }
        }
    }
}

/// All TCs are received via UDP, so the UDP server is the sender of TMTC events.
async fn send_tmtc_event(telemetry: &mut Telemetry, event: tmtc::Event) {
    telemetry.send(ComponentId::UdpServer, None, &event).await;
}

/// The controller does not control anything yet, but handles generic requests like pings.
async fn handle_controller_tc(
    payload: &[u8],
    tc_id: CcsdsPacketIdAndPsc,
    telemetry: &mut Telemetry,
) {
    let Ok(request) = postcard::from_bytes::<control::request::Request>(payload) else {
        defmt::warn!("Failed to deserialize controller request");
        return;
    };
    match request {
        control::request::Request::Ping => defmt::info!("Received controller ping request"),
        control::request::Request::TestEvent => {
            defmt::info!("Received test event request");
            let event = types::Event::ControllerEvent(control::Event::TestEvent);
            telemetry.send(ComponentId::Controller, None, &event).await;
        }
        control::request::Request::SimConnect(opt_ipv4_addr) => match opt_ipv4_addr {
            Some(ipv4_addr) => sim_client::SIM_HOST.signal(ipv4_addr),
            None => {
                // If the UDP socket has received anything, the last sender should be stored and
                // we use that IP address.
                let opt_last_sender = LAST_SENDER.lock(|val| val.clone());
                match opt_last_sender.get() {
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
            defmt::info!("Received LED mode request: {}", Debug2Format(&mode));
            LED_MODE.signal(mode);
        }
    }
    telemetry
        .send(ComponentId::Led, Some(tc_id), &led::response::Response::Ok)
        .await;
}

/// Packs TM and passes it to the UDP task.
pub struct Telemetry {
    tx: TmSender,
    sequence_count: u14,
}

impl Telemetry {
    pub fn new(tx: TmSender) -> Self {
        Self {
            tx,
            sequence_count: u14::new(0),
        }
    }

    /// TM without a TC ID is sent unsolicited, for example events.
    async fn send(
        &mut self,
        sender_id: ComponentId,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        payload: &(impl serde::Serialize + Message),
    ) {
        let sp_header = SpHeader::new_for_unseg_tm(Apid::Tmtc.raw_value(), self.sequence_count, 0);
        let tm_header = TmHeader::new_without_timestamp(
            sender_id,
            ComponentId::Ground,
            payload.message_type(),
            tc_id,
        );
        match CcsdsTmPacketOwned::new_with_serde_payload(sp_header, &tm_header, payload)
            .map_err(CcsdsCreationError::from)
            .and_then(|packet| packet.try_to_vec())
        {
            Ok(raw_packet) => {
                self.tx.send(raw_packet).await;
                self.sequence_count = self.sequence_count.wrapping_add(u14::new(1));
            }
            Err(e) => defmt::warn!("Failed to create TM packet: {}", Debug2Format(&e)),
        }
    }
}
