use std::{
    collections::VecDeque,
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    sync::{atomic::AtomicBool, mpsc, Arc},
    time::Duration,
};

use satrs_minisim::{SimReply, SimRequestWithTime};

// A UDP server which handles all TC received by a client application.
pub struct SimUdpServer {
    socket: UdpSocket,
    request_sender: mpsc::Sender<SimRequestWithTime>,
    reply_receiver: mpsc::Receiver<SimReply>,
    reply_queue: VecDeque<SimReply>,
    max_num_replies: usize,
    // Stop signal to stop the server. Required for unittests and useful to allow clean shutdown
    // of the application.
    stop_signal: Option<Arc<AtomicBool>>,
    idle_sleep_period_ms: u64,
    req_buf: [u8; 4096],
    sender_addr: Option<SocketAddr>,
}

impl SimUdpServer {
    pub fn new(
        local_port: u16,
        request_sender: mpsc::Sender<SimRequestWithTime>,
        reply_receiver: mpsc::Receiver<SimReply>,
        max_num_replies: usize,
        stop_signal: Option<Arc<AtomicBool>>,
    ) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], local_port)))?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            request_sender,
            reply_receiver,
            reply_queue: VecDeque::new(),
            max_num_replies,
            stop_signal,
            idle_sleep_period_ms: 3,
            req_buf: [0; 4096],
            sender_addr: None,
        })
    }

    #[cfg(test)]
    pub fn server_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn run(&mut self) {
        loop {
            if let Some(stop_signal) = &self.stop_signal {
                if stop_signal.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
            }
            let processed_requests = self.process_requests();
            let processed_replies = self.process_replies();
            let sent_replies = self.send_replies();
            // Sleep for a bit if there is nothing to do to prevent burning CPU cycles. Delay
            // should be kept short to ensure responsiveness of the system.
            if !processed_requests && !processed_replies && !sent_replies {
                std::thread::sleep(Duration::from_millis(self.idle_sleep_period_ms));
            }
        }
    }

    fn process_requests(&mut self) -> bool {
        let mut processed_requests = false;
        loop {
            // Blocks for a certain amount of time until data is received to allow doing periodic
            // work like checking the stop signal.
            let (bytes_read, src) = match self.socket.recv_from(&mut self.req_buf) {
                Ok((bytes_read, src)) => (bytes_read, src),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    // Continue to perform regular checks like the stop signal.
                    break;
                }
                Err(e) => {
                    // Handle unexpected errors (e.g., socket closed) here.
                    log::error!("unexpected request server error: {e}");
                    break;
                }
            };

            self.sender_addr = Some(src);

            let sim_req = serde_json::from_slice::<SimRequestWithTime>(&self.req_buf[..bytes_read]);
            if let Err(e) = sim_req {
                log::warn!("received UDP request with invalid format: {}", e);
                return processed_requests;
            }
            self.request_sender.send(sim_req.unwrap()).unwrap();
            processed_requests = true;
        }
        processed_requests
    }

    fn process_replies(&mut self) -> bool {
        let mut processed_replies = false;
        loop {
            match self.reply_receiver.try_recv() {
                Ok(reply) => {
                    if self.reply_queue.len() >= self.max_num_replies {
                        self.reply_queue.pop_front();
                    }
                    self.reply_queue.push_back(reply);
                    processed_replies = true;
                }
                Err(e) => match e {
                    mpsc::TryRecvError::Empty => return processed_replies,
                    mpsc::TryRecvError::Disconnected => {
                        log::error!("all UDP reply senders disconnected")
                    }
                },
            }
        }
    }

    fn send_replies(&mut self) -> bool {
        if self.sender_addr.is_none() {
            return false;
        }
        let mut sent_replies = false;
        while !self.reply_queue.is_empty() {
            let next_reply_to_send = self.reply_queue.pop_front().unwrap();
            self.socket
                .send_to(
                    serde_json::to_string(&next_reply_to_send)
                        .unwrap()
                        .as_bytes(),
                    self.sender_addr.unwrap(),
                )
                .expect("sending reply failed");
            sent_replies = true;
        }
        sent_replies
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::ErrorKind,
        net::{SocketAddr, UdpSocket},
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc,
        },
        time::Duration,
    };

    use satrs_minisim::{
        eps::{PcduReply, PcduRequest},
        SimCtrlReply, SimCtrlRequest, SimReply, SimRequestWithTime,
    };

    use crate::eps::tests::get_all_off_switch_map;
    use delegate::delegate;

    use super::SimUdpServer;

    // Wait time to ensure even possibly laggy systems like CI servers can run the tests.
    const SERVER_WAIT_TIME_MS: u64 = 50;

    #[derive(thiserror::Error, Debug)]
    pub enum ReceptionError {
        #[error("IO error: {0}")]
        Io(#[from] std::io::Error),
        #[error("Serde JSON error: {0}")]
        SerdeJson(#[from] serde_json::Error),
    }

    pub struct SimUdpTestClient {
        socket: UdpSocket,
        pub reply_buf: [u8; 4096],
    }

    impl SimUdpTestClient {
        pub fn new(
            server_addr: &SocketAddr,
            non_blocking: bool,
            read_timeot_ms: Option<u64>,
        ) -> std::io::Result<Self> {
            let socket = UdpSocket::bind("127.0.0.1:0")?;
            socket.set_nonblocking(non_blocking)?;
            socket
                .connect(server_addr)
                .expect("could not connect to server addr");
            if let Some(read_timeout) = read_timeot_ms {
                // Set a read timeout so the test does not hang on failures.
                socket.set_read_timeout(Some(Duration::from_millis(read_timeout)))?;
            }
            Ok(Self {
                socket,
                reply_buf: [0; 4096],
            })
        }

        pub fn send_request(&self, sim_request: &SimRequestWithTime) -> std::io::Result<usize> {
            self.socket.send(
                &serde_json::to_vec(sim_request).expect("conversion of request to vector failed"),
            )
        }

        pub fn recv_raw(&mut self) -> std::io::Result<usize> {
            self.socket.recv(&mut self.reply_buf)
        }

        pub fn recv_sim_reply(&mut self) -> Result<SimReply, ReceptionError> {
            let read_len = self.recv_raw()?;
            Ok(serde_json::from_slice(&self.reply_buf[0..read_len])?)
        }
    }
    struct UdpTestbench {
        client: SimUdpTestClient,
        stop_signal: Arc<AtomicBool>,
        request_receiver: mpsc::Receiver<SimRequestWithTime>,
        reply_sender: mpsc::Sender<SimReply>,
    }

    impl UdpTestbench {
        pub fn new(
            client_non_blocking: bool,
            client_read_timeout_ms: Option<u64>,
            max_num_replies: usize,
        ) -> std::io::Result<(Self, SimUdpServer)> {
            let (request_sender, request_receiver) = mpsc::channel();
            let (reply_sender, reply_receiver) = mpsc::channel();
            let stop_signal = Arc::new(AtomicBool::new(false));
            let server = SimUdpServer::new(
                0,
                request_sender,
                reply_receiver,
                max_num_replies,
                Some(stop_signal.clone()),
            )?;
            let server_addr = server.server_addr()?;
            Ok((
                Self {
                    client: SimUdpTestClient::new(
                        &server_addr,
                        client_non_blocking,
                        client_read_timeout_ms,
                    )?,
                    stop_signal,
                    request_receiver,
                    reply_sender,
                },
                server,
            ))
        }

        pub fn try_recv_request(&self) -> Result<SimRequestWithTime, mpsc::TryRecvError> {
            self.request_receiver.try_recv()
        }

        pub fn stop(&self) {
            self.stop_signal.store(true, Ordering::Relaxed);
        }

        pub fn send_reply(&self, sim_reply: &SimReply) {
            self.reply_sender
                .send(sim_reply.clone())
                .expect("sending sim reply failed");
        }

        delegate! {
            to self.client {
                pub fn send_request(&self, sim_request: &SimRequestWithTime) -> std::io::Result<usize>;
                pub fn recv_sim_reply(&mut self) -> Result<SimReply, ReceptionError>;
            }
        }

        pub fn check_no_sim_reply_available(&mut self) {
            if let Err(ReceptionError::Io(ref io_error)) = self.recv_sim_reply() {
                if io_error.kind() == ErrorKind::WouldBlock {
                    // Continue to perform regular checks like the stop signal.
                    return;
                } else {
                    // Handle unexpected errors (e.g., socket closed) here.
                    panic!("unexpected request server error: {io_error}");
                }
            }
            panic!("unexpected reply available");
        }

        pub fn check_next_sim_reply(&mut self, expected_reply: &SimReply) {
            match self.recv_sim_reply() {
                Ok(received_sim_reply) => assert_eq!(expected_reply, &received_sim_reply),
                Err(e) => match e {
                    ReceptionError::Io(ref io_error) => {
                        if io_error.kind() == ErrorKind::WouldBlock {
                            // Continue to perform regular checks like the stop signal.
                            panic!("no simulation reply received");
                        } else {
                            // Handle unexpected errors (e.g., socket closed) here.
                            panic!("unexpected request server error: {e}");
                        }
                    }
                    ReceptionError::SerdeJson(json_error) => {
                        panic!("unexpected JSON error: {json_error}");
                    }
                },
            }
        }
    }
    #[test]
    fn test_basic_udp_request_reception() {
        let (udp_testbench, mut udp_server) =
            UdpTestbench::new(true, Some(SERVER_WAIT_TIME_MS), 10)
                .expect("could not create testbench");
        let server_thread = std::thread::spawn(move || udp_server.run());
        let sim_request = SimRequestWithTime::new_with_epoch_time(PcduRequest::RequestSwitchInfo);
        udp_testbench
            .send_request(&sim_request)
            .expect("sending request failed");
        std::thread::sleep(Duration::from_millis(SERVER_WAIT_TIME_MS));
        // Check that the sim request has arrives and was forwarded.
        let received_sim_request = udp_testbench
            .try_recv_request()
            .expect("did not receive request");
        assert_eq!(sim_request, received_sim_request);
        // Stop the server.
        udp_testbench.stop();
        server_thread.join().unwrap();
    }

    #[test]
    fn test_udp_reply_server() {
        let (mut udp_testbench, mut udp_server) =
            UdpTestbench::new(false, Some(SERVER_WAIT_TIME_MS), 10)
                .expect("could not create testbench");
        let server_thread = std::thread::spawn(move || udp_server.run());
        udp_testbench
            .send_request(&SimRequestWithTime::new_with_epoch_time(
                SimCtrlRequest::Ping,
            ))
            .expect("sending request failed");

        let sim_reply = SimReply::from(PcduReply::SwitchInfo(get_all_off_switch_map()));
        udp_testbench.send_reply(&sim_reply);

        udp_testbench.check_next_sim_reply(&sim_reply);

        // Stop the server.
        udp_testbench.stop();
        server_thread.join().unwrap();
    }

    #[test]
    fn test_udp_req_server_and_reply_sender() {
        let (mut udp_testbench, mut udp_server) =
            UdpTestbench::new(false, Some(SERVER_WAIT_TIME_MS), 10)
                .expect("could not create testbench");

        let server_thread = std::thread::spawn(move || udp_server.run());

        // Send a ping so that the server knows the address of the client.
        // Do not check that the request arrives on the receiver side, is done by other test.
        udp_testbench
            .send_request(&SimRequestWithTime::new_with_epoch_time(
                SimCtrlRequest::Ping,
            ))
            .expect("sending request failed");

        // Send a reply to the server, ensure it gets forwarded to the client.
        let sim_reply = SimReply::from(PcduReply::SwitchInfo(get_all_off_switch_map()));
        udp_testbench.send_reply(&sim_reply);
        std::thread::sleep(Duration::from_millis(SERVER_WAIT_TIME_MS));

        // Now we check that the reply server can send back replies to the client.
        udp_testbench.check_next_sim_reply(&sim_reply);

        udp_testbench.stop();
        server_thread.join().unwrap();
    }

    #[test]
    fn test_udp_replies_client_unconnected() {
        let (mut udp_testbench, mut udp_server) =
            UdpTestbench::new(true, None, 10).expect("could not create testbench");

        let server_thread = std::thread::spawn(move || udp_server.run());

        // Send a reply to the server. The client is not connected, so it won't get forwarded.
        let sim_reply = SimReply::from(PcduReply::SwitchInfo(get_all_off_switch_map()));
        udp_testbench.send_reply(&sim_reply);
        std::thread::sleep(Duration::from_millis(10));

        udp_testbench.check_no_sim_reply_available();

        // Connect by sending a ping.
        udp_testbench
            .send_request(&SimRequestWithTime::new_with_epoch_time(
                SimCtrlRequest::Ping,
            ))
            .expect("sending request failed");
        std::thread::sleep(Duration::from_millis(SERVER_WAIT_TIME_MS));

        udp_testbench.check_next_sim_reply(&sim_reply);

        // Now we check that the reply server can sent back replies to the client.
        udp_testbench.stop();
        server_thread.join().unwrap();
    }

    #[test]
    fn test_udp_reply_server_old_replies_overwritten() {
        let (mut udp_testbench, mut udp_server) =
            UdpTestbench::new(true, None, 3).expect("could not create testbench");

        let server_thread = std::thread::spawn(move || udp_server.run());

        // The server only caches up to 3 replies.
        let sim_reply = SimReply::from(SimCtrlReply::Pong);
        for _ in 0..4 {
            udp_testbench.send_reply(&sim_reply);
        }
        std::thread::sleep(Duration::from_millis(20));

        udp_testbench.check_no_sim_reply_available();

        // Connect by sending a ping.
        udp_testbench
            .send_request(&SimRequestWithTime::new_with_epoch_time(
                SimCtrlRequest::Ping,
            ))
            .expect("sending request failed");
        std::thread::sleep(Duration::from_millis(SERVER_WAIT_TIME_MS));

        for _ in 0..3 {
            udp_testbench.check_next_sim_reply(&sim_reply);
        }
        udp_testbench.check_no_sim_reply_available();
        udp_testbench.stop();
        server_thread.join().unwrap();
    }
}
