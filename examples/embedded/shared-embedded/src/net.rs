use core::cell::Cell;

use embassy_futures::select::{Either3, select3};
use embassy_net::{
    IpEndpoint,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_stm32::{eth, peripherals};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Timer;

use crate::tmtc::{TcSender, TmReceiver};

const PORT: u16 = 7301;
const MTU: usize = 1500;

/// Locally administered MAC address
pub const MAC_ADDRESS: [u8; 6] = [0x02, 0x00, 0x11, 0x22, 0x33, 0x44];

pub static LAST_SENDER: Mutex<CriticalSectionRawMutex, Cell<Option<core::net::Ipv4Addr>>> =
    Mutex::new(Cell::new(None));

pub type Device = eth::Ethernet<
    'static,
    peripherals::ETH,
    eth::GenericPhy<eth::Sma<'static, peripherals::ETH_SMA>>,
>;

pub async fn net_stack_task(runner: &mut embassy_net::Runner<'static, Device>) -> ! {
    runner.run().await
}

pub async fn udp_task(stack: embassy_net::Stack<'static>, tc_tx: TcSender, tm_rx: TmReceiver) {
    // Task futures are allocated statically, so these buffers do not live on the stack.
    let mut rx_udp_meta = [PacketMetadata::EMPTY; 8];
    let mut tx_udp_meta = [PacketMetadata::EMPTY; 8];
    let mut rx_udp_buf = [0; MTU];
    let mut tx_udp_buf = [0; MTU];
    let mut rx_buffer = [0; MTU];

    loop {
        stack.wait_link_up().await;
        defmt::info!("Network link is up");

        // Ensure DHCP configuration is up before trying connect
        stack.wait_config_up().await;
        defmt::info!("Network task initialized, config: {}", stack.config_v4());

        let mut udp = UdpSocket::new(
            stack,
            &mut rx_udp_meta,
            &mut rx_udp_buf,
            &mut tx_udp_meta,
            &mut tx_udp_buf,
        );
        if let Err(e) = udp.bind(PORT) {
            defmt::error!("Failed to bind UDP socket: {}", e);
            Timer::after_secs(1).await;
            continue;
        }
        defmt::info!("UDP socket bound to port {}", PORT);
        let mut remote_endpoint = None;
        loop {
            match select3(
                udp.recv_from(&mut rx_buffer),
                tm_rx.receive(),
                stack.wait_link_down(),
            )
            .await
            {
                Either3::First(Ok((len, meta))) => {
                    remote_endpoint = Some(meta.endpoint);
                    let embassy_net::IpAddress::Ipv4(sender_ip) = meta.endpoint.addr;

                    LAST_SENDER.lock(|val| {
                        val.set(Some(sender_ip));
                    });

                    defmt::debug!("UDP RX {}, Meta: {}", len, meta);
                    tc_tx.send(rx_buffer[0..len].to_vec()).await;
                    // Incoming TCs take priority in the select. Draining TM here prevents a burst
                    // of TCs from overflowing the TM queue. This could lead to a task deadlock
                    // where each task is waiting on each other.
                    while let Ok(packet) = tm_rx.try_receive() {
                        handle_tm(&packet, &mut udp, &remote_endpoint).await;
                    }
                }
                Either3::First(Err(e)) => {
                    defmt::warn!("udp receive error: {}", e);
                    Timer::after_millis(100).await;
                }
                // TM is only generated as a response to a TC, so the endpoint is usually known.
                Either3::Second(packet) => handle_tm(&packet, &mut udp, &remote_endpoint).await,
                Either3::Third(()) => {
                    defmt::warn!("Network link is down");
                    break;
                }
            }
        }
    }
}

async fn handle_tm(
    packet: &[u8],
    udp_socket: &mut UdpSocket<'_>,
    remote_endpoint: &Option<IpEndpoint>,
) {
    match remote_endpoint {
        Some(endpoint) => match udp_socket.send_to(packet, *endpoint).await {
            Ok(_) => {
                defmt::debug!("UDP TX: {} bytes to: {}", packet.len(), endpoint)
            }
            Err(e) => defmt::warn!("udp send error: {}", e),
        },
        None => defmt::warn!("dropping TM, no remote endpoint known"),
    };
}
