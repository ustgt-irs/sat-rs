use alloc::sync::Arc;
use alloc::vec;
use cobs::CobsDecoderOwned;
use cobs::DecodeError;
use cobs::encode;
use core::sync::atomic::AtomicBool;
use core::time::Duration;
use delegate::delegate;
use mio::net::{TcpListener, TcpStream};
use std::io::Write;
use std::net::SocketAddr;
use std::vec::Vec;

use crate::queue::GenericSendError;
use crate::tmtc::PacketHandler;
use crate::tmtc::PacketSource;

use crate::ComponentId;
use crate::hal::std::tcp_server::{
    ConnectionResult, ServerConfig, TcpTcParser, TcpTmSender, TcpTmtcGenericServer,
};

use super::tcp_server::HandledConnectionHandler;
use super::tcp_server::HandledConnectionInfo;

/// Concrete [TcpTcParser] implementation for the [TcpTmtcInCobsServer].
pub struct CobsTcParser<PacketHandlerInstance: PacketHandler> {
    sender_id: ComponentId,
    owned_decoder: CobsDecoderOwned,
    packet_handler: PacketHandlerInstance,
    last_decode_error: Option<DecodeError>,
}

impl<PacketHandlerInstance: PacketHandler> CobsTcParser<PacketHandlerInstance> {
    pub fn new(
        sender_id: ComponentId,
        decoder_buf_size: usize,
        packet_handler: PacketHandlerInstance,
    ) -> Self {
        Self {
            sender_id,
            owned_decoder: CobsDecoderOwned::new(decoder_buf_size),
            packet_handler,
            last_decode_error: None,
        }
    }
}
impl<PacketHandlerInstance: PacketHandler> TcpTcParser for CobsTcParser<PacketHandlerInstance> {
    fn reset(&mut self) {
        self.owned_decoder.reset();
    }

    fn push(&mut self, data: &[u8], conn_result: &mut HandledConnectionInfo) {
        for byte in data {
            match self.owned_decoder.feed(*byte) {
                Ok(Some(packet_len)) => {
                    self.packet_handler
                        .handle_packet(self.sender_id, &self.owned_decoder.dest()[..packet_len])
                        .ok();
                    conn_result.num_received_tcs += 1;
                }
                Ok(None) => (),
                Err(e) => self.last_decode_error = Some(e),
            }
        }
    }
}

/// Concrete [TcpTmSender] implementation for the [TcpTmtcInCobsServer].
pub struct CobsTmSender {
    tm_encoding_buffer: Vec<u8>,
}

impl CobsTmSender {
    fn new(tm_buffer_size: usize) -> Self {
        Self {
            // The buffer should be large enough to hold the maximum expected TM size encoded with
            // COBS.
            tm_encoding_buffer: vec![0; cobs::max_encoding_length(tm_buffer_size)],
        }
    }
}

impl TcpTmSender for CobsTmSender {
    fn handle_tm_sending(
        &mut self,
        tm_buffer: &mut [u8],
        tm_source: &mut (impl PacketSource<Error = ()> + ?Sized),
        conn_result: &mut HandledConnectionInfo,
        stream: &mut TcpStream,
    ) -> Result<bool, std::io::Error> {
        let mut tm_was_sent = false;
        // Write TM until TM source is exhausted or there is an unexpected error. For now, there
        // is no limit for the amount of TM written this way.
        while let Ok(read_tm_len) = tm_source.retrieve_packet(tm_buffer) {
            if read_tm_len == 0 {
                return Ok(tm_was_sent);
            }
            tm_was_sent = true;
            conn_result.num_sent_tms += 1;

            // Encode into COBS and sent to client.
            let mut current_idx = 0;
            self.tm_encoding_buffer[current_idx] = 0;
            current_idx += 1;
            current_idx += encode(
                &tm_buffer[..read_tm_len],
                &mut self.tm_encoding_buffer[current_idx..],
            );
            self.tm_encoding_buffer[current_idx] = 0;
            current_idx += 1;
            stream.write_all(&self.tm_encoding_buffer[..current_idx])?;
        }
        Ok(tm_was_sent)
    }
}

