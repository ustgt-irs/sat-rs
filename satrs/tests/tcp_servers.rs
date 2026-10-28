//! This serves as both an integration test and an example application showcasing all major
//! features of the TCP COBS server by performing following steps:
//!
//! 1. It defines both a TC receiver and a TM source which are [Sync].
//! 2. A telemetry packet is inserted into the TM source. The packet will be handled by the
//!    TCP server after handling all TCs.
//! 3. It instantiates the TCP server on localhost with automatic port assignment and assigns
//!    the TC receiver and TM source created previously.
//! 4. It moves the TCP server to a different thread and calls the
//!    [TcpTmtcInCobsServer::handle_next_connection] call inside that thread
//! 5. The main threads connects to the server, sends a test telecommand and then reads back
//!    the test telemetry insertd in to the TM source previously.
use core::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use std::{
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    sync::{Mutex, mpsc},
    thread,
};

use arbitrary_int::{u11, u14};
use hashbrown::HashSet;
use satrs::{
    ComponentId,
    encoding::{
        ccsds::{SpValidity, SpacePacketValidator},
        cobs::encode_packet_with_cobs,
    },
    hal::std::{
        tcp_server::{
            CobsTcParser, ConnectionResult, HandledConnectionHandler, HandledConnectionInfo,
            ServerConfig, TcpSpacepacketsServer, TcpTmtcInCobsServer,
        },
        tcp_spacepackets_server::CcsdsPacketParser,
    },
    tmtc::PacketSource,
};
use spacepackets::{
    CcsdsPacket, PacketId, SpHeader,
    ecss::{CreatorConfig, MessageTypeId, tc::PusTcCreator},
};
use std::{collections::VecDeque, sync::Arc, vec::Vec};

#[derive(Default)]
pub struct ConnectionFinishedHandler {
    connection_info: VecDeque<HandledConnectionInfo>,
}

impl HandledConnectionHandler for ConnectionFinishedHandler {
    fn handled_connection(&mut self, info: HandledConnectionInfo) {
        self.connection_info.push_back(info);
    }
}

impl ConnectionFinishedHandler {
    pub fn check_last_connection(&mut self, num_tms: u32, num_tcs: u32) {
        let last_conn_result = self
            .connection_info
            .pop_back()
            .expect("no connection info available");
        assert_eq!(last_conn_result.num_received_tcs, num_tcs);
        assert_eq!(last_conn_result.num_sent_tms, num_tms);
    }

    pub fn check_no_connections_left(&self) {
        assert!(self.connection_info.is_empty());
    }
}

#[derive(Default, Clone)]
struct SyncTmSource {
    tm_queue: Arc<Mutex<VecDeque<Vec<u8>>>>,
}

impl SyncTmSource {
    pub(crate) fn add_tm(&mut self, tm: &[u8]) {
        let mut tm_queue = self.tm_queue.lock().expect("locking tm queue failec");
        tm_queue.push_back(tm.to_vec());
    }
}

impl PacketSource for SyncTmSource {
    type Error = ();

    fn retrieve_packet(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error> {
        let mut tm_queue = self.tm_queue.lock().expect("locking tm queue failed");
        if !tm_queue.is_empty() {
            let next_vec = tm_queue.front().unwrap();
            if buffer.len() < next_vec.len() {
                panic!(
                    "provided buffer too small, must be at least {} bytes",
                    next_vec.len()
                );
            }
            println!("Sending and encoding TM: {:x?}", next_vec);
            let next_vec = tm_queue.pop_front().unwrap();
            buffer[0..next_vec.len()].copy_from_slice(&next_vec);
            return Ok(next_vec.len());
        }
        Ok(0)
    }
}

const TCP_SERVER_ID: ComponentId = 0x05;
const SIMPLE_PACKET: [u8; 5] = [1, 2, 3, 4, 5];
const INVERTED_PACKET: [u8; 5] = [5, 4, 3, 4, 1];
const AUTO_PORT_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);

