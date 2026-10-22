//! Converts the events of all components to TM. There is no event filtering yet.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use types::{ComponentId, EventId, Message, control, pcdu, tmtc};

use crate::config::EVENT_QUEUE_DEPTH;
use crate::tmtc::{TmFunnelSender, pack_ccsds_tm_packet_for_now};

#[derive(Debug, Clone, Copy)]
pub enum EventMessage {
    Controller(control::Event),
    /// TC sources share one event type, which is why the sender ID is part of the message.
    TcSource(ComponentId, tmtc::Event),
    Pcdu(pcdu::Event),
}

pub type EventChannel = Channel<CriticalSectionRawMutex, EventMessage, EVENT_QUEUE_DEPTH>;
pub type EventSender = Sender<'static, CriticalSectionRawMutex, EventMessage, EVENT_QUEUE_DEPTH>;
pub type EventReceiver =
    Receiver<'static, CriticalSectionRawMutex, EventMessage, EVENT_QUEUE_DEPTH>;

#[derive(Clone, Copy)]
pub struct EventManagerEndpoints {
    pub event_rx: EventReceiver,
    pub tm_tx: TmFunnelSender,
}

pub async fn event_manager_task(endpoints: EventManagerEndpoints) {
    let EventManagerEndpoints { event_rx, tm_tx } = endpoints;
    loop {
        match event_rx.receive().await {
            // The client expects controller events wrapped in the generic event type.
            EventMessage::Controller(event) => {
                event_to_tm(
                    &tm_tx,
                    ComponentId::Controller,
                    &types::Event::ControllerEvent(event),
                )
                .await
            }
            EventMessage::TcSource(sender_id, event) => {
                event_to_tm(&tm_tx, sender_id, &event).await
            }
            EventMessage::Pcdu(event) => event_to_tm(&tm_tx, ComponentId::EpsPcdu, &event).await,
        }
    }
}

async fn event_to_tm(
    tm_tx: &TmFunnelSender,
    sender_id: ComponentId,
    event: &(impl serde::Serialize + Message + EventId),
) {
    defmt::info!("Event ID {} from {}", event.event_id(), &sender_id);
    match pack_ccsds_tm_packet_for_now(sender_id, None, event) {
        Ok(packet) => tm_tx.send(packet).await,
        Err(e) => defmt::warn!("Failed to create event TM packet: {}", &e),
    }
}
