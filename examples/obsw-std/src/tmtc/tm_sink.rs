use std::{
    collections::HashMap,
    sync::mpsc::{self},
};

use arbitrary_int::{u11, u14};
use satrs::spacepackets::seq_count::{SequenceCounter, SequenceCounterCcsdsSimple};
use types::ccsds::CcsdsTmPacketOwned;

use crate::interface::tcp::SyncTcpTmSource;

#[derive(Default)]
pub struct CcsdsSeqCounterMap {
    apid_seq_counter_map: HashMap<u11, SequenceCounterCcsdsSimple>,
}
impl CcsdsSeqCounterMap {
    pub fn get_and_increment(&mut self, apid: u11) -> u14 {
        u14::new(
            self.apid_seq_counter_map
                .entry(apid)
                .or_default()
                .get_and_increment(),
        )
    }
}

pub struct TmSink {
    seq_counter_map: CcsdsSeqCounterMap,
    sync_tm_tcp_source: SyncTcpTmSource,
    tm_funnel_rx: mpsc::Receiver<CcsdsTmPacketOwned>,
    tm_server_tx: mpsc::SyncSender<CcsdsTmPacketOwned>,
}

impl TmSink {
    pub fn new(
        sync_tm_tcp_source: SyncTcpTmSource,
        tm_funnel_rx: mpsc::Receiver<CcsdsTmPacketOwned>,
        tm_server_tx: mpsc::SyncSender<CcsdsTmPacketOwned>,
    ) -> Self {
        Self {
            seq_counter_map: Default::default(),
            sync_tm_tcp_source,
            tm_funnel_rx,
            tm_server_tx,
        }
    }

    pub fn operation(&mut self) {
        if let Ok(mut tm) = self.tm_funnel_rx.try_recv() {
            tm.sp_header
                .set_seq_count(self.seq_counter_map.get_and_increment(tm.sp_header.apid()));
            self.sync_tm_tcp_source.add_tm(&tm.to_vec());
            self.tm_server_tx
                .send(tm)
                .expect("Sending TM to server failed");
        }
    }
}
