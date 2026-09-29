//! UDP client for the minisim, which simulates the devices of the OBSW.

use core::net::{Ipv4Addr, SocketAddrV4};

use defmt::Debug2Format;
use embassy_futures::select::{Either, select};
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_sync::watch::Watch;
use embassy_time::{Duration, WithTimeout as _};
use minisim_types::udp::SIM_CTRL_PORT;
use minisim_types::{SimCtrlReply, SimCtrlRequest, SimReply, SimRequestWithTime};

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

pub async fn sim_client_task(stack: embassy_net::Stack<'static>) {
    let mut rx_meta = [PacketMetadata::EMPTY; 4];
    let mut tx_meta = [PacketMetadata::EMPTY; 4];
    let mut rx_buf = [0; MAX_PACKET_LEN];
    let mut tx_buf = [0; MAX_PACKET_LEN];

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
        loop {
            if let Some(addr) = sim_addr {
                let connected = check_connection(&udp, addr).await;
                if connected {
                    defmt::info!("Connected to simulator at {}", Debug2Format(&addr));
                } else {
                    defmt::warn!(
                        "Simulator at {} not reachable, using dummy interfaces",
                        Debug2Format(&addr)
                    );
                }
                sim_available.send(connected);
            }
            match select(SIM_HOST.wait(), stack.wait_link_down()).await {
                Either::First(ip_addr) => {
                    sim_addr = Some(SocketAddrV4::new(ip_addr, SIM_CTRL_PORT));
                }
                Either::Second(()) => {
                    sim_available.send(false);
                    break;
                }
            }
        }
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
/// simulator host is resolved.
async fn check_connection(udp: &UdpSocket<'_>, sim_addr: SocketAddrV4) -> bool {
    let mut tx_buf = [0; MAX_PACKET_LEN];
    let mut rx_buf = [0; MAX_PACKET_LEN];
    let request = SimRequestWithTime::new_with_epoch_time(SimCtrlRequest::Ping);
    let ping = match postcard::to_slice(&request, &mut tx_buf) {
        Ok(ping) => ping,
        Err(e) => {
            defmt::error!("Failed to serialize simulator ping: {}", Debug2Format(&e));
            return false;
        }
    };
    for _ in 0..PING_ATTEMPTS {
        if let Err(e) = udp.send_to(ping, sim_addr).await {
            defmt::warn!("Failed to send simulator ping: {}", e);
            continue;
        }
        if let Ok(Ok((len, _))) = udp.recv_from(&mut rx_buf).with_timeout(PING_TIMEOUT).await
            && matches!(
                postcard::from_bytes::<SimReply>(&rx_buf[..len]),
                Ok(SimReply::SimCtrl(SimCtrlReply::Pong))
            )
        {
            return true;
        }
    }
    false
}
