use satrs::{
    ComponentId as RawComponentId, HandlingStatus,
    spacepackets::{CcsdsPacketReader, ChecksumType},
    tmtc::PacketAsVec,
};
use std::{
    collections::HashMap,
    sync::mpsc::{self, TryRecvError},
};
use types::{ComponentId, TcHeader, ccsds::CcsdsTcPacketOwned, tmtc};

pub type CcsdsDistributor = HashMap<ComponentId, std::sync::mpsc::SyncSender<CcsdsTcPacketOwned>>;

// TC source components where the heap is the backing memory of the received telecommands.
pub struct TcSourceTask {
    pub tc_receiver: mpsc::Receiver<PacketAsVec>,
    ccsds_distributor: CcsdsDistributor,
    event_tx: mpsc::SyncSender<(ComponentId, tmtc::Event)>,
}

impl TcSourceTask {
    pub fn new(
        tc_receiver: mpsc::Receiver<PacketAsVec>,
        ccsds_distributor: CcsdsDistributor,
        event_tx: mpsc::SyncSender<(ComponentId, tmtc::Event)>,
    ) -> Self {
        Self {
            tc_receiver,
            ccsds_distributor,
            event_tx,
        }
    }

    pub fn add_target(
        &mut self,
        target_id: ComponentId,
        sender: mpsc::SyncSender<CcsdsTcPacketOwned>,
    ) {
        self.ccsds_distributor.insert(target_id, sender);
    }

    pub fn periodic_operation(&mut self) {
        loop {
            if self.poll_tc() == HandlingStatus::Empty {
                break;
            }
        }
    }

    pub fn poll_tc(&mut self) -> HandlingStatus {
        match self.tc_receiver.try_recv() {
            Ok(packet) => {
                log::debug!("received raw packet: {:?}", packet);
                let ccsds_tc_reader_result =
                    CcsdsPacketReader::new(&packet.packet, Some(ChecksumType::WithCrc16));
                if ccsds_tc_reader_result.is_err() {
                    log::warn!(
                        "received invalid CCSDS TC packet: {:?}",
                        ccsds_tc_reader_result.err()
                    );
                    self.send_event(packet.sender_id, tmtc::Event::InvalidTcPacket);
                    return HandlingStatus::HandledOne;
                }
                let ccsds_tc_reader = ccsds_tc_reader_result.unwrap();
                let tc_header_result =
                    postcard::take_from_bytes::<TcHeader>(ccsds_tc_reader.user_data());
                if tc_header_result.is_err() {
                    log::warn!(
                        "received CCSDS TC packet with invalid TC header: {:?}",
                        tc_header_result.err()
                    );
                    self.send_event(packet.sender_id, tmtc::Event::InvalidTcHeader);
                    return HandlingStatus::HandledOne;
                }
                let (tc_header, payload) = tc_header_result.unwrap();
                if let Some(sender) = self.ccsds_distributor.get(&tc_header.target_id) {
                    log::debug!("sending TC packet to target ID: {:?}", tc_header.target_id);
                    sender
                        .send(CcsdsTcPacketOwned {
                            sp_header: *ccsds_tc_reader.sp_header(),
                            tc_header,
                            payload: payload.to_vec(),
                        })
                        .ok();
                } else {
                    log::warn!("no TC handler for target ID {:?}", tc_header.target_id);
                    self.send_event(
                        packet.sender_id,
                        tmtc::Event::UnknownTargetId(tc_header.target_id),
                    );
                }
                HandlingStatus::HandledOne
            }
            Err(e) => match e {
                TryRecvError::Empty => HandlingStatus::Empty,
                TryRecvError::Disconnected => {
                    log::warn!("tmtc thread: sender disconnected");
                    HandlingStatus::Empty
                }
            },
        }
    }

