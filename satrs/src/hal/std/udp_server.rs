//! Generic UDP TC server.
use crate::ComponentId;
use crate::tmtc::PacketHandler;
use core::fmt::Debug;
use std::io::{self, ErrorKind};
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::vec;
use std::vec::Vec;

/// This UDP server can be used to receive CCSDS space packet telecommands or any other telecommand
/// format.
///
/// It caches all received telecomands into a vector. The maximum expected telecommand size should
/// be declared upfront. This avoids dynamic allocation during run-time. The user can specify a TC
/// sender in form of a special trait object which implements [PacketHandler]. For example, this
/// can be used to send the telecommands to a centralized TC source component for further
/// processing and routing.
///
/// # Examples
///
/// ```
/// use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
/// use std::sync::mpsc;
/// use spacepackets::ecss::WritablePusPacket;
/// use satrs::hal::std::udp_server::UdpTcServer;
/// use satrs::ComponentId;
/// use satrs::tmtc::PacketHandler;
/// use spacepackets::SpHeader;
/// use spacepackets::ecss::tc::{MessageTypeId, PusTcCreator, CreatorConfig};
/// use arbitrary_int::u11;
///
/// const UDP_SERVER_ID: ComponentId = 0x05;
///
/// let (packet_sender, packet_receiver) = mpsc::channel();
/// let dest_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7777);
/// let mut udp_tc_server = UdpTcServer::new(UDP_SERVER_ID, dest_addr, 2048, packet_sender)
///       .expect("Creating UDP TMTC server failed");
/// let sph = SpHeader::new_from_apid(u11::new(0x02));
/// let pus_tc = PusTcCreator::new_simple(sph, MessageTypeId::new(17, 1), &[], CreatorConfig::default());
/// // Can not fail.
/// let ping_tc_raw = pus_tc.to_vec().unwrap();
///
/// // Now create a UDP client and send the ping telecommand to the server.
/// let client = UdpSocket::bind("127.0.0.1:0").expect("creating UDP client failed");
/// client
///     .send_to(&ping_tc_raw, dest_addr)
///     .expect("Error sending PUS TC via UDP");
/// let recv_result = udp_tc_server.try_recv_tc();
/// assert!(recv_result.is_ok());
/// // The packet is received by the UDP TC server and sent via the mpsc channel.
/// let sent_packet_with_sender = packet_receiver.try_recv().expect("expected telecommand");
/// assert_eq!(sent_packet_with_sender.packet, ping_tc_raw);
/// assert_eq!(sent_packet_with_sender.sender_id, UDP_SERVER_ID);
/// // No more packets received.
/// matches!(packet_receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
/// ```
///
/// The [satrs-example crate](https://egit.irs.uni-stuttgart.de/rust/sat-rs/src/branch/main/examples/obsw-std)
/// server code also includes
/// [example code](https://egit.irs.uni-stuttgart.de/rust/sat-rs/src/branch/main/examples/obsw-std/src/tmtc.rs#L67)
/// on how to use this TC server. It uses the server to receive PUS telecommands on a specific port
/// and then forwards them to a generic CCSDS packet receiver.
pub struct UdpTcServer<TcSender: PacketHandler<Error = SendError>, SendError> {
    pub id: ComponentId,
    pub socket: UdpSocket,
    recv_buf: Vec<u8>,
    sender_addr: Option<SocketAddr>,
    pub tc_sender: TcSender,
}

#[derive(Debug, thiserror::Error)]
pub enum ReceiveResult<SendError: Debug + 'static> {
    #[error("nothing was received")]
    NothingReceived,
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Send(SendError),
}

