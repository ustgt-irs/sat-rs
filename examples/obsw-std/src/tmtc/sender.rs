use std::{cell::RefCell, collections::VecDeque, sync::mpsc};

use satrs::{
    ComponentId,
    queue::GenericSendError,
    tmtc::{PacketAsVec, PacketHandler},
};

#[derive(Default, Debug, Clone)]
pub struct MockSender(pub RefCell<VecDeque<PacketAsVec>>);

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum TmTcSender {
    Normal(mpsc::SyncSender<PacketAsVec>),
    Mock(MockSender),
}

impl TmTcSender {
    #[allow(dead_code)]
    pub fn get_mock_sender(&mut self) -> Option<&mut MockSender> {
        match self {
            TmTcSender::Mock(sender) => Some(sender),
            _ => None,
        }
    }
}

impl PacketHandler for TmTcSender {
    type Error = GenericSendError;

    fn handle_packet(&self, sender_id: ComponentId, packet: &[u8]) -> Result<(), Self::Error> {
        match self {
            TmTcSender::Normal(sync_sender) => {
                if let Err(e) = sync_sender.send(PacketAsVec::new(sender_id, packet.to_vec())) {
                    log::error!("Error sending packet via Heap TM/TC sender: {:?}", e);
                }
            }
            TmTcSender::Mock(sender) => {
                sender.handle_packet(sender_id, packet).unwrap();
            }
        }
        Ok(())
    }
}

impl PacketHandler for MockSender {
    type Error = GenericSendError;

    fn handle_packet(&self, sender_id: ComponentId, tc_raw: &[u8]) -> Result<(), Self::Error> {
        let mut mut_queue = self.0.borrow_mut();
        mut_queue.push_back(PacketAsVec::new(sender_id, tc_raw.to_vec()));
        Ok(())
    }
}
