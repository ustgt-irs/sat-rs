//! UDP client for the minisim, which simulates the devices of the OBSW.

use core::net::{Ipv4Addr, SocketAddrV4};

use embassy_futures::select::{Either4, select4};
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender};
use embassy_sync::signal::Signal;
use embassy_sync::watch::Watch;
use embassy_time::{Duration, Instant, WithTimeout as _};
use minisim_types::eps::PcduReply;
use minisim_types::udp::SIM_CTRL_PORT;
use minisim_types::{SimCtrlReply, SimCtrlRequest, SimReply, SimRequestWithTime};

use crate::config;

/// Set by a sim connect request. Each new address triggers a connection attempt.
pub static SIM_HOST: Signal<CriticalSectionRawMutex, Ipv4Addr> = Signal::new();

/// IP address of the host running the minisim, set at build time. Without it, no connection
/// to a simulator is attempted until a sim connect request is received.
const SIM_FROM_ENV: Option<&str> = option_env!("SIM_IP_ADDR");

const PING_ATTEMPTS: usize = 3;
const PING_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_PACKET_LEN: usize = 1024;

/// Result of the connection check. Device handlers use it to pick either the simulator or a
/// dummy interface.
pub static SIM_AVAILABLE: Watch<CriticalSectionRawMutex, bool, 4> = Watch::new();

// Critical section mutexes, because RTIC tasks with different priorities can preempt each other.
pub type SimRequestChannel =
    Channel<CriticalSectionRawMutex, SimRequestWithTime, { config::SIM_REQUEST_QUEUE_DEPTH }>;
pub type SimRequestSender = Sender<
    'static,
    CriticalSectionRawMutex,
    SimRequestWithTime,
    { config::SIM_REQUEST_QUEUE_DEPTH },
>;
pub type SimRequestReceiver = Receiver<
    'static,
    CriticalSectionRawMutex,
    SimRequestWithTime,
    { config::SIM_REQUEST_QUEUE_DEPTH },
>;

pub type PcduSimReplyChannel =
    Channel<CriticalSectionRawMutex, PcduReply, { config::PCDU_SIM_REPLY_QUEUE_DEPTH }>;
pub type PcduSimReplySender =
    Sender<'static, CriticalSectionRawMutex, PcduReply, { config::PCDU_SIM_REPLY_QUEUE_DEPTH }>;
pub type PcduSimReplyReceiver =
    Receiver<'static, CriticalSectionRawMutex, PcduReply, { config::PCDU_SIM_REPLY_QUEUE_DEPTH }>;

/// One sender for each device handler which receives simulator replies.
#[derive(Clone, Copy)]
pub struct SimReplySenders {
    pub pcdu: PcduSimReplySender,
}

impl SimReplySenders {
    /// A full reply queue drops the reply, so a slow handler can not block the other handlers.
    fn route(&self, reply: SimReply) {
        let component = reply.component();
        let result = match reply {
            SimReply::Pcdu(reply) => self.pcdu.try_send(reply).map_err(|_| ()),
            _ => {
                defmt::warn!("No recipient for simulator reply from {}", &component);
                return;
            }
        };
        if result.is_err() {
            defmt::warn!(
                "Reply queue for {} full, dropping simulator reply",
                &component
            );
        }
    }
}

#[derive(Clone, Copy)]
pub struct SimClientEndpoints {
    pub request_rx: SimRequestReceiver,
    pub reply_senders: SimReplySenders,
}

