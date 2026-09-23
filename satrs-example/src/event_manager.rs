use std::collections::HashSet;

use satrs::spacepackets::CcsdsPacketIdAndPsc;
use types::{
    ComponentId, Event, EventId, Message,
    acs::{mgm, mgm_assembly},
    ccsds::{CcsdsTcPacketOwned, CcsdsTmPacketOwned},
    control,
    event_manager::{request::Request, response::Response},
    pcdu, tmtc,
};

use crate::ccsds::pack_ccsds_tm_packet_for_now;

pub struct EventManager {
    pub tc_rx: std::sync::mpsc::Receiver<CcsdsTcPacketOwned>,
    pub ctrl_rx: std::sync::mpsc::Receiver<control::Event>,
    /// Shared by all MGM instances, which is why the sender ID is part of the message.
    pub mgm_rx: std::sync::mpsc::Receiver<(ComponentId, mgm::Event)>,
    pub mgm_assembly_rx: std::sync::mpsc::Receiver<mgm_assembly::Event>,
    pub pcdu_rx: std::sync::mpsc::Receiver<pcdu::Event>,
    /// Shared by all TC sources, which is why the sender ID is part of the message.
    pub tc_source_rx: std::sync::mpsc::Receiver<(ComponentId, tmtc::Event)>,
    pub tm_tx: std::sync::mpsc::SyncSender<CcsdsTmPacketOwned>,
    /// Senders with all their event TM generation disabled.
    disabled_components: HashSet<ComponentId>,
    /// Individual (sender, event ID) pairs with their TM generation disabled.
    disabled_events: HashSet<(ComponentId, u16)>,
}

impl EventManager {
    pub fn new(
        tc_rx: std::sync::mpsc::Receiver<CcsdsTcPacketOwned>,
        ctrl_rx: std::sync::mpsc::Receiver<control::Event>,
        mgm_rx: std::sync::mpsc::Receiver<(ComponentId, mgm::Event)>,
        mgm_assembly_rx: std::sync::mpsc::Receiver<mgm_assembly::Event>,
        pcdu_rx: std::sync::mpsc::Receiver<pcdu::Event>,
        tc_source_rx: std::sync::mpsc::Receiver<(ComponentId, tmtc::Event)>,
        tm_tx: std::sync::mpsc::SyncSender<CcsdsTmPacketOwned>,
    ) -> Self {
        Self {
            tc_rx,
            ctrl_rx,
            mgm_rx,
            mgm_assembly_rx,
            pcdu_rx,
            tc_source_rx,
            tm_tx,
            disabled_components: HashSet::new(),
            disabled_events: HashSet::new(),
        }
    }

    /// Silences all event TM generation for `sender_id`, regardless of the specific event.
    fn disable_component(&mut self, sender_id: ComponentId) {
        self.disabled_components.insert(sender_id);
    }

    fn enable_component(&mut self, sender_id: ComponentId) {
        self.disabled_components.remove(&sender_id);
    }

    /// Silences TM generation for one specific event of `sender_id`, leaving its other events
    /// unaffected.
    fn disable_event(&mut self, sender_id: ComponentId, event_id: u16) {
        self.disabled_events.insert((sender_id, event_id));
    }

    fn enable_event(&mut self, sender_id: ComponentId, event_id: u16) {
        self.disabled_events.remove(&(sender_id, event_id));
    }

    fn tm_generation_enabled(&self, sender_id: ComponentId, event_id: u16) -> bool {
        !self.disabled_components.contains(&sender_id)
            && !self.disabled_events.contains(&(sender_id, event_id))
    }

