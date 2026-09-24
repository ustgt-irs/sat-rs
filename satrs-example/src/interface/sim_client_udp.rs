use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    sync::mpsc,
    time::Duration,
};

use satrs::HandlingStatus;
use satrs_minisim::{ComponentId, SimReply, SimRequestWithTime, udp::SIM_CTRL_PORT};
use satrs_minisim::{SimCtrlReply, SimCtrlRequest};

struct SimReplyMap(pub HashMap<ComponentId, mpsc::Sender<SimReply>>);

pub fn create_sim_client(
    sim_request_rx: mpsc::Receiver<SimRequestWithTime>,
) -> Option<SimClientUdp> {
    match SimClientUdp::new(
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, SIM_CTRL_PORT)),
        sim_request_rx,
    ) {
        Ok(sim_client) => {
            log::info!("simulator client connection success");
            return Some(sim_client);
        }
        Err(e) => {
            log::warn!("sim client creation error: {e}");
        }
    }
    None
}

#[derive(thiserror::Error, Debug)]
pub enum SimClientCreationError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("timeout when trying to connect to sim UDP server")]
    Timeout,
    #[error("invalid ping reply when trying connection to UDP sim server")]
    InvalidReplyJsonError(#[from] serde_json::Error),
    #[error("invalid sim reply, not pong reply as expected: {0:?}")]
    ReplyIsNotPong(SimReply),
}

pub struct SimClientUdp {
    udp_client: UdpSocket,
    simulator_addr: SocketAddr,
    sim_request_rx: mpsc::Receiver<SimRequestWithTime>,
    reply_map: SimReplyMap,
    reply_buf: [u8; 4096],
}

impl SimClientUdp {
    pub fn new(
        simulator_addr: SocketAddr,
        sim_request_rx: mpsc::Receiver<SimRequestWithTime>,
    ) -> Result<Self, SimClientCreationError> {
        let mut reply_buf: [u8; 4096] = [0; 4096];
        let mut udp_client = UdpSocket::bind("127.0.0.1:0")?;
        udp_client.set_read_timeout(Some(Duration::from_millis(100)))?;
        Self::attempt_connection(&mut udp_client, simulator_addr, &mut reply_buf)?;
        udp_client.set_nonblocking(true)?;
        Ok(Self {
            udp_client,
            simulator_addr,
            sim_request_rx,
            reply_map: SimReplyMap(HashMap::new()),
            reply_buf,
        })
    }

    pub fn attempt_connection(
        udp_client: &mut UdpSocket,
        simulator_addr: SocketAddr,
        reply_buf: &mut [u8],
    ) -> Result<(), SimClientCreationError> {
        let sim_req = SimRequestWithTime::new_with_epoch_time(SimCtrlRequest::Ping);
        let sim_req_json = serde_json::to_string(&sim_req).expect("failed to serialize SimRequest");
        udp_client.send_to(sim_req_json.as_bytes(), simulator_addr)?;
        match udp_client.recv(reply_buf) {
            Ok(reply_len) => {
                let sim_reply: SimReply = serde_json::from_slice(&reply_buf[0..reply_len])?;
                match sim_reply {
                    SimReply::SimCtrl(SimCtrlReply::Pong) => Ok(()),
                    _ => Err(SimClientCreationError::ReplyIsNotPong(sim_reply)),
                }
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::WouldBlock
                {
                    Err(SimClientCreationError::Timeout)
                } else {
                    Err(SimClientCreationError::Io(e))
                }
            }
        }
    }

    pub fn operation(&mut self) -> HandlingStatus {
        let mut no_sim_requests_handled = true;
        let mut no_data_from_udp_server_received = true;
        loop {
            match self.sim_request_rx.try_recv() {
                Ok(request) => {
                    let request_json =
                        serde_json::to_string(&request).expect("failed to serialize SimRequest");
                    if let Err(e) = self
                        .udp_client
                        .send_to(request_json.as_bytes(), self.simulator_addr)
                    {
                        log::error!("error sending data to UDP SIM server: {e}");
                        break;
                    } else {
                        no_sim_requests_handled = false;
                    }
                }
                Err(e) => match e {
                    mpsc::TryRecvError::Empty => {
                        break;
                    }
                    mpsc::TryRecvError::Disconnected => {
                        log::warn!("SIM request sender disconnected");
                        break;
                    }
                },
            }
        }
        loop {
            match self.udp_client.recv(&mut self.reply_buf) {
                Ok(recvd_bytes) => {
                    no_data_from_udp_server_received = false;
                    let sim_reply_result: serde_json::Result<SimReply> =
                        serde_json::from_slice(&self.reply_buf[0..recvd_bytes]);
                    match sim_reply_result {
                        Ok(sim_reply) => {
                            if let Some(sender) = self.reply_map.0.get(&sim_reply.component()) {
                                sender.send(sim_reply).expect("failed to send SIM reply");
                            } else {
                                log::warn!(
                                    "no recipient for SIM reply from component {:?}",
                                    sim_reply.component()
                                );
                            }
                        }
                        Err(e) => {
                            log::warn!("failed to deserialize SIM reply: {e}");
                        }
                    }
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut
                    {
                        break;
                    }
                    log::error!("error receiving data from UDP SIM server: {e}");
                    break;
                }
            }
        }
        if no_sim_requests_handled && no_data_from_udp_server_received {
            return HandlingStatus::Empty;
        }
        HandlingStatus::HandledOne
    }

