use types::{
    ComponentId, Event, Message,
    acs::{mgm, mgm_assembly},
    ccsds::CcsdsTmPacketOwned,
    control, pcdu, tmtc,
};

use crate::ccsds::pack_ccsds_tm_packet_for_now;

// TODO: We should add the capability to enable/disable the TM generation of individual events and
// event groups as well.
pub struct EventManager {
    pub ctrl_rx: std::sync::mpsc::Receiver<control::Event>,
    /// Shared by all MGM instances, which is why the sender ID is part of the message.
    pub mgm_rx: std::sync::mpsc::Receiver<(ComponentId, mgm::Event)>,
    pub mgm_assembly_rx: std::sync::mpsc::Receiver<mgm_assembly::Event>,
    pub pcdu_rx: std::sync::mpsc::Receiver<pcdu::Event>,
    /// Shared by all TC sources, which is why the sender ID is part of the message.
    pub tc_source_rx: std::sync::mpsc::Receiver<(ComponentId, tmtc::Event)>,
    pub tm_tx: std::sync::mpsc::SyncSender<CcsdsTmPacketOwned>,
}

impl EventManager {
    pub fn periodic_operation(&mut self) {
        while let Ok(event) = self.ctrl_rx.try_recv() {
            self.event_to_tm(ComponentId::Controller, &Event::ControllerEvent(event));
        }
        while let Ok((sender_id, event)) = self.mgm_rx.try_recv() {
            self.event_to_tm(sender_id, &event);
        }
        while let Ok(event) = self.mgm_assembly_rx.try_recv() {
            self.event_to_tm(ComponentId::AcsMgmAssembly, &event);
        }
        while let Ok(event) = self.pcdu_rx.try_recv() {
            self.event_to_tm(ComponentId::EpsPcdu, &event);
        }
        while let Ok((sender_id, event)) = self.tc_source_rx.try_recv() {
            self.event_to_tm(sender_id, &event);
        }
    }

    pub fn event_to_tm(
        &mut self,
        sender_id: ComponentId,
        event: &(impl serde::Serialize + Message),
    ) {
        match pack_ccsds_tm_packet_for_now(sender_id, None, event) {
            Ok(packet) => {
                if let Err(e) = self.tm_tx.send(packet) {
                    log::warn!("error sending event TM packet: {:?}", e);
                }
            }
            Err(e) => {
                log::warn!("error packing event TM packet: {:?}", e);
            }
        }
    }
}