/// TCP TMTC server implementation for exchange of generic TMTC packets which are framed with the
/// [COBS protocol](https://en.wikipedia.org/wiki/Consistent_Overhead_Byte_Stuffing).
///
/// Telemetry will be encoded with the COBS  protocol using [cobs::encode] in addition to being
/// wrapped with the sentinel value 0 as the packet delimiter as well before being sent back to
/// the client. Please note that the server will send as much data as it can retrieve from the
/// [PacketSource] in its current implementation.
///
/// Using a framing protocol like COBS imposes minimal restrictions on the type of TMTC data
/// exchanged while also allowing packets with flexible size and a reliable way to reconstruct full
/// packets even from a data stream which is split up. The server wil use the streaming
/// [CobsDecoderOwned] decoder to parse for packets and pass them to a
/// generic TC receiver. The user can use [crate::encoding::encode_packet_with_cobs] to encode
/// telecommands sent to the server.
///
/// ## Example
///
/// The [TCP integration tests](https://github.com/ustgt-irs/sat-rs/blob/main/satrs/tests/tcp_servers.rs)
/// test also serves as the example application for this module.
pub struct TcpTmtcInCobsServer<
    TmSource: PacketSource<Error = ()>,
    TcHandler: PacketHandler<Error = GenericSendError>,
    HandledConnection: HandledConnectionHandler,
> {
    pub generic_server:
        TcpTmtcGenericServer<TmSource, CobsTmSender, CobsTcParser<TcHandler>, HandledConnection>,
}

impl<
    TmSource: PacketSource<Error = ()>,
    TcHandler: PacketHandler<Error = GenericSendError>,
    HandledConnection: HandledConnectionHandler,