    pub fn add_reply_recipient(
        &mut self,
        component: ComponentId,
        reply_sender: mpsc::Sender<SimReply>,
    ) {
        self.reply_map.0.insert(component, reply_sender);
    }
}

#[cfg(test)]
pub mod tests {
    use std::{
        collections::HashMap,
        net::{SocketAddr, UdpSocket},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::Duration,
    };

    use satrs_minisim::{
        ComponentId, SimCtrlReply, SimCtrlRequest, SimReply, SimRequest, SimRequestWithTime,
        eps::{PcduReply, PcduRequest},
    };

    use super::SimClientUdp;

    struct UdpSimTestServer {
        udp_server: UdpSocket,
        request_tx: mpsc::Sender<SimRequestWithTime>,
        reply_rx: mpsc::Receiver<SimReply>,
        last_sender: Option<SocketAddr>,
        stop_signal: Arc<AtomicBool>,
        recv_buf: [u8; 1024],
    }

    impl UdpSimTestServer {
        pub fn new(
            request_tx: mpsc::Sender<SimRequestWithTime>,
            reply_rx: mpsc::Receiver<SimReply>,
            stop_signal: Arc<AtomicBool>,
        ) -> Self {
            let udp_server = UdpSocket::bind("127.0.0.1:0").expect("creating UDP server failed");
            udp_server
                .set_nonblocking(true)
                .expect("failed to set UDP server to non-blocking");
            Self {
                udp_server,
                request_tx,
                reply_rx,
                last_sender: None,
                stop_signal,
                recv_buf: [0; 1024],
            }
        }