#[test]
fn test_cobs_server() {
    let (tc_sender, tc_receiver) = mpsc::channel();
    let mut tm_source = SyncTmSource::default();
    // Insert a telemetry packet which will be read back by the client at a later stage.
    tm_source.add_tm(&INVERTED_PACKET);
    let mut tcp_server = TcpTmtcInCobsServer::new(
        ServerConfig::new(
            TCP_SERVER_ID,
            AUTO_PORT_ADDR,
            Duration::from_millis(2),
            1024,
            1024,
        ),
        tm_source,
        CobsTcParser::new(TCP_SERVER_ID, 1024, tc_sender.clone()),
        ConnectionFinishedHandler::default(),
        None,
    )
    .expect("TCP server generation failed");
    let dest_addr = tcp_server
        .local_addr()
        .expect("retrieving dest addr failed");
    let conn_handled: Arc<AtomicBool> = Default::default();
    let set_if_done = conn_handled.clone();

    // Call the connection handler in separate thread, does block.
    thread::spawn(move || {
        let result = tcp_server.handle_all_connections(Some(Duration::from_millis(400)));
        if result.is_err() {
            panic!("handling connection failed: {:?}", result.unwrap_err());
        }
        let conn_result = result.unwrap();
        assert_eq!(conn_result, ConnectionResult::HandledConnections(1));
        tcp_server
            .generic_server
            .finished_handler
            .check_last_connection(1, 1);
        tcp_server
            .generic_server
            .finished_handler
            .check_no_connections_left();
        // Signal the main thread we are done.
        set_if_done.store(true, Ordering::Relaxed);
    });

    // Send TC to server now.
    let mut encoded_buf: [u8; 16] = [0; 16];
    let mut current_idx = 0;
    encode_packet_with_cobs(&SIMPLE_PACKET, &mut encoded_buf, &mut current_idx);
    let mut stream = TcpStream::connect(dest_addr).expect("connecting to TCP server failed");
    stream
        .write_all(&encoded_buf[..current_idx])
        .expect("writing to TCP server failed");
    // Done with writing.
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("shutting down write failed");
    let mut read_buf: [u8; 16] = [0; 16];
    let read_len = stream.read(&mut read_buf).expect("read failed");
    drop(stream);

    // 1 byte encoding overhead, 2 sentinel bytes.
    assert_eq!(read_len, 8);
    assert_eq!(read_buf[0], 0);
    assert_eq!(read_buf[read_len - 1], 0);
    let decoded_len =
        cobs::decode_in_place(&mut read_buf[1..read_len]).expect("COBS decoding failed");
    assert_eq!(decoded_len, 5);
    // Skip first sentinel byte.
    assert_eq!(&read_buf[1..1 + INVERTED_PACKET.len()], &INVERTED_PACKET);
    // A certain amount of time is allowed for the transaction to complete.
    for _ in 0..3 {
        if !conn_handled.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(5));
        }
    }
    if !conn_handled.load(Ordering::Relaxed) {
        panic!("connection was not handled properly");
    }
    // Check that the packet was received and decoded successfully.
    let tc_with_sender = tc_receiver.try_recv().expect("no TC received");
    assert_eq!(tc_with_sender.packet, SIMPLE_PACKET);
    assert_eq!(tc_with_sender.sender_id, TCP_SERVER_ID);
    matches!(tc_receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
}

const TEST_APID_0: u11 = u11::new(0x02);
const TEST_PACKET_ID_0: PacketId = PacketId::new_for_tc(true, TEST_APID_0);

#[derive(Default)]
pub struct SimpleVerificator {
    pub valid_ids: HashSet<PacketId>,
}

impl SpacePacketValidator for SimpleVerificator {
    fn validate(
        &self,
        sp_header: &SpHeader,
        _raw_buf: &[u8],
    ) -> satrs::encoding::ccsds::SpValidity {
        if self.valid_ids.contains(&sp_header.packet_id()) {
            return SpValidity::Valid;
        }
        SpValidity::Skip
    }
}