> TcpTmtcInCobsServer<TmSource, TcHandler, HandledConnection>
{
    /// Create a new TCP TMTC server which exchanges TMTC packets encoded with
    /// [COBS protocol](https://en.wikipedia.org/wiki/Consistent_Overhead_Byte_Stuffing).
    ///
    /// ## Parameter
    ///
    /// * `cfg` - Configuration of the server.
    /// * `tm_source` - Generic TM source used by the server to pull telemetry packets which are
    ///   then sent back to the client.
    /// * `tc_receiver` - Any received telecommands which were decoded successfully will be
    ///   forwarded to this TC receiver.
    pub fn new(
        cfg: ServerConfig,
        tm_source: TmSource,
        cobs_tc_parser: CobsTcParser<TcHandler>,
        handled_connection: HandledConnection,
        stop_signal: Option<Arc<AtomicBool>>,
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            generic_server: TcpTmtcGenericServer::new(
                cfg,
                cobs_tc_parser,
                CobsTmSender::new(cfg.tm_buffer_size),
                tm_source,
                handled_connection,
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
                poll_duration: Option<Duration>,
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
    use std::{
        io::{Read, Write},
        net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
        panic,
        sync::mpsc,
        thread,
        time::Instant,
    };

    use crate::{
        ComponentId,
        encoding::tests::{INVERTED_PACKET, SIMPLE_PACKET},
        hal::std::tcp_server::{
            ConnectionResult, ServerConfig,
            tests::{ConnectionFinishedHandler, SyncTmSource},
        },
        tmtc::PacketAsVec,
    };
    use alloc::sync::Arc;
    use cobs::encode;

    use super::TcpTmtcInCobsServer;

    const TCP_SERVER_ID: ComponentId = 0x05;

    fn encode_simple_packet(encoded_buf: &mut [u8], current_idx: &mut usize) {
        encode_packet(&SIMPLE_PACKET, encoded_buf, current_idx)
    }

    fn encode_inverted_packet(encoded_buf: &mut [u8], current_idx: &mut usize) {
        encode_packet(&INVERTED_PACKET, encoded_buf, current_idx)
    }

    fn encode_packet(packet: &[u8], encoded_buf: &mut [u8], current_idx: &mut usize) {
        encoded_buf[*current_idx] = 0;
        *current_idx += 1;
        *current_idx += encode(packet, &mut encoded_buf[*current_idx..]);
        encoded_buf[*current_idx] = 0;
        *current_idx += 1;
    }

    fn generic_tmtc_server(
        addr: &SocketAddr,
        tc_sender: mpsc::Sender<PacketAsVec>,
        tm_source: SyncTmSource,
        stop_signal: Option<Arc<AtomicBool>>,
    ) -> TcpTmtcInCobsServer<SyncTmSource, mpsc::Sender<PacketAsVec>, ConnectionFinishedHandler>
    {
        TcpTmtcInCobsServer::new(
            ServerConfig::new(TCP_SERVER_ID, *addr, Duration::from_millis(2), 1024, 1024),
            tm_source,
            super::CobsTcParser::new(TCP_SERVER_ID, 1024, tc_sender),
            ConnectionFinishedHandler::default(),
            stop_signal,
        )
        .expect("TCP server generation failed")
    }

    #[test]
    fn test_server_basic_no_tm() {
        let auto_port_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let (tc_sender, tc_receiver) = mpsc::channel();
        let tm_source = SyncTmSource::default();
        let mut tcp_server =
            generic_tmtc_server(&auto_port_addr, tc_sender.clone(), tm_source, None);
        let dest_addr = tcp_server
            .local_addr()
            .expect("retrieving dest addr failed");
        let conn_handled: Arc<AtomicBool> = Default::default();
        let set_if_done = conn_handled.clone();
        // Call the connection handler in separate thread, does block.
        thread::spawn(move || {
            let result = tcp_server.handle_all_connections(Some(Duration::from_millis(100)));
            if let Err(e) = result {
                panic!("handling connection failed: {e:?}");
            }
            let result = result.unwrap();
            assert_eq!(result, ConnectionResult::HandledConnections(1));
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
        // Send TC to server now.
        let mut encoded_buf: [u8; 16] = [0; 16];
        let mut current_idx = 0;
        encode_simple_packet(&mut encoded_buf, &mut current_idx);
        let mut stream = TcpStream::connect(dest_addr).expect("connecting to TCP server failed");
        stream
            .write_all(&encoded_buf[..current_idx])
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
        // Check that the packet was received and decoded successfully.
        let packet_with_sender = tc_receiver.recv().expect("receiving TC failed");
        assert_eq!(packet_with_sender.packet, &SIMPLE_PACKET);
        matches!(tc_receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
    }

    #[test]
    fn test_server_basic_multi_tm_multi_tc() {
        let auto_port_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let (tc_sender, tc_receiver) = mpsc::channel();
        let mut tm_source = SyncTmSource::default();
        tm_source.add_tm(&INVERTED_PACKET);
        tm_source.add_tm(&SIMPLE_PACKET);
        let mut tcp_server =
            generic_tmtc_server(&auto_port_addr, tc_sender.clone(), tm_source.clone(), None);
        let dest_addr = tcp_server
            .local_addr()
            .expect("retrieving dest addr failed");
        let conn_handled: Arc<AtomicBool> = Default::default();
        let set_if_done = conn_handled.clone();
        // Call the connection handler in separate thread, does block.
        thread::spawn(move || {
            let result = tcp_server.handle_all_connections(Some(Duration::from_millis(100)));
            if let Err(e) = result {
                panic!("handling connection failed: {e:?}");
            }
            let result = result.unwrap();
            assert_eq!(result, ConnectionResult::HandledConnections(1));
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
        // Send TC to server now.
        let mut encoded_buf: [u8; 32] = [0; 32];
        let mut current_idx = 0;
        encode_simple_packet(&mut encoded_buf, &mut current_idx);
        encode_inverted_packet(&mut encoded_buf, &mut current_idx);
        let mut stream = TcpStream::connect(dest_addr).expect("connecting to TCP server failed");
        stream
            .set_read_timeout(Some(Duration::from_millis(10)))
            .expect("setting reas timeout failed");
        stream
            .write_all(&encoded_buf[..current_idx])
            .expect("writing to TCP server failed");
        // Done with writing.
        stream
            .shutdown(std::net::Shutdown::Write)
            .expect("shutting down write failed");
        let mut read_buf: [u8; 16] = [0; 16];
        let mut read_len_total = 0;
        // Timeout ensures this does not block forever.
        while read_len_total < 16 {
            let read_len = stream.read(&mut read_buf).expect("read failed");
            read_len_total += read_len;
            // Read until full expected size is available.
            if read_len == 16 {
                // Read first TM packet.
                current_idx = 0;
                assert_eq!(read_len, 16);
                assert_eq!(read_buf[0], 0);
                current_idx += 1;
                let mut dec_report = cobs::decode_in_place_report(&mut read_buf[current_idx..])
                    .expect("COBS decoding failed");
                assert_eq!(dec_report.frame_size(), 5);
                // Skip first sentinel byte.
                assert_eq!(
                    &read_buf[current_idx..current_idx + INVERTED_PACKET.len()],
                    &INVERTED_PACKET
                );
                current_idx += dec_report.parsed_size();
                // End sentinel.
                assert_eq!(read_buf[current_idx], 0, "invalid sentinel end byte");
                current_idx += 1;

                // Read second TM packet.
                assert_eq!(read_buf[current_idx], 0);
                current_idx += 1;
                dec_report = cobs::decode_in_place_report(&mut read_buf[current_idx..])
                    .expect("COBS decoding failed");
                assert_eq!(dec_report.frame_size(), 5);
                // Skip first sentinel byte.
                assert_eq!(
                    &read_buf[current_idx..current_idx + SIMPLE_PACKET.len()],
                    &SIMPLE_PACKET
                );
                current_idx += dec_report.parsed_size();
                // End sentinel.
                assert_eq!(read_buf[current_idx], 0);
                break;
            }
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
        // Check that the packet was received and decoded successfully.
        let packet_with_sender = tc_receiver.recv().expect("receiving TC failed");
        let packet = &packet_with_sender.packet;
        assert_eq!(packet, &SIMPLE_PACKET);
        let packet_with_sender = tc_receiver.recv().expect("receiving TC failed");
        let packet = &packet_with_sender.packet;
        assert_eq!(packet, &INVERTED_PACKET);
        matches!(tc_receiver.try_recv(), Err(mpsc::TryRecvError::Empty));
    }

    #[test]
    fn test_server_accept_timeout() {
        let auto_port_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let (tc_sender, _tc_receiver) = mpsc::channel();
        let tm_source = SyncTmSource::default();
        let mut tcp_server =
            generic_tmtc_server(&auto_port_addr, tc_sender.clone(), tm_source, None);
        let start = Instant::now();
        // Call the connection handler in separate thread, does block.
        let thread_jh = thread::spawn(move || {
            loop {
                let result = tcp_server.handle_all_connections(Some(Duration::from_millis(20)));
                if let Err(e) = result {
                    panic!("handling connection failed: {e:?}");
                }
                let result = result.unwrap();
                if result == ConnectionResult::AcceptTimeout {
                    break;
                }
                if Instant::now() - start > Duration::from_millis(100) {
                    panic!("regular stop signal handling failed");
                }
            }
        });
        thread_jh.join().expect("thread join failed");
    }

    #[test]
    fn test_server_stop_signal() {
        let auto_port_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 0);
        let (tc_sender, _tc_receiver) = mpsc::channel();
        let tm_source = SyncTmSource::default();
        let stop_signal = Arc::new(AtomicBool::new(false));
        let mut tcp_server = generic_tmtc_server(
            &auto_port_addr,
            tc_sender.clone(),
            tm_source,
            Some(stop_signal.clone()),
        );
        let dest_addr = tcp_server
            .local_addr()
            .expect("retrieving dest addr failed");
        let stop_signal_copy = stop_signal.clone();
        let start = Instant::now();
        // Call the connection handler in separate thread, does block.
        let thread_jh = thread::spawn(move || {
            loop {
                let result = tcp_server.handle_all_connections(Some(Duration::from_millis(20)));
                if let Err(e) = result {
                    panic!("handling connection failed: {e:?}");
                }
                let result = result.unwrap();
                if result == ConnectionResult::AcceptTimeout {
                    panic!("unexpected accept timeout");
                }
                if stop_signal_copy.load(Ordering::Relaxed) {
                    break;
                }
                if Instant::now() - start > Duration::from_millis(100) {
                    panic!("regular stop signal handling failed");
                }
            }
        });
        // We connect but do not do anything.
        let _stream = TcpStream::connect(dest_addr).expect("connecting to TCP server failed");
        stop_signal.store(true, Ordering::Relaxed);
        // No need to drop the connection, the stop signal should take take of everything.
        thread_jh.join().expect("thread join failed");
    }
}