    pub fn periodic_operation(&mut self) {
        // Telecommands first, so filter changes already apply to the events of this cycle.
        self.handle_telecommands();
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

    fn handle_telecommands(&mut self) {
        while let Ok(packet) = self.tc_rx.try_recv() {
            let tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&packet.sp_header);
            let request = match postcard::from_bytes::<Request>(&packet.payload) {
                Ok(request) => request,
                Err(e) => {
                    log::warn!("failed to deserialize event manager request: {}", e);
                    continue;
                }
            };
            log::info!(
                "received request {:?} with TC ID {:#010x}",
                request,
                tc_id.raw()
            );
            match request {
                Request::EnableComponent(sender_id) => self.enable_component(sender_id),
                Request::DisableComponent(sender_id) => self.disable_component(sender_id),
                Request::EnableEvent {
                    sender_id,
                    event_id,
                } => self.enable_event(sender_id, event_id),
                Request::DisableEvent {
                    sender_id,
                    event_id,
                } => self.disable_event(sender_id, event_id),
            }
            self.send_tm(ComponentId::EventManager, Some(tc_id), &Response::Ok);
        }
    }

    pub fn event_to_tm(
        &mut self,
        sender_id: ComponentId,
        event: &(impl serde::Serialize + Message + EventId + core::fmt::Debug),
    ) {
        let tm_enabled = self.tm_generation_enabled(sender_id, event.event_id());
        log::info!(
            "event {:?} (ID {}) from {:?}{}",
            event,
            event.event_id(),
            sender_id,
            if tm_enabled { "" } else { ", TM disabled" }
        );
        if !tm_enabled {
            return;
        }
        self.send_tm(sender_id, None, event);
    }

