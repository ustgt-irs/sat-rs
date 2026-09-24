#![allow(dead_code)]
use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::{Arc, Mutex, mpsc};

use log::warn;
use satrs::HandlingStatus;
use satrs::hal::std::udp_server::{ReceiveResult, UdpTcServer};
use satrs::queue::GenericSendError;
use types::ccsds::CcsdsTmPacketOwned;

use crate::tmtc::sender::TmTcSender;

pub trait UdpTmHandlerProvider {
    fn send_tm_to_udp_client(&mut self, socket: &UdpSocket, recv_addr: &SocketAddr);
}

pub struct UdpTmHandlerWithChannel {
    pub tm_rx: mpsc::Receiver<CcsdsTmPacketOwned>,
}

impl UdpTmHandlerProvider for UdpTmHandlerWithChannel {
    fn send_tm_to_udp_client(&mut self, socket: &UdpSocket, recv_addr: &SocketAddr) {
        while let Ok(tm) = self.tm_rx.try_recv() {
            log::debug!("sending TM from sender {:?}", tm.tm_header.sender_id);
            let result = socket.send_to(&tm.to_vec(), recv_addr);
            if let Err(e) = result {
                warn!("Sending TM with UDP socket failed: {e}")
            }
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct TestTmHandler {
    addrs_to_send_to: Arc<Mutex<VecDeque<SocketAddr>>>,
}

impl UdpTmHandlerProvider for TestTmHandler {
    fn send_tm_to_udp_client(&mut self, _socket: &UdpSocket, recv_addr: &SocketAddr) {
        self.addrs_to_send_to.lock().unwrap().push_back(*recv_addr);
    }
}

pub enum UdpTmHandler {
    Normal(UdpTmHandlerWithChannel),
    Test(TestTmHandler),
}

impl From<UdpTmHandlerWithChannel> for UdpTmHandler {
    fn from(handler: UdpTmHandlerWithChannel) -> Self {
        UdpTmHandler::Normal(handler)
    }
}

impl From<TestTmHandler> for UdpTmHandler {
    fn from(handler: TestTmHandler) -> Self {
        UdpTmHandler::Test(handler)
    }
}

impl UdpTmHandlerProvider for UdpTmHandler {
    fn send_tm_to_udp_client(&mut self, socket: &UdpSocket, recv_addr: &SocketAddr) {
        match self {
            UdpTmHandler::Normal(handler) => handler.send_tm_to_udp_client(socket, recv_addr),
            UdpTmHandler::Test(handler) => handler.send_tm_to_udp_client(socket, recv_addr),
        }
    }
}

pub struct UdpTmtcServer {
    pub udp_tc_server: UdpTcServer<TmTcSender, GenericSendError>,
    pub tm_handler: UdpTmHandler,
}

impl UdpTmtcServer {
    pub fn periodic_operation(&mut self) {
        loop {
            if self.poll_tc_server() == HandlingStatus::Empty {
                break;
            }
        }
        if let Some(recv_addr) = self.udp_tc_server.last_sender() {
            self.tm_handler
                .send_tm_to_udp_client(&self.udp_tc_server.socket, &recv_addr);
        }
    }

    fn poll_tc_server(&mut self) -> HandlingStatus {
        match self.udp_tc_server.try_recv_tc() {
            Ok(_) => HandlingStatus::HandledOne,
            Err(e) => {
                match e {
                    ReceiveResult::NothingReceived => (),
                    ReceiveResult::Io(e) => {
                        warn!("IO error {e}");
                    }
                    ReceiveResult::Send(send_error) => {
                        warn!("send error {send_error:?}");
                    }
                }
                HandlingStatus::Empty
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::net::Ipv4Addr;

    use arbitrary_int::traits::Integer as _;
    use arbitrary_int::u14;
    use satrs::spacepackets::ecss::{CreatorConfig, MessageTypeId};
    use satrs::{
        ComponentId,
        spacepackets::{
            SpHeader,
            ecss::{WritablePusPacket, tc::PusTcCreator},
        },
    };
    use satrs_example::config::OBSW_SERVER_ADDR;
    use types::Apid;

    use crate::tmtc::sender::MockSender;

    use super::*;

    const UDP_SERVER_ID: ComponentId = 0x05;

    #[test]
    fn test_basic() {
        let sock_addr = SocketAddr::new(IpAddr::V4(OBSW_SERVER_ADDR), 0);
        let test_receiver = TmTcSender::Mock(MockSender::default());
        let udp_tc_server =
            UdpTcServer::new(UDP_SERVER_ID, sock_addr, 2048, test_receiver).unwrap();
        let tm_handler = TestTmHandler::default();
        let tm_handler_calls = tm_handler.addrs_to_send_to.clone();
        let mut udp_dyn_server = UdpTmtcServer {
            udp_tc_server,
            tm_handler: tm_handler.into(),
        };
        udp_dyn_server.periodic_operation();
        let queue = udp_dyn_server
            .udp_tc_server
            .tc_sender
            .get_mock_sender()
            .unwrap()
            .0
            .borrow();
        assert!(queue.is_empty());
        assert!(tm_handler_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn test_transactions() {
        let sock_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let test_receiver = TmTcSender::Mock(MockSender::default());
        let udp_tc_server =
            UdpTcServer::new(UDP_SERVER_ID, sock_addr, 2048, test_receiver).unwrap();
        let server_addr = udp_tc_server.socket.local_addr().unwrap();
        let tm_handler = TestTmHandler::default();
        let tm_handler_calls = tm_handler.addrs_to_send_to.clone();
        let mut udp_dyn_server = UdpTmtcServer {
            udp_tc_server,
            tm_handler: tm_handler.into(),
        };
        let sph = SpHeader::new_for_unseg_tc(Apid::Tmtc.raw_value(), u14::ZERO, 0);
        let ping_tc = PusTcCreator::new_simple(
            sph,
            MessageTypeId::new(17, 1),
            &[],
            CreatorConfig::default(),
        )
        .to_vec()
        .unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").expect("Connecting to UDP server failed");
        let client_addr = client.local_addr().unwrap();
        println!("{}", server_addr);
        client.send_to(&ping_tc, server_addr).unwrap();
        udp_dyn_server.periodic_operation();
        {
            let mut queue = udp_dyn_server
                .udp_tc_server
                .tc_sender
                .get_mock_sender()
                .unwrap()
                .0
                .borrow_mut();
            assert!(!queue.is_empty());
            let packet_with_sender = queue.pop_front().unwrap();
            assert_eq!(packet_with_sender.packet, ping_tc);
            assert_eq!(packet_with_sender.sender_id, UDP_SERVER_ID);
        }

        {
            let mut tm_handler_calls = tm_handler_calls.lock().unwrap();
            assert!(!tm_handler_calls.is_empty());
            assert_eq!(tm_handler_calls.len(), 1);
            let received_addr = tm_handler_calls.pop_front().unwrap();
            assert_eq!(received_addr, client_addr);
        }
        udp_dyn_server.periodic_operation();
        let queue = udp_dyn_server
            .udp_tc_server
            .tc_sender
            .get_mock_sender()
            .unwrap()
            .0
            .borrow();
        assert!(queue.is_empty());
        drop(queue);
        // Still tries to send to the same client.
        {
            let mut tm_handler_calls = tm_handler_calls.lock().unwrap();
            assert!(!tm_handler_calls.is_empty());
            assert_eq!(tm_handler_calls.len(), 1);
            let received_addr = tm_handler_calls.pop_front().unwrap();
            assert_eq!(received_addr, client_addr);
        }
    }
}
