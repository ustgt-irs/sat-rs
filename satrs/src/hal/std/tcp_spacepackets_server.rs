use alloc::sync::Arc;
use core::{sync::atomic::AtomicBool, time::Duration};
use delegate::delegate;
use mio::net::{TcpListener, TcpStream};
use std::{io::Write, net::SocketAddr};

use crate::{
    ComponentId,
    encoding::{ccsds::SpacePacketValidator, parse_buffer_for_ccsds_space_packets},
    queue::GenericSendError,
    tmtc::{PacketHandler, PacketSource},
};

use super::tcp_server::{
    ConnectionResult, HandledConnectionHandler, HandledConnectionInfo, ServerConfig, TcpTcParser,
    TcpTmSender, TcpTmtcGenericServer,
};

pub struct CcsdsPacketParser<
    PacketValidator: SpacePacketValidator,
    PacketHandlerInstance: PacketHandler,
> {
    sender_id: ComponentId,
    parsing_buffer: alloc::vec::Vec<u8>,
    validator: PacketValidator,
    packet_handler: PacketHandlerInstance,
    current_write_index: usize,
}

impl<PacketValidator: SpacePacketValidator, PacketHandlerInstance: PacketHandler>
    CcsdsPacketParser<PacketValidator, PacketHandlerInstance>
{
    pub fn new(
        sender_id: ComponentId,
        parsing_buf_size: usize,
        packet_handler: PacketHandlerInstance,
        validator: PacketValidator,
    ) -> Self {
        Self {
            sender_id,
            parsing_buffer: alloc::vec![0; parsing_buf_size],
            validator,
            packet_handler,
            current_write_index: 0,
        }
    }

    fn write_to_buffer(&mut self, data: &[u8]) -> usize {
        let available = self.parsing_buffer.len() - self.current_write_index;
        let to_write = core::cmp::min(data.len(), available);

        self.parsing_buffer[self.current_write_index..self.current_write_index + to_write]
            .copy_from_slice(&data[..to_write]);
        self.current_write_index += to_write;

        to_write
    }

    fn parse_and_handle_packets(&mut self) -> u32 {
        match parse_buffer_for_ccsds_space_packets(
            &self.parsing_buffer[..self.current_write_index],
            &self.validator,
            self.sender_id,
            &self.packet_handler,
        ) {
            Ok(parse_result) => {
                self.parsing_buffer
                    .copy_within(parse_result.parsed_bytes..self.current_write_index, 0);
                self.current_write_index -= parse_result.parsed_bytes;
                parse_result.packets_found
            }
            Err(_) => 0,
        }
    }

    fn drop_first_half_of_buffer(&mut self) {
        let mid = self.parsing_buffer.len() / 2;
        self.parsing_buffer.copy_within(mid.., 0);
        self.current_write_index -= mid;
    }
}
impl<PacketValidator: SpacePacketValidator, PacketHandlerInstance: PacketHandler> TcpTcParser
    for CcsdsPacketParser<PacketValidator, PacketHandlerInstance>
{
    fn reset(&mut self) {
        self.current_write_index = 0;
    }

    fn push(&mut self, mut tc_buffer: &[u8], conn_result: &mut HandledConnectionInfo) {
        while !tc_buffer.is_empty() {
            // Write as much as possible to buffer
            let written = self.write_to_buffer(tc_buffer);
            tc_buffer = &tc_buffer[written..];

            // Parse for complete packets
            let packets_found = self.parse_and_handle_packets();
            conn_result.num_received_tcs += packets_found;

            if tc_buffer.is_empty() {
                break;
            }

            // Handle buffer overflow
            if self.current_write_index == self.parsing_buffer.len() {
                self.drop_first_half_of_buffer();
            }
        }
    }
}

/// Concrete [TcpTmSender] implementation for the [TcpSpacepacketsServer].
#[derive(Default)]
pub struct SpacepacketsTmSender {}