    fn send_tm(
        &self,
        sender_id: ComponentId,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        payload: &(impl serde::Serialize + Message),
    ) {
        match pack_ccsds_tm_packet_for_now(sender_id, tc_id, payload) {
            Ok(packet) => {
                if let Err(e) = self.tm_tx.send(packet) {
                    log::warn!("error sending TM packet: {:?}", e);
                }
            }
            Err(e) => {
                log::warn!("error packing TM packet: {:?}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{self, TryRecvError};

    use arbitrary_int::u11;
    use satrs::spacepackets::SpacePacketHeader;
    use types::{Apid, MessageType, TcHeader, acs::mgm};

    use super::*;

    struct Testbench {
        tc_tx: mpsc::SyncSender<CcsdsTcPacketOwned>,
        mgm_event_tx: mpsc::SyncSender<(ComponentId, mgm::Event)>,
        tm_rx: mpsc::Receiver<CcsdsTmPacketOwned>,
        event_manager: EventManager,
    }

    impl Testbench {
        fn new() -> Self {
            let (tc_tx, tc_rx) = mpsc::sync_channel(5);
            let (_ctrl_tx, ctrl_rx) = mpsc::sync_channel(5);
            let (mgm_event_tx, mgm_rx) = mpsc::sync_channel(5);
            let (_mgm_assembly_tx, mgm_assembly_rx) = mpsc::sync_channel(5);
            let (_pcdu_tx, pcdu_rx) = mpsc::sync_channel(5);
            let (_tc_source_tx, tc_source_rx) = mpsc::sync_channel(5);
            let (tm_tx, tm_rx) = mpsc::sync_channel(5);
            Self {
                tc_tx,
                mgm_event_tx,
                tm_rx,
                event_manager: EventManager::new(
                    tc_rx,
                    ctrl_rx,
                    mgm_rx,
                    mgm_assembly_rx,
                    pcdu_rx,
                    tc_source_rx,
                    tm_tx,
                ),
            }
        }

        fn send_request(&self, request: Request) {
            self.tc_tx
                .send(CcsdsTcPacketOwned::new_with_request(
                    SpacePacketHeader::new_from_apid(u11::new(Apid::Tmtc as u16)),
                    TcHeader::new(ComponentId::EventManager, MessageType::Event),
                    request,
                ))
                .unwrap();
        }
    }

    #[test]
    fn test_events_enabled_by_default() {
        let mut tb = Testbench::new();
        tb.mgm_event_tx
            .send((ComponentId::AcsMgm0, mgm::Event::SpiFaultThresholdExceeded))
            .unwrap();
        tb.event_manager.periodic_operation();
        tb.tm_rx.try_recv().expect("expected event TM");
    }

    #[test]
    fn test_disabled_component_silences_all_its_events() {
        let mut tb = Testbench::new();
        tb.event_manager.disable_component(ComponentId::AcsMgm0);
        tb.mgm_event_tx
            .send((ComponentId::AcsMgm0, mgm::Event::SpiFaultThresholdExceeded))
            .unwrap();
        tb.mgm_event_tx
            .send((
                ComponentId::AcsMgm0,
                mgm::Event::ModeChanged(types::DeviceMode::Normal),
            ))
            .unwrap();
        tb.event_manager.periodic_operation();
        assert!(matches!(tb.tm_rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn test_disabled_component_does_not_affect_other_components() {
        let mut tb = Testbench::new();
        tb.event_manager.disable_component(ComponentId::AcsMgm1);
        tb.mgm_event_tx
            .send((ComponentId::AcsMgm0, mgm::Event::SpiFaultThresholdExceeded))
            .unwrap();
        tb.event_manager.periodic_operation();
        tb.tm_rx.try_recv().expect("expected event TM");
    }

    #[test]
    fn test_disabled_event_only_silences_that_specific_event() {
        let mut tb = Testbench::new();
        tb.event_manager.disable_event(
            ComponentId::AcsMgm0,
            mgm::Event::SpiFaultThresholdExceeded.event_id(),
        );
        tb.mgm_event_tx
            .send((ComponentId::AcsMgm0, mgm::Event::SpiFaultThresholdExceeded))
            .unwrap();
        tb.mgm_event_tx
            .send((
                ComponentId::AcsMgm0,
                mgm::Event::ModeChanged(types::DeviceMode::Normal),
            ))
            .unwrap();
        tb.event_manager.periodic_operation();
        let tm = tb.tm_rx.try_recv().expect("expected event TM");
        let event: mgm::Event = postcard::from_bytes(&tm.payload).unwrap();
        assert!(matches!(event, mgm::Event::ModeChanged(_)));
        assert!(matches!(tb.tm_rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn test_re_enabling_a_component_restores_its_events() {
        let mut tb = Testbench::new();
        tb.event_manager.disable_component(ComponentId::AcsMgm0);
        tb.event_manager.enable_component(ComponentId::AcsMgm0);
        tb.mgm_event_tx
            .send((ComponentId::AcsMgm0, mgm::Event::SpiFaultThresholdExceeded))
            .unwrap();
        tb.event_manager.periodic_operation();
        tb.tm_rx.try_recv().expect("expected event TM");
    }

    #[test]
    fn test_disable_component_request() {
        let mut tb = Testbench::new();
        tb.send_request(Request::DisableComponent(ComponentId::AcsMgm0));
        tb.mgm_event_tx
            .send((ComponentId::AcsMgm0, mgm::Event::SpiFaultThresholdExceeded))
            .unwrap();
        tb.event_manager.periodic_operation();

        let tm = tb.tm_rx.try_recv().expect("expected request response TM");
        assert_eq!(tm.tm_header.sender_id, ComponentId::EventManager);
        assert!(tm.tm_header.tc_id.is_some());
        let response: Response = postcard::from_bytes(&tm.payload).unwrap();
        assert_eq!(response, Response::Ok);
        assert!(matches!(tb.tm_rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn test_disable_event_request() {
        let mut tb = Testbench::new();
        tb.send_request(Request::DisableEvent {
            sender_id: ComponentId::AcsMgm0,
            event_id: mgm::Event::SpiFaultThresholdExceeded.event_id(),
        });
        tb.event_manager.periodic_operation();
        tb.tm_rx.try_recv().expect("expected request response TM");

        tb.send_request(Request::EnableEvent {
            sender_id: ComponentId::AcsMgm0,
            event_id: mgm::Event::SpiFaultThresholdExceeded.event_id(),
        });
        tb.mgm_event_tx
            .send((ComponentId::AcsMgm0, mgm::Event::SpiFaultThresholdExceeded))
            .unwrap();
        tb.event_manager.periodic_operation();
        tb.tm_rx.try_recv().expect("expected request response TM");
        tb.tm_rx.try_recv().expect("expected event TM");
    }
}
