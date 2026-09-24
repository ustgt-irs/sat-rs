use std::{net::UdpSocket, time::Duration};

use anyhow::{Context as _, bail};
use clap::Parser;
use embedded_client::setup_logger;
use embedded_types::{TmHeader, stm32h7};
use spacepackets::{CcsdsPacketCreatorOwned, CcsdsPacketReader, SpHeader};
use tmtc_utils::transport::udp::PacketTransportUdp;

#[derive(Parser, Debug)]
struct Cli {
    #[arg(short, long)]
    ping: bool,

    /// Set frequency in milliseconds.
    #[arg(short, long)]
    set_led_frequency: Option<u32>,

    /// UDP address to bind to.
    #[arg(short, long)]
    udp_addr: Option<std::net::SocketAddr>,
}

fn main() -> anyhow::Result<()> {
    setup_logger().expect("failed to initialize logger");
    println!("-- STM32H7 TMTC client --");
    let cli = Cli::parse();
    let config = embedded_client::Config::new_from_file();
    let mut udp_addr = cli.udp_addr;
    if udp_addr.is_none() {
        udp_addr = config.interface.udp_addr;
    }
    if udp_addr.is_none() {
        bail!("UDP address not specified in config.toml or via command line");
    }
    let udp_addr = udp_addr.unwrap();
    log::info!("binding to UDP address: {}", udp_addr);
    let local_socket = UdpSocket::bind("0.0.0.0:0").expect("failed to bind UDP socket");
    let mut transport = PacketTransportUdp::new(local_socket, udp_addr)
        .with_context(|| "crateing UDP transport failed")?;

    if cli.ping {
        let tc = create_stm32h7_tc(&embedded_types::stm32h7::Request::Ping);
        log::info!(
            "Sending ping request with TC ID: {:#010x}",
            tc.ccsds_packet_id_and_psc().raw()
        );
        transport.send(&tc.to_vec()).unwrap();
    }

    if let Some(freq_ms) = cli.set_led_frequency {
        let request = stm32h7::Request::ChangeBlinkFrequency(Duration::from_millis(freq_ms as u64));
        let tc = create_stm32h7_tc(&request);
        log::info!(
            "Sending change blink frequency request {:?} with TC ID: {:#010x}",
            request,
            tc.ccsds_packet_id_and_psc().raw()
        );
        transport.send(&tc.to_vec()).unwrap();
    }

    log::info!("Waiting for response...");
    loop {
        transport
            .receive(|packet: &[u8]| {
                let reader = CcsdsPacketReader::new_with_checksum(packet);
                log::debug!("Received packet: {:?}", reader);
                if let Ok(reader) = reader {
                    let packet_data = reader.packet_data();
                    let tm_header = postcard::take_from_bytes::<TmHeader>(packet_data);
                    if let Ok((tm_header, remainder)) = tm_header {
                        let response = postcard::from_bytes::<stm32h7::Response>(remainder);
                        if let Ok(response) = response {
                            log::info!(
                                "Received TM with header: {:?} and response: {:?}",
                                tm_header,
                                response
                            );
                        } else {
                            log::error!("Failed to deserialize response: {:?}", response.err());
                        }
                    } else {
                        log::error!("Failed to deserialize TM header: {:?}", tm_header.err());
                    }
                }
            })
            .unwrap();
    }
}

fn create_stm32h7_tc(request: &stm32h7::Request) -> CcsdsPacketCreatorOwned {
    let req_raw = postcard::to_allocvec(&request).unwrap();
    let sp_header = SpHeader::new_from_apid(embedded_types::stm32h7::PUS_APID);
    CcsdsPacketCreatorOwned::new_tc_with_checksum(sp_header, &req_raw).unwrap()
}