impl TcpTmSender for SpacepacketsTmSender {
    fn handle_tm_sending(
        &mut self,
        tm_buffer: &mut [u8],
        tm_source: &mut (impl PacketSource<Error = ()> + ?Sized),
        conn_result: &mut HandledConnectionInfo,
        stream: &mut TcpStream,
    ) -> Result<bool, std::io::Error> {
        let mut tm_was_sent = false;
        while let Ok(read_tm_len) = tm_source.retrieve_packet(tm_buffer) {
            // Write TM until TM source is exhausted. For now, there is no limit for the amount
            // of TM written this way.

            if read_tm_len == 0 {
                return Ok(tm_was_sent);
            }
            tm_was_sent = true;
            conn_result.num_sent_tms += 1;

            stream.write_all(&tm_buffer[..read_tm_len])?;
        }
        Ok(tm_was_sent)
    }
}

/// TCP TMTC server implementation for exchange of tightly stuffed
/// [CCSDS space packets](https://public.ccsds.org/Pubs/133x0b2e1.pdf).
///
/// This serves only works if
/// [CCSDS 133.0-B-2 space packets](https://public.ccsds.org/Pubs/133x0b2e1.pdf) are the only
/// packet type being exchanged. It uses the CCSDS space packet header [spacepackets::SpHeader] and
/// a user specified [SpacePacketValidator] to determine the space packets relevant for further
/// processing.
///
/// ## Example
///
/// The [TCP server integration tests](https://github.com/ustgt-irs/sat-rs/blob/main/satrs/tests/tcp_servers.rs)
/// also serves as the example application for this module.
pub struct TcpSpacepacketsServer<
    TmSource: PacketSource<Error = ()>,
    TcSender: PacketHandler<Error = GenericSendError>,
    Validator: SpacePacketValidator,
    HandledConnection: HandledConnectionHandler,
> {
    pub generic_server: TcpTmtcGenericServer<
        TmSource,
        SpacepacketsTmSender,
        CcsdsPacketParser<Validator, TcSender>,
        HandledConnection,
    >,
}

impl<
    TmSource: PacketSource<Error = ()>,
    TcSender: PacketHandler<Error = GenericSendError>,
    Validator: SpacePacketValidator,
    HandledConnection: HandledConnectionHandler,
> TcpSpacepacketsServer<TmSource, TcSender, Validator, HandledConnection>
{
    ///
    /// ## Parameter
    ///
    /// * `cfg` - Configuration of the server.
    /// * `tm_source` - Generic TM source used by the server to pull telemetry packets which are
    ///   then sent back to the client.
    /// * `tc_sender` - Any received telecommands which were decoded successfully will be
    ///   forwarded using this [PacketHandler].
    /// * `validator` - Used to determine the space packets relevant for further processing and
    ///   to detect broken space packets.
    /// * `handled_connection_hook` - Called to notify the user about a succesfully handled
    ///   connection.
    /// * `stop_signal` - Can be used to shut down the TCP server even for longer running
    ///   connections.
    pub fn new(
        cfg: ServerConfig,
        tm_source: TmSource,
        tc_parser: CcsdsPacketParser<Validator, TcSender>,
        handled_connection_hook: HandledConnection,
        stop_signal: Option<Arc<AtomicBool>>,
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            generic_server: TcpTmtcGenericServer::new(
                cfg,
                tc_parser,
                SpacepacketsTmSender::default(),
                tm_source,
                handled_connection_hook,
                stop_signal,
            )?,
        })
    }

    delegate! {
        to self.generic_server {
            pub fn listener(&mut self) -> &mut TcpListener;

            /// Can be used to retrieve the local assigned address of the TCP server. This is especially
            /// useful if using the port number 0 for OS auto-assignment.
            pub fn local_addr(&self) -> std::io::Result<SocketAddr>;

            /// Delegation to the [TcpTmtcGenericServer::handle_all_connections] call.
            pub fn handle_all_connections(
                &mut self,
                poll_timeout: Option<Duration>
            ) -> Result<ConnectionResult, std::io::Error>;
        }
    }
}

#[cfg(test)]
mod tests {
    use core::{
        sync::atomic::{AtomicBool, Ordering},
        time::Duration,
    };
    #[allow(unused_imports)]
    use std::println;
    use std::{
        io::{Read, Write},
        net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
        sync::mpsc,
        thread,
    };

