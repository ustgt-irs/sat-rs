use std::time::Duration;

use anyhow::bail;
use clap::Parser;
use cobs::CobsDecoderOwned;
use embedded_client::setup_logger;
use embedded_types::stm32f3;
use spacepackets::{CcsdsPacketCreatorOwned, CcsdsPacketReader, SpHeader};
use tmtc_utils::transport::serial::PacketTransportSerialCobs;

#[derive(Parser, Debug)]
struct Cli {
    #[arg(short, long)]
    ping: bool,

    /// Set frequency in milliseconds.
    #[arg(short, long)]
    set_led_frequency: Option<u32>,
}

fn main() -> anyhow::Result<()> {
    setup_logger().expect("failed to initialize logger");
    println!("-- STM32F3 TMTC client --");
    let cli = Cli::parse();
    let config = embedded_client::Config::new_from_file();

    if config.interface.serial_port.is_none() {
        bail!("Serial port not specified in configuration file.");
    }
    let serial_port = config.interface.serial_port.as_ref().unwrap();
    let serial = serialport::new(serial_port, 115200)
        .open()
        .expect("opening serial port failed");
    let mut transport = PacketTransportSerialCobs::new(serial, CobsDecoderOwned::new(1024));

    if cli.ping {
        let tc = create_stm32f3_tc(&embedded_types::stm32f3::Request::Ping);
        log::info!(
            "Sending ping request with TC ID: {:#010x}",
            tc.ccsds_packet_id_and_psc().raw()
        );
        transport.send(&tc.to_vec()).unwrap();
    }

    if let Some(freq_ms) = cli.set_led_frequency {
        let request = stm32f3::Request::ChangeBlinkFrequency(Duration::from_millis(freq_ms as u64));
        let tc = create_stm32f3_tc(&request);
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
                log::info!("Received packet: {:?}", reader);
            })
            .unwrap();
    }
}

fn create_stm32f3_tc(request: &stm32f3::Request) -> CcsdsPacketCreatorOwned {
    let req_raw = postcard::to_allocvec(&request).unwrap();
    let sp_header = SpHeader::new_from_apid(embedded_types::stm32f3::PUS_APID);
    CcsdsPacketCreatorOwned::new_tc_with_checksum(sp_header, &req_raw).unwrap()
}