        pub fn operation(&mut self) {
            loop {
                let mut no_sim_replies_handled = true;
                let mut no_data_received = true;
                if self.stop_signal.load(Ordering::Relaxed) {
                    break;
                }
                if let Some(last_sender) = self.last_sender {
                    loop {
                        match self.reply_rx.try_recv() {
                            Ok(sim_reply) => {
                                let sim_reply_json = serde_json::to_string(&sim_reply)
                                    .expect("failed to serialize SimReply");
                                self.udp_server
                                    .send_to(sim_reply_json.as_bytes(), last_sender)
                                    .expect("failed to send reply to client from UDP server");
                                no_sim_replies_handled = false;
                            }
                            Err(e) => match e {
                                mpsc::TryRecvError::Empty => break,
                                mpsc::TryRecvError::Disconnected => {
                                    panic!("reply sender disconnected")
                                }
                            },
                        }
                    }
                }

                loop {
                    match self.udp_server.recv_from(&mut self.recv_buf) {
                        Ok((read_bytes, from)) => {
                            let sim_request: SimRequestWithTime =
                                serde_json::from_slice(&self.recv_buf[0..read_bytes])
                                    .expect("failed to deserialize SimRequest");
                            // For a ping, we perform the reply handling here directly
                            if sim_request.request == SimRequest::SimCtrl(SimCtrlRequest::Ping) {
                                no_data_received = false;
                                self.last_sender = Some(from);
                                let sim_reply = SimReply::from(SimCtrlReply::Pong);
                                let sim_reply_json = serde_json::to_string(&sim_reply)
                                    .expect("failed to serialize SimReply");
                                self.udp_server
                                    .send_to(sim_reply_json.as_bytes(), from)
                                    .expect("failed to send reply to client from UDP server");
                            }
                            // Forward each SIM request for testing purposes.
                            self.request_tx
                                .send(sim_request)
                                .expect("failed to send request");
                        }
                        Err(e) => {
                            if e.kind() != std::io::ErrorKind::WouldBlock
                                && e.kind() != std::io::ErrorKind::TimedOut
                            {
                                panic!("UDP server error: {}", e);
                            }
                            break;
                        }
                    }
                }
                if no_sim_replies_handled && no_data_received {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }

        pub fn local_addr(&self) -> SocketAddr {
            self.udp_server.local_addr().unwrap()
        }
    }

    #[test]
    fn basic_connection_test() {
        let (server_sim_request_tx, server_sim_request_rx) = mpsc::channel();
        let (_server_sim_reply_tx, server_sim_reply_rx) = mpsc::channel();
        let stop_signal = Arc::new(AtomicBool::new(false));
        let mut udp_server = UdpSimTestServer::new(
            server_sim_request_tx,
            server_sim_reply_rx,
            stop_signal.clone(),
        );
        let server_addr = udp_server.local_addr();
        let (_client_sim_req_tx, client_sim_req_rx) = mpsc::channel();
        // Need to spawn the simulator UDP server before calling the client constructor.
        let jh0 = std::thread::spawn(move || {
            udp_server.operation();
        });
        // Creating the client also performs the connection test.
        SimClientUdp::new(server_addr, client_sim_req_rx).unwrap();
        let sim_request = server_sim_request_rx
            .recv_timeout(Duration::from_millis(50))
            .expect("no SIM request received");
        assert_eq!(
            sim_request.request,
            SimRequest::SimCtrl(SimCtrlRequest::Ping)
        );
        // Stop the server.
        stop_signal.store(true, Ordering::Relaxed);
        jh0.join().unwrap();
    }

    #[test]
    fn basic_request_reply_test() {
        let (server_sim_request_tx, server_sim_request_rx) = mpsc::channel();
        let (server_sim_reply_tx, sever_sim_reply_rx) = mpsc::channel();
        let stop_signal = Arc::new(AtomicBool::new(false));
        let mut udp_server = UdpSimTestServer::new(
            server_sim_request_tx,
            sever_sim_reply_rx,
            stop_signal.clone(),
        );
        let server_addr = udp_server.local_addr();
        let (client_sim_req_tx, client_sim_req_rx) = mpsc::channel();
        let (client_pcdu_reply_tx, client_pcdu_reply_rx) = mpsc::channel();
        // Need to spawn the simulator UDP server before calling the client constructor.
        let jh0 = std::thread::spawn(move || {
            udp_server.operation();
        });

        // Creating the client also performs the connection test.
        let mut client = SimClientUdp::new(server_addr, client_sim_req_rx).unwrap();
        client.add_reply_recipient(ComponentId::Pcdu, client_pcdu_reply_tx);

        let sim_request = server_sim_request_rx
            .recv_timeout(Duration::from_millis(50))
            .expect("no SIM request received");
        assert_eq!(
            sim_request.request,
            SimRequest::SimCtrl(SimCtrlRequest::Ping)
        );

        let pcdu_req = PcduRequest::RequestSwitchInfo;
        client_sim_req_tx
            .send(SimRequestWithTime::new_with_epoch_time(pcdu_req))
            .expect("send failed");
        client.operation();

        // Check that the request arrives properly at the server.
        let sim_request = server_sim_request_rx
            .recv_timeout(Duration::from_millis(50))
            .expect("no SIM request received");
        assert_eq!(
            sim_request.request,
            SimRequest::Pcdu(PcduRequest::RequestSwitchInfo)
        );

        // We inject the reply ourselves.
        let pcdu_reply = PcduReply::SwitchInfo(HashMap::new());
        server_sim_reply_tx
            .send(SimReply::from(pcdu_reply.clone()))
            .expect("sending PCDU reply failed");

        // Now we verify that the reply is sent by the UDP server back to the client, and then
        // forwarded by the clients internal map.
        let mut pcdu_reply_received = false;
        for _ in 0..3 {
            client.operation();

            match client_pcdu_reply_rx.try_recv() {
                Ok(sim_reply) => {
                    assert_eq!(sim_reply.component(), ComponentId::Pcdu);
                    assert_eq!(sim_reply, SimReply::Pcdu(pcdu_reply.clone()));
                    pcdu_reply_received = true;
                    break;
                }
                Err(e) => match e {
                    mpsc::TryRecvError::Empty => std::thread::sleep(Duration::from_millis(10)),
                    mpsc::TryRecvError::Disconnected => panic!("reply sender disconnected"),
                },
            }
        }
        if !pcdu_reply_received {
            panic!("no reply received");
        }

        // Stop the server.
        stop_signal.store(true, Ordering::Relaxed);
        jh0.join().unwrap();
    }
}