    use alloc::sync::Arc;
    use arbitrary_int::u11;
    use hashbrown::HashSet;
    use spacepackets::{
        CcsdsPacket, PacketId, SpHeader,
        ecss::{CreatorConfig, MessageTypeId, tc::PusTcCreator},
    };

    use crate::{
        ComponentId,
        encoding::ccsds::{SpValidity, SpacePacketValidator},
        hal::std::tcp_server::{
            ConnectionResult, ServerConfig,
            tests::{ConnectionFinishedHandler, SyncTmSource},
        },
        tmtc::PacketAsVec,
    };

    use super::TcpSpacepacketsServer;

    const TCP_SERVER_ID: ComponentId = 0x05;
    const TEST_APID_0: u11 = u11::new(0x02);
    const TEST_PACKET_ID_0: PacketId = PacketId::new_for_tc(true, TEST_APID_0);
    const TEST_APID_1: u11 = u11::new(0x10);
    const TEST_PACKET_ID_1: PacketId = PacketId::new_for_tc(true, TEST_APID_1);

    #[derive(Default)]
    pub struct SimpleValidator(pub HashSet<PacketId>);

    impl SpacePacketValidator for SimpleValidator {
        fn validate(&self, sp_header: &SpHeader, _raw_buf: &[u8]) -> SpValidity {
            if self.0.contains(&sp_header.packet_id()) {
                return SpValidity::Valid;
            }
            // Simple case: Assume that the interface always contains valid space packets.
            SpValidity::Skip
        }
    }

    fn generic_tmtc_server(
        addr: &SocketAddr,
        tc_parser: super::CcsdsPacketParser<SimpleValidator, mpsc::Sender<PacketAsVec>>,
        tm_source: SyncTmSource,
        stop_signal: Option<Arc<AtomicBool>>,
    ) -> TcpSpacepacketsServer<
        SyncTmSource,
        mpsc::Sender<PacketAsVec>,
        SimpleValidator,
        ConnectionFinishedHandler,
    > {
        TcpSpacepacketsServer::new(
            ServerConfig::new(TCP_SERVER_ID, *addr, Duration::from_millis(2), 1024, 1024),
            tm_source,
            tc_parser,
            ConnectionFinishedHandler::default(),
            stop_signal,
        )
        .expect("TCP server generation failed")
    }

