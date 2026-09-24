use satrs::spacepackets::CcsdsPacketIdAndPsc;
use types::{
    ComponentId,
    ccsds::{CcsdsTcPacketOwned, CcsdsTmPacketOwned},
    control,
};

use crate::ccsds::pack_ccsds_tm_packet_for_now;

pub struct Controller {
    pub tc_rx: std::sync::mpsc::Receiver<CcsdsTcPacketOwned>,
    pub tm_tx: std::sync::mpsc::SyncSender<CcsdsTmPacketOwned>,
    pub event_ctrl_tx: std::sync::mpsc::SyncSender<control::Event>,
}

impl Controller {
    pub fn new(
        tc_rx: std::sync::mpsc::Receiver<CcsdsTcPacketOwned>,
        tm_tx: std::sync::mpsc::SyncSender<CcsdsTmPacketOwned>,
        event_ctrl_tx: std::sync::mpsc::SyncSender<control::Event>,
    ) -> Self {
        Self {
            tc_rx,
            tm_tx,
            event_ctrl_tx,
        }
    }

    pub fn periodic_operation(&mut self) {
        self.handle_telecommands();
    }

    pub fn handle_telecommands(&mut self) {
        loop {
            match self.tc_rx.try_recv() {
                Ok(packet) => {
                    let tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&packet.sp_header);
                    match postcard::from_bytes::<control::request::Request>(&packet.payload) {
                        Ok(request) => {
                            log::info!(
                                "received request {:?} with TC ID {:#010x}",
                                request,
                                tc_id.raw()
                            );
                            match request {
                                control::request::Request::Ping => self
                                    .send_telemetry(Some(tc_id), control::response::Response::Ok),
                                control::request::Request::TestEvent => {
                                    self.event_ctrl_tx.send(control::Event::TestEvent).unwrap();
                                    self.send_telemetry(
                                        Some(tc_id),
                                        control::response::Response::Ok,
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("failed to deserialize request: {}", e);
                        }
                    }
                }
                Err(e) => match e {
                    std::sync::mpsc::TryRecvError::Empty => break,
                    std::sync::mpsc::TryRecvError::Disconnected => {
                        log::warn!("packet sender disconnected")
                    }
                },
            }
        }
    }

    pub fn send_telemetry(
        &self,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        response: control::response::Response,
    ) {
        match pack_ccsds_tm_packet_for_now(ComponentId::Controller, tc_id, &response) {
            Ok(packet) => {
                if let Err(e) = self.tm_tx.send(packet) {
                    log::warn!("failed to send TM packet: {}", e);
                }
            }
            Err(e) => {
                log::warn!("failed to pack TM packet: {}", e);
            }
        }
    }
}