/// Forwards requests to the simulator and routes the replies to the device handlers. Requests
/// are dropped while no simulator is connected.
pub async fn sim_client_task(stack: embassy_net::Stack<'static>, endpoints: SimClientEndpoints) {
    let SimClientEndpoints {
        request_rx,
        reply_senders,
    } = endpoints;
    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx_buf = [0; MAX_PACKET_LEN];
    let mut tx_buf = [0; MAX_PACKET_LEN];
    let mut request_buf = [0; MAX_PACKET_LEN];
    let mut reply_buf = [0; MAX_PACKET_LEN];

    let sim_available = SIM_AVAILABLE.sender();
    sim_available.send(false);

    // Bound to an ephemeral port without a fixed local address, so the socket survives a new
    // DHCP lease and does not need to be recreated after a link loss.
    let mut udp = UdpSocket::new(stack, &mut rx_meta, &mut rx_buf, &mut tx_meta, &mut tx_buf);
    if let Err(e) = udp.bind(0) {
        defmt::error!("Failed to bind simulator UDP socket: {}", e);
        return;
    }
    let mut sim_addr = sim_addr_from_env();

    loop {
        stack.wait_link_up().await;
        stack.wait_config_up().await;
        let mut check_pending = true;
        let mut connected = false;
        loop {
            if check_pending && let Some(addr) = sim_addr {
                connected = check_connection(&udp, addr, &reply_senders).await;
                if connected {
                    defmt::info!("Connected to simulator at {}", &addr);
                } else {
                    defmt::warn!(
                        "Simulator at {} not reachable, using dummy interfaces",
                        &addr
                    );
                }
                sim_available.send(connected);
            }
            check_pending = false;
            match select4(
                SIM_HOST.wait(),
                stack.wait_link_down(),
                request_rx.receive(),
                udp.recv_from(&mut reply_buf),
            )
            .await
            {
                Either4::First(ip_addr) => {
                    sim_addr = Some(SocketAddrV4::new(ip_addr, SIM_CTRL_PORT));
                    check_pending = true;
                }
                Either4::Second(()) => {
                    sim_available.send(false);
                    break;
                }
                Either4::Third(request) => {
                    if connected && let Some(addr) = sim_addr {
                        send_request(&udp, addr, &request, &mut request_buf).await;
                    }
                }
                Either4::Fourth(Ok((len, _))) => {
                    match postcard::from_bytes::<SimReply>(&reply_buf[..len]) {
                        Ok(reply) => reply_senders.route(reply),
                        Err(e) => {
                            defmt::warn!("Invalid simulator reply: {}", &e)
                        }
                    }
                }
                Either4::Fourth(Err(e)) => {
                    defmt::warn!("Receiving simulator reply failed: {}", e)
                }
            }
        }
    }
}

async fn send_request(
    udp: &UdpSocket<'_>,
    sim_addr: SocketAddrV4,
    request: &SimRequestWithTime,
    buf: &mut [u8],
) {
    match postcard::to_slice(request, buf) {
        Ok(raw) => {
            if let Err(e) = udp.send_to(raw, sim_addr).await {
                defmt::warn!("Failed to send simulator request: {}", e);
            }
        }
        Err(e) => defmt::error!("Failed to serialize simulator request: {}", &e),
    }
}

fn sim_addr_from_env() -> Option<SocketAddrV4> {
    let ip_addr = SIM_FROM_ENV?;
    match ip_addr.parse::<Ipv4Addr>() {
        Ok(ip_addr) => Some(SocketAddrV4::new(ip_addr, SIM_CTRL_PORT)),
        Err(_) => {
            defmt::error!("Invalid simulator IP address {}", ip_addr);
            None
        }
    }
}

/// Uses several attempts, because the first packet can get lost while the MAC address of the
/// simulator host is resolved. Device replies which arrive in the meantime are still routed.
async fn check_connection(
    udp: &UdpSocket<'_>,
    sim_addr: SocketAddrV4,
    reply_senders: &SimReplySenders,
) -> bool {
    let mut tx_buf = [0; MAX_PACKET_LEN];
    let mut rx_buf = [0; MAX_PACKET_LEN];
    let request = SimRequestWithTime::new_with_epoch_time(SimCtrlRequest::Ping);
    let ping = match postcard::to_slice(&request, &mut tx_buf) {
        Ok(ping) => ping,
        Err(e) => {
            defmt::error!("Failed to serialize simulator ping: {}", &e);
            return false;
        }
    };
    for _ in 0..PING_ATTEMPTS {
        if let Err(e) = udp.send_to(ping, sim_addr).await {
            defmt::warn!("Failed to send simulator ping: {}", e);
            continue;
        }
        let deadline = Instant::now() + PING_TIMEOUT;
        while let Ok(Ok((len, _))) = udp.recv_from(&mut rx_buf).with_deadline(deadline).await {
            match postcard::from_bytes::<SimReply>(&rx_buf[..len]) {
                Ok(SimReply::SimCtrl(SimCtrlReply::Pong)) => return true,
                Ok(reply) => reply_senders.route(reply),
                Err(e) => defmt::warn!("Invalid simulator reply: {}", &e),
            }
        }
    }
    false
}