    #[test]
    fn test_basic_tc_only() {
        let auto_port_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let (tc_sender, tc_receiver) = mpsc::channel();
        let tm_source = SyncTmSource::default();
        let mut validator = SimpleValidator::default();
        validator.0.insert(TEST_PACKET_ID_0);
        let mut tcp_server = generic_tmtc_server(
            &auto_port_addr,
            super::CcsdsPacketParser::new(TCP_SERVER_ID, 1024, tc_sender.clone(), validator),
            tm_source,
            None,
        );
        let dest_addr = tcp_server
            .local_addr()
            .expect("retrieving dest addr failed");
        let conn_handled: Arc<AtomicBool> = Default::default();
        let set_if_done = conn_handled.clone();
        // Call the connection handler in separate thread, does block.
        thread::spawn(move || {
            let result = tcp_server.handle_all_connections(Some(Duration::from_millis(100)));
            if result.is_err() {
                panic!("handling connection failed: {:?}", result.unwrap_err());
            }
            let conn_result = result.unwrap();
            matches!(conn_result, ConnectionResult::HandledConnections(1));
            tcp_server
                .generic_server
                .finished_handler
                .check_last_connection(0, 1);
            tcp_server
                .generic_server
                .finished_handler
                .check_no_connections_left();
            set_if_done.store(true, Ordering::Relaxed);
        });
        let ping_tc = PusTcCreator::new_simple(
            SpHeader::new_from_apid(TEST_APID_0),
            MessageTypeId::new(17, 1),
            &[],
            CreatorConfig::default(),
        );
        let tc_0 = ping_tc.to_vec().expect("packet generation failed");
        let mut stream = TcpStream::connect(dest_addr).expect("connecting to TCP server failed");
        stream
            .write_all(&tc_0)
            .expect("writing to TCP server failed");
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
        let packet = tc_receiver.try_recv().expect("receiving TC failed");
        assert_eq!(packet.packet, tc_0);
        matches!(tc_receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
    }

    #[test]
    fn test_multi_tc_multi_tm() {
        let auto_port_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let (tc_sender, tc_receiver) = mpsc::channel();
        let mut tm_source = SyncTmSource::default();

        // Add telemetry
        let mut total_tm_len = 0;
        let verif_tm = PusTcCreator::new_simple(
            SpHeader::new_from_apid(TEST_APID_0),
            MessageTypeId::new(1, 1),
            &[],
            CreatorConfig::default(),
        );
        let tm_0 = verif_tm.to_vec().expect("writing packet failed");
        total_tm_len += tm_0.len();
        tm_source.add_tm(&tm_0);
        let verif_tm = PusTcCreator::new_simple(
            SpHeader::new_from_apid(TEST_APID_1),
            MessageTypeId::new(1, 3),
            &[],
            CreatorConfig::default(),
        );
        let tm_1 = verif_tm.to_vec().expect("writing packet failed");
        total_tm_len += tm_1.len();
        tm_source.add_tm(&tm_1);

        // Set up server
        let mut validator = SimpleValidator::default();
        validator.0.insert(TEST_PACKET_ID_0);
        validator.0.insert(TEST_PACKET_ID_1);
        let mut tcp_server = generic_tmtc_server(
            &auto_port_addr,
            super::CcsdsPacketParser::new(TCP_SERVER_ID, 1024, tc_sender.clone(), validator),
            tm_source,
            None,
        );
        let dest_addr = tcp_server
            .local_addr()
            .expect("retrieving dest addr failed");
        let conn_handled: Arc<AtomicBool> = Default::default();
        let set_if_done = conn_handled.clone();

        // Call the connection handler in separate thread, does block.
        thread::spawn(move || {
            let result = tcp_server.handle_all_connections(Some(Duration::from_millis(100)));
            if result.is_err() {
                panic!("handling connection failed: {:?}", result.unwrap_err());
            }
            let conn_result = result.unwrap();
            matches!(conn_result, ConnectionResult::HandledConnections(1));
            tcp_server
                .generic_server
                .finished_handler
                .check_last_connection(2, 2);
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

        // Send telecommands
        let ping_tc = PusTcCreator::new_simple(
            SpHeader::new_from_apid(TEST_APID_0),
            MessageTypeId::new(17, 1),
            &[],
            CreatorConfig::default(),
        );
        let tc_0 = ping_tc.to_vec().expect("ping tc creation failed");
        stream
            .write_all(&tc_0)
            .expect("writing to TCP server failed");
        let action_tc = PusTcCreator::new_simple(
            SpHeader::new_from_apid(TEST_APID_1),
            MessageTypeId::new(8, 0),
            &[],
            CreatorConfig::default(),
        );
        let tc_1 = action_tc.to_vec().expect("action tc creation failed");
        stream
            .write_all(&tc_1)
            .expect("writing to TCP server failed");

        // Done with writing.
        stream
            .shutdown(std::net::Shutdown::Write)
            .expect("shutting down write failed");
        let mut read_buf: [u8; 32] = [0; 32];
        let mut current_idx = 0;
        let mut read_len_total = 0;
        // Timeout ensures this does not block forever.
        while read_len_total < total_tm_len {
            let read_len = stream
                .read(&mut read_buf[current_idx..])
                .expect("read failed");
            current_idx += read_len;
            read_len_total += read_len;
        }
        drop(stream);
        assert_eq!(read_buf[..tm_0.len()], tm_0);
        assert_eq!(read_buf[tm_0.len()..tm_0.len() + tm_1.len()], tm_1);

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
        let packet_0 = tc_receiver.try_recv().expect("receiving TC failed");
        assert_eq!(packet_0.packet, tc_0);
        let packet_1 = tc_receiver.try_recv().expect("receiving TC failed");
        assert_eq!(packet_1.packet, tc_1);
        matches!(tc_receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
    }
}