    /// `sender_id` is the raw ID tagged on the received packet, which is not necessarily a
    /// known [ComponentId] (e.g. a spoofed or garbled packet). Falls back to [ComponentId::Ground]
    /// as the event sender in that case.
    fn send_event(&self, sender_id: RawComponentId, event: tmtc::Event) {
        let sender_id = ComponentId::try_from(sender_id).unwrap_or_else(|_| {
            log::warn!("TC source event for unknown raw sender ID {}", sender_id);
            ComponentId::Ground
        });
        if let Err(e) = self.event_tx.send((sender_id, event)) {
            log::warn!("failed to send TC source event: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::TryRecvError;

    use arbitrary_int::u11;
    use satrs::spacepackets::{
        CcsdsPacketCreatorOwned, ChecksumType, PacketType, SpacePacketHeader,
    };
    use types::{Apid, MessageType, ccsds::CcsdsTcPacketOwned};

    use super::*;

    struct Testbench {
        tc_tx: mpsc::SyncSender<PacketAsVec>,
        target_rx: mpsc::Receiver<CcsdsTcPacketOwned>,
        event_rx: mpsc::Receiver<(ComponentId, tmtc::Event)>,
        tc_source: TcSourceTask,
    }

    impl Testbench {
        fn new() -> Self {
            let (tc_tx, tc_source_rx) = mpsc::sync_channel(5);
            let (target_tx, target_rx) = mpsc::sync_channel(5);
            let (event_tx, event_rx) = mpsc::sync_channel(5);
            let mut tc_source =
                TcSourceTask::new(tc_source_rx, CcsdsDistributor::default(), event_tx);
            tc_source.add_target(ComponentId::EpsPcdu, target_tx);
            Self {
                tc_tx,
                target_rx,
                event_rx,
                tc_source,
            }
        }
    }

    fn valid_tc_raw(target_id: ComponentId) -> Vec<u8> {
        CcsdsTcPacketOwned::new_with_request(
            SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
            TcHeader::new(target_id, MessageType::Ping),
            (),
        )
        .to_vec()
    }

    /// A structurally valid CCSDS packet (correct length and CRC), but with arbitrary user data
    /// instead of a postcard-encoded [TcHeader].
    fn raw_ccsds_tc_with_user_data(user_data: &[u8]) -> Vec<u8> {
        CcsdsPacketCreatorOwned::new(
            SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
            PacketType::Tc,
            user_data,
            Some(ChecksumType::WithCrc16),
        )
        .unwrap()
        .to_vec()
    }

    #[test]
    fn test_valid_tc_is_routed_without_event() {
        let mut tb = Testbench::new();
        tb.tc_tx
            .send(PacketAsVec::new(
                ComponentId::UdpServer as u32,
                valid_tc_raw(ComponentId::EpsPcdu),
            ))
            .unwrap();
        tb.tc_source.periodic_operation();
        tb.target_rx.try_recv().expect("TC was not routed");
        assert!(matches!(tb.event_rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn test_invalid_ccsds_packet_sends_event() {
        let mut tb = Testbench::new();
        tb.tc_tx
            .send(PacketAsVec::new(
                ComponentId::UdpServer as u32,
                vec![1, 2, 3],
            ))
            .unwrap();
        tb.tc_source.periodic_operation();
        let (sender_id, event) = tb.event_rx.try_recv().expect("expected event");
        assert_eq!(sender_id, ComponentId::UdpServer);
        assert!(matches!(event, tmtc::Event::InvalidTcPacket));
    }

    #[test]
    fn test_invalid_tc_header_sends_event() {
        let mut tb = Testbench::new();
        tb.tc_tx
            .send(PacketAsVec::new(
                ComponentId::UdpServer as u32,
                // A single byte is enough to decode the `ComponentId` discriminant, but not
                // enough for the trailing `MessageType`, so `TcHeader` deserialization fails
                // while the CCSDS packet itself stays valid.
                raw_ccsds_tc_with_user_data(&[0]),
            ))
            .unwrap();
        tb.tc_source.periodic_operation();
        let (sender_id, event) = tb.event_rx.try_recv().expect("expected event");
        assert_eq!(sender_id, ComponentId::UdpServer);
        assert!(matches!(event, tmtc::Event::InvalidTcHeader));
    }

    #[test]
    fn test_unknown_target_id_sends_event() {
        let mut tb = Testbench::new();
        tb.tc_tx
            .send(PacketAsVec::new(
                ComponentId::UdpServer as u32,
                valid_tc_raw(ComponentId::Ground),
            ))
            .unwrap();
        tb.tc_source.periodic_operation();
        let (sender_id, event) = tb.event_rx.try_recv().expect("expected event");
        assert_eq!(sender_id, ComponentId::UdpServer);
        assert!(matches!(
            event,
            tmtc::Event::UnknownTargetId(ComponentId::Ground)
        ));
    }
}