impl<TcSender: PacketHandler<Error = SendError>, SendError: Debug + 'static>
    UdpTcServer<TcSender, SendError>
{
    pub fn new<A: ToSocketAddrs>(
        id: ComponentId,
        addr: A,
        max_recv_size: usize,
        tc_sender: TcSender,
    ) -> Result<Self, io::Error> {
        let server = Self {
            id,
            socket: UdpSocket::bind(addr)?,
            recv_buf: vec![0; max_recv_size],
            sender_addr: None,
            tc_sender,
        };
        server.socket.set_nonblocking(true)?;
        Ok(server)
    }

    pub fn try_recv_tc(&mut self) -> Result<(usize, SocketAddr), ReceiveResult<SendError>> {
        let res = match self.socket.recv_from(&mut self.recv_buf) {
            Ok(res) => res,
            Err(e) => {
                return if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut {
                    Err(ReceiveResult::NothingReceived)
                } else {
                    Err(e.into())
                };
            }
        };
        let (num_bytes, from) = res;
        self.sender_addr = Some(from);
        self.tc_sender
            .handle_packet(self.id, &self.recv_buf[0..num_bytes])
            .map_err(ReceiveResult::Send)?;
        Ok(res)
    }

    pub fn last_sender(&self) -> Option<SocketAddr> {
        self.sender_addr
    }
}

#[cfg(test)]
mod tests {
    use crate::ComponentId;
    use crate::hal::std::udp_server::{ReceiveResult, UdpTcServer};
    use crate::queue::GenericSendError;
    use crate::tmtc::PacketHandler;
    use arbitrary_int::u11;
    use core::cell::RefCell;
    use spacepackets::SpHeader;
    use spacepackets::ecss::tc::PusTcCreator;
    use spacepackets::ecss::{CreatorConfig, MessageTypeId};
    use std::collections::VecDeque;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
    use std::vec::Vec;

    fn is_send<T: Send>(_: &T) {}

    const UDP_SERVER_ID: ComponentId = 0x05;

    #[derive(Default)]
    struct PingReceiver {
        pub sent_cmds: RefCell<VecDeque<Vec<u8>>>,
    }

    impl PacketHandler for PingReceiver {
        type Error = GenericSendError;

        fn handle_packet(&self, sender_id: ComponentId, tc_raw: &[u8]) -> Result<(), Self::Error> {
            assert_eq!(sender_id, UDP_SERVER_ID);
            let mut sent_data = Vec::new();
            sent_data.extend_from_slice(tc_raw);
            let mut queue = self.sent_cmds.borrow_mut();
            queue.push_back(sent_data);
            Ok(())
        }
    }

    #[test]
    fn basic_test() {
        let mut buf = [0; 32];
        let dest_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7777);
        let ping_receiver = PingReceiver::default();
        is_send(&ping_receiver);
        let mut udp_tc_server = UdpTcServer::new(UDP_SERVER_ID, dest_addr, 2048, ping_receiver)
            .expect("Creating UDP TMTC server failed");
        is_send(&udp_tc_server);
        let sph = SpHeader::new_from_apid(u11::new(0x02));
        let pus_tc = PusTcCreator::new_simple(
            sph,
            MessageTypeId::new(17, 1),
            &[],
            CreatorConfig::default(),
        );
        let len = pus_tc
            .write_to_bytes(&mut buf)
            .expect("Error writing PUS TC packet");
        let client = UdpSocket::bind("127.0.0.1:7778").expect("Connecting to UDP server failed");
        client
            .send_to(&buf[0..len], dest_addr)
            .expect("Error sending PUS TC via UDP");
        let local_addr = client.local_addr().unwrap();
        udp_tc_server
            .try_recv_tc()
            .expect("Error receiving sent telecommand");
        assert_eq!(
            udp_tc_server.last_sender().expect("No sender set"),
            local_addr
        );
        let ping_receiver = &mut udp_tc_server.tc_sender;
        let mut queue = ping_receiver.sent_cmds.borrow_mut();
        assert_eq!(queue.len(), 1);
        let sent_cmd = queue.pop_front().unwrap();
        assert_eq!(sent_cmd, buf[0..len]);
    }

    #[test]
    fn test_nothing_received() {
        let dest_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 7779);
        let ping_receiver = PingReceiver::default();
        let mut udp_tc_server = UdpTcServer::new(UDP_SERVER_ID, dest_addr, 2048, ping_receiver)
            .expect("Creating UDP TMTC server failed");
        let res = udp_tc_server.try_recv_tc();
        assert!(res.is_err());
        let err = res.unwrap_err();
        matches!(err, ReceiveResult::NothingReceived);
    }
}