#[test]
fn test_ccsds_server() {
    let (tc_sender, tc_receiver) = mpsc::channel();
    let mut tm_source = SyncTmSource::default();
    let sph = SpHeader::new_for_unseg_tc(TEST_APID_0, u14::new(0), 0);
    let verif_tm =
        PusTcCreator::new_simple(sph, MessageTypeId::new(1, 1), &[], CreatorConfig::default());
    let tm_0 = verif_tm.to_vec().expect("tm generation failed");
    tm_source.add_tm(&tm_0);
    let mut packet_id_lookup = SimpleVerificator::default();
    packet_id_lookup.valid_ids.insert(TEST_PACKET_ID_0);
    let mut tcp_server = TcpSpacepacketsServer::new(
        ServerConfig::new(
            TCP_SERVER_ID,
            AUTO_PORT_ADDR,
            Duration::from_millis(2),
            1024,
            1024,
        ),
        tm_source,
        CcsdsPacketParser::new(TCP_SERVER_ID, 1024, tc_sender, packet_id_lookup),
        ConnectionFinishedHandler::default(),
        None,
    )
    .expect("TCP server generation failed");
    let dest_addr = tcp_server
        .local_addr()
        .expect("retrieving dest addr failed");
    let conn_handled: Arc<AtomicBool> = Default::default();
    let set_if_done = conn_handled.clone();
    // Call the connection handler in separate thread, does block.
    thread::spawn(move || {
        let result = tcp_server.handle_all_connections(Some(Duration::from_millis(500)));
        if result.is_err() {
            panic!("handling connection failed: {:?}", result.unwrap_err());
        }
        let conn_result = result.unwrap();
        assert_eq!(conn_result, ConnectionResult::HandledConnections(1));
        tcp_server
            .generic_server
            .finished_handler
            .check_last_connection(1, 1);
        tcp_server
            .generic_server
            .finished_handler
            .check_no_connections_left();
        set_if_done.store(true, Ordering::Relaxed);
    });
    let mut stream = TcpStream::connect(dest_addr).expect("connecting to TCP server failed");
    stream
        .set_read_timeout(Some(Duration::from_millis(10)))
        .expect("setting reas timeout failed");

    // Send ping telecommand.
    let sph = SpHeader::new_for_unseg_tc(TEST_APID_0, u14::new(0), 0);
    let ping_tc = PusTcCreator::new_simple(
        sph,
        MessageTypeId::new(17, 1),
        &[],
        CreatorConfig::default(),
    );
    let tc_0 = ping_tc.to_vec().expect("packet creation failed");
    stream
        .write_all(&tc_0)
        .expect("writing to TCP server failed");
    // Done with writing.
    stream
        .shutdown(std::net::Shutdown::Write)
        .expect("shutting down write failed");

    // Now read all the telemetry from the server.
    let mut read_buf: [u8; 16] = [0; 16];
    let mut read_len_total = 0;
    // Timeout ensures this does not block forever.
    while read_len_total < tm_0.len() {
        let read_len = stream.read(&mut read_buf).expect("read failed");
        read_len_total += read_len;
        assert_eq!(read_buf[..read_len], tm_0);
    }
    drop(stream);

    // A certain amount of time is allowed for the transaction to complete.
    for _ in 0..3 {
        if !conn_handled.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(5));
        }
    }
    if !conn_handled.load(Ordering::Relaxed) {
        panic!("connection was not handled properly");
    }
    // Check that TC has arrived.
    let tc_with_sender = tc_receiver.try_recv().expect("no TC received");
    assert_eq!(tc_with_sender.packet, tc_0);
    assert_eq!(tc_with_sender.sender_id, TCP_SERVER_ID);
    matches!(tc_receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
}
