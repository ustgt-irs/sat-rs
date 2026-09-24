use anyhow::{Context as _, bail};
use arbitrary_int::u11;
use clap::Parser as _;
use satrs_example::config::{OBSW_SERVER_ADDR, SERVER_PORT};
use satrs_minisim::{
    SimCtrlReply, SimCtrlRequest, SimReply, SimRequest, SimRequestWithTime, acs::mgm,
    udp::SIM_CTRL_PORT,
};
use spacepackets::{CcsdsPacketIdAndPsc, SpacePacketHeader};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};
use types::{Apid, Message as _, MessageType, TcHeader, acs::mgm::request::HkRequest};

#[derive(clap::Parser)]
pub struct Cli {
    #[arg(short, long)]
    ping: bool,
    #[arg(short, long)]
    test_event: bool,

    #[command(subcommand)]
    commands: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    Mgm0(MgmArgs),
    Mgm1(MgmArgs),
    MgmAssy(MgmAssemblyArgs),
    Mgt(MgtArgs),
    AcsSubsystem(SubsystemArgs),
    EventManager(EventManagerArgs),
}

#[derive(clap::Parser)]
struct EventManagerArgs {
    #[command(subcommand)]
    action: EventFilterAction,
}

#[derive(clap::Subcommand)]
enum EventFilterAction {
    /// Enable event TM generation.
    Enable(EventFilterArgs),
    /// Disable event TM generation.
    Disable(EventFilterArgs),
}

#[derive(clap::Args)]
struct EventFilterArgs {
    #[arg(value_enum)]
    component: EventSenderSelect,
    /// Raw event ID. Without it, the filter applies to all events of the component.
    #[arg(short, long)]
    event_id: Option<u16>,
}

/// Components which emit events.
#[derive(Debug, PartialEq, Eq, Clone, Copy, clap::ValueEnum)]
enum EventSenderSelect {
    Controller,
    Mgm0,
    Mgm1,
    MgmAssy,
    Mgt,
    Pcdu,
    UdpServer,
    TcpServer,
    Ground,
}

impl From<EventSenderSelect> for types::ComponentId {
    fn from(sender: EventSenderSelect) -> Self {
        match sender {
            EventSenderSelect::Controller => types::ComponentId::Controller,
            EventSenderSelect::Mgm0 => types::ComponentId::AcsMgm0,
            EventSenderSelect::Mgm1 => types::ComponentId::AcsMgm1,
            EventSenderSelect::Mgt => types::ComponentId::AcsMgt,
            EventSenderSelect::MgmAssy => types::ComponentId::AcsMgmAssembly,
            EventSenderSelect::Pcdu => types::ComponentId::EpsPcdu,
            EventSenderSelect::UdpServer => types::ComponentId::UdpServer,
            EventSenderSelect::TcpServer => types::ComponentId::TcpServer,
            EventSenderSelect::Ground => types::ComponentId::Ground,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, clap::ValueEnum)]
enum FaultMode {
    None,
    /// SPI communication is all zeroes, modelling an unconnected sensor.
    AllZeros,
    /// SPI communication is all ones, modelling a broken sensor.
    AllOnes,
}

impl From<FaultMode> for mgm::SpiFaultMode {
    fn from(mode: FaultMode) -> Self {
        match mode {
            FaultMode::None => mgm::SpiFaultMode::None,
            FaultMode::AllZeros => mgm::SpiFaultMode::AllZeros,
            FaultMode::AllOnes => mgm::SpiFaultMode::AllOnes,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy, clap::ValueEnum)]
enum FaultKind {
    /// Cleared when the device is switched off, so a power cycle recovers from it.
    Transient,
    /// Survives power cycles.
    #[default]
    Permanent,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, clap::ValueEnum)]
enum HkSelect {
    OneShot,
    EnablePeriodic,
    DisablePeriodic,
    ModifyInterval,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, clap::ValueEnum)]
enum HealthStateSelect {
    Healthy,
    Faulty,
    PermanentFaulty,
    ExternalControl,
    NeedsRecovery,
}

impl From<HealthStateSelect> for satrs::health::HealthState {
    fn from(state: HealthStateSelect) -> Self {
        match state {
            HealthStateSelect::Healthy => satrs::health::HealthState::Healthy,
            HealthStateSelect::Faulty => satrs::health::HealthState::Faulty,
            HealthStateSelect::PermanentFaulty => satrs::health::HealthState::PermanentFaulty,
            HealthStateSelect::ExternalControl => satrs::health::HealthState::ExternalControl,
            HealthStateSelect::NeedsRecovery => satrs::health::HealthState::NeedsRecovery,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, clap::Parser)]
struct MgmArgs {
    #[arg(short, long)]
    ping: bool,
    /// Housekeeping request for the sensor data set.
    #[arg(long, value_enum)]
    hk: Option<HkSelect>,
    /// Periodic HK interval. Required for `modify-interval`, optional for `enable-periodic`.
    #[arg(long)]
    hk_interval_ms: Option<u64>,
    #[arg(short, long)]
    mode: Option<DeviceModeSelect>,
    /// Inject (or clear) an SPI bus failure on the simulated device, bypassing the OBSW.
    #[arg(long, value_enum)]
    fault: Option<FaultMode>,
    /// Whether a power cycle clears the injected SPI fault.
    #[arg(long, value_enum, default_value_t)]
    fault_kind: FaultKind,
    /// Override the device's FDIR health state, for example to clear a `Faulty` state set by
    /// the handler after the underlying issue has been fixed or worked around.
    #[arg(long, value_enum)]
    health: Option<HealthStateSelect>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, clap::Parser)]
struct MgtArgs {
    #[arg(short, long)]
    ping: bool,
    /// Housekeeping request for the status data set.
    #[arg(long, value_enum)]
    hk: Option<HkSelect>,
    /// Periodic HK interval. Required for `modify-interval`, optional for `enable-periodic`.
    #[arg(long)]
    hk_interval_ms: Option<u64>,
    #[arg(short, long)]
    mode: Option<DeviceModeSelect>,
    /// Apply a dipole, given as `x,y,z`. Only accepted in normal mode.
    #[arg(long, value_name = "X,Y,Z", value_parser = parse_dipole, allow_hyphen_values = true)]
    torque: Option<types::acs::mgt::Dipole>,
    #[arg(long, default_value_t = 1000)]
    torque_duration_ms: u64,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, clap::Parser)]
struct MgmAssemblyArgs {
    #[arg(short, long)]
    ping: bool,
    #[arg(short, long)]
    mode: Option<AssemblyModeSelect>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, clap::Parser)]
struct SubsystemArgs {
    #[arg(short, long)]
    ping: bool,
    #[arg(short, long)]
    mode: Option<SubsystemModeSelect>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, clap::ValueEnum)]
pub enum DeviceModeSelect {
    Off,
    Normal,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, clap::ValueEnum)]
pub enum AssemblyModeSelect {
    NoModeKeeping,
    Off,
    Normal,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, clap::ValueEnum)]
pub enum SubsystemModeSelect {
    Off,
    Safe,
}

fn hk_request_type(
    hk: HkSelect,
    hk_interval_ms: Option<u64>,
) -> anyhow::Result<types::HkRequestType> {
    let opt_interval = hk_interval_ms.map(Duration::from_millis);
    Ok(match hk {
        HkSelect::OneShot => types::HkRequestType::OneShot,
        HkSelect::EnablePeriodic => types::HkRequestType::EnablePeriodic(opt_interval),
        HkSelect::DisablePeriodic => types::HkRequestType::DisablePeriodic,
        HkSelect::ModifyInterval => types::HkRequestType::ModifyInterval(
            opt_interval.context("--hk-interval-ms is required for modify-interval")?,
        ),
    })
}

fn parse_dipole(value: &str) -> Result<types::acs::mgt::Dipole, String> {
    let axes: Vec<i16> = value
        .split(',')
        .map(|axis| axis.trim().parse::<i16>().map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let [x, y, z] = axes[..] else {
        return Err(format!("expected 3 values, got {}", axes.len()));
    };
    Ok(types::acs::mgt::Dipole { x, y, z })
}

fn send_mgt_request(
    client: &UdpSocket,
    addr: SocketAddr,
    request: types::acs::mgt::request::Request,
) {
    let packet = types::ccsds::CcsdsTcPacketOwned::new_with_request(
        SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
        TcHeader::new(types::ComponentId::AcsMgt, request.message_type()),
        request,
    );
    let sent_tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&packet.sp_header);
    log::info!(
        "sending MGT request {:?} with TC ID {:#010x}",
        request,
        sent_tc_id.raw()
    );
    client.send_to(&packet.to_vec(), addr).unwrap();
}

fn handle_mgt_command(client: &UdpSocket, addr: SocketAddr, args: MgtArgs) -> anyhow::Result<()> {
    use types::acs::mgt::request::{ModeRequest, Request};

    if args.ping {
        send_mgt_request(client, addr, Request::Ping);
    }
    if let Some(hk) = args.hk {
        let req_type = hk_request_type(hk, args.hk_interval_ms)?;
        send_mgt_request(client, addr, Request::Hk(req_type));
    }
    if let Some(mode) = args.mode {
        let mode = match mode {
            DeviceModeSelect::Off => types::DeviceMode::Off,
            DeviceModeSelect::Normal => types::DeviceMode::Normal,
        };
        send_mgt_request(client, addr, Request::Mode(ModeRequest::SetMode(mode)));
    }
    if let Some(dipole) = args.torque {
        let request = Request::ApplyTorque {
            dipole,
            duration: Duration::from_millis(args.torque_duration_ms),
        };
        send_mgt_request(client, addr, request);
    }
    Ok(())
}

fn handle_mgm_command(
    client: &UdpSocket,
    addr: SocketAddr,
    target_id: types::ComponentId,
    args: MgmArgs,
) -> anyhow::Result<()> {
    if let Some(mode) = args.fault {
        inject_mgm_failure(
            target_id,
            mgm::SpiFault {
                mode: mode.into(),
                cleared_by_power_cycle: args.fault_kind == FaultKind::Transient,
            },
        )?;
    }
    if args.ping {
        let request = types::ccsds::CcsdsTcPacketOwned::new_with_request(
            SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
            TcHeader::new(target_id, types::MessageType::Ping),
            types::acs::mgm::request::Request::Ping,
        );
        let sent_tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&request.sp_header);
        log::info!(
            "sending {:?} ping request with TC ID {:#010x}",
            target_id,
            sent_tc_id.raw()
        );
        let request_packet = request.to_vec();
        client.send_to(&request_packet, addr).unwrap();
    }
    if let Some(hk) = args.hk {
        let req_type = hk_request_type(hk, args.hk_interval_ms)?;
        let request = types::ccsds::CcsdsTcPacketOwned::new_with_request(
            SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
            TcHeader::new(target_id, types::MessageType::Hk),
            types::acs::mgm::request::Request::Hk(HkRequest {
                id: types::acs::mgm::request::HkId::Sensor,
                req_type,
            }),
        );
        let sent_tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&request.sp_header);
        log::info!(
            "sending {:?} HK request with TC ID {:#010x}",
            target_id,
            sent_tc_id.raw()
        );
        let request_packet = request.to_vec();
        client.send_to(&request_packet, addr).unwrap();
    }
    if let Some(mode) = args.mode {
        let dev_mode = match mode {
            DeviceModeSelect::Off => types::DeviceMode::Off,
            DeviceModeSelect::Normal => types::DeviceMode::Normal,
        };

        let request = types::ccsds::CcsdsTcPacketOwned::new_with_request(
            SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
            TcHeader::new(target_id, types::MessageType::Mode),
            types::acs::mgm::request::Request::Mode(
                types::acs::mgm::request::ModeRequest::SetMode(dev_mode),
            ),
        );
        let sent_tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&request.sp_header);
        log::info!(
            "sending {:?} HK request with TC ID {:#010x}",
            target_id,
            sent_tc_id.raw()
        );
        let request_packet = request.to_vec();
        client.send_to(&request_packet, addr).unwrap();
    }
    if let Some(health) = args.health {
        let request = types::ccsds::CcsdsTcPacketOwned::new_with_request(
            SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
            TcHeader::new(target_id, types::MessageType::Health),
            types::acs::mgm::request::Request::Health(
                types::acs::mgm::request::HealthRequest::SetHealth(health.into()),
            ),
        );
        let sent_tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&request.sp_header);
        log::info!(
            "sending {:?} set-health request with TC ID {:#010x}",
            target_id,
            sent_tc_id.raw()
        );
        let request_packet = request.to_vec();
        client.send_to(&request_packet, addr).unwrap();
    }
    Ok(())
}

fn handle_event_manager_command(client: &UdpSocket, addr: SocketAddr, args: EventManagerArgs) {
    use types::event_manager::request::Request;

    let request = match args.action {
        EventFilterAction::Enable(filter) => match filter.event_id {
            Some(event_id) => Request::EnableEvent {
                sender_id: filter.component.into(),
                event_id,
            },
            None => Request::EnableComponent(filter.component.into()),
        },
        EventFilterAction::Disable(filter) => match filter.event_id {
            Some(event_id) => Request::DisableEvent {
                sender_id: filter.component.into(),
                event_id,
            },
            None => Request::DisableComponent(filter.component.into()),
        },
    };
    let request_packet = types::ccsds::CcsdsTcPacketOwned::new_with_request(
        SpacePacketHeader::new_from_apid(u11::new(Apid::Tmtc as u16)),
        TcHeader::new(types::ComponentId::EventManager, MessageType::Event),
        request,
    );
    let sent_tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&request_packet.sp_header);
    log::info!(
        "sending event manager request {:?} with TC ID {:#010x}",
        request,
        sent_tc_id.raw()
    );
    client.send_to(&request_packet.to_vec(), addr).unwrap();
}

fn setup_logger(level: log::LevelFilter) -> Result<(), fern::InitError> {
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{} {} {}] {}",
                humantime::format_rfc3339_seconds(SystemTime::now()),
                record.level(),
                record.target(),
                message
            ))
        })
        .level(level)
        .chain(std::io::stdout())
        .chain(fern::log_file("output.log")?)
        .apply()?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    setup_logger(log::LevelFilter::Debug).unwrap();
    let kill_signal = Arc::new(AtomicBool::new(false));
    let ctrl_kill_signal = kill_signal.clone();
    ctrlc::set_handler(move || ctrl_kill_signal.store(true, Ordering::Relaxed)).unwrap();
    let cli = Cli::parse();

    let addr = SocketAddr::new(IpAddr::V4(OBSW_SERVER_ADDR), SERVER_PORT);
    let client = UdpSocket::bind("127.0.0.1:7302").expect("Connecting to UDP server failed");
    client.set_nonblocking(true)?;
    client.set_read_timeout(Some(Duration::from_millis(200)))?;

    if cli.ping {
        let request = types::ccsds::CcsdsTcPacketOwned::new_with_request(
            SpacePacketHeader::new_from_apid(u11::new(Apid::Tmtc as u16)),
            TcHeader::new(types::ComponentId::Controller, types::MessageType::Ping),
            types::control::request::Request::Ping,
        );
        let sent_tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&request.sp_header);
        log::info!("sending ping request with TC ID {:#010x}", sent_tc_id.raw());
        let request_packet = request.to_vec();
        client.send_to(&request_packet, addr).unwrap();
    }
    if cli.test_event {
        let request = types::ccsds::CcsdsTcPacketOwned::new_with_request(
            SpacePacketHeader::new_from_apid(u11::new(Apid::Tmtc as u16)),
            TcHeader::new(types::ComponentId::Controller, types::MessageType::Event),
            types::control::request::Request::TestEvent,
        );
        let sent_tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&request.sp_header);
        log::info!(
            "sending event request with TC ID {:#010x}",
            sent_tc_id.raw()
        );
        let request_packet = request.to_vec();
        client.send_to(&request_packet, addr).unwrap();
    }
    if let Some(cmd) = cli.commands {
        match cmd {
            Commands::Mgm0(args) => {
                handle_mgm_command(&client, addr, types::ComponentId::AcsMgm0, args)?
            }
            Commands::Mgm1(args) => {
                handle_mgm_command(&client, addr, types::ComponentId::AcsMgm1, args)?
            }
            Commands::Mgt(args) => handle_mgt_command(&client, addr, args)?,
            Commands::MgmAssy(mgm_assembly_args) => {
                let target_id = types::ComponentId::AcsMgmAssembly;
                if mgm_assembly_args.ping {
                    let request = types::ccsds::CcsdsTcPacketOwned::new_with_request(
                        SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
                        TcHeader::new(target_id, types::MessageType::Ping),
                        types::acs::mgm::request::Request::Ping,
                    );
                    let sent_tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&request.sp_header);
                    log::info!(
                        "sending {:?} ping request with TC ID {:#010x}",
                        target_id,
                        sent_tc_id.raw()
                    );
                    let request_packet = request.to_vec();
                    client.send_to(&request_packet, addr).unwrap();
                }
                if let Some(mode) = mgm_assembly_args.mode {
                    let assembly_mode = match mode {
                        AssemblyModeSelect::NoModeKeeping => {
                            types::acs::mgm_assembly::Mode::NoModeKeeping
                        }
                        AssemblyModeSelect::Off => {
                            types::acs::mgm_assembly::Mode::Device(types::DeviceMode::Off)
                        }
                        AssemblyModeSelect::Normal => {
                            types::acs::mgm_assembly::Mode::Device(types::DeviceMode::Normal)
                        }
                    };

                    let request = types::ccsds::CcsdsTcPacketOwned::new_with_request(
                        SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
                        TcHeader::new(target_id, types::MessageType::Mode),
                        types::acs::mgm_assembly::request::Request::Mode(
                            types::acs::mgm_assembly::request::ModeRequest::SetMode(assembly_mode),
                        ),
                    );
                    let sent_tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&request.sp_header);
                    log::info!(
                        "sending {:?} HK request with TC ID {:#010x}",
                        target_id,
                        sent_tc_id.raw()
                    );
                    let request_packet = request.to_vec();
                    client.send_to(&request_packet, addr).unwrap();
                }
            }
            Commands::AcsSubsystem(subsystem_args) => {
                let target_id = types::ComponentId::AcsSubsystem;
                if subsystem_args.ping {
                    let request = types::ccsds::CcsdsTcPacketOwned::new_with_request(
                        SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
                        TcHeader::new(target_id, types::MessageType::Ping),
                        types::acs::subsystem::request::Request::Ping,
                    );
                    let sent_tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&request.sp_header);
                    log::info!(
                        "sending {:?} ping request with TC ID {:#010x}",
                        target_id,
                        sent_tc_id.raw()
                    );
                    let request_packet = request.to_vec();
                    client.send_to(&request_packet, addr).unwrap();
                }
                if let Some(mode) = subsystem_args.mode {
                    let subsystem_mode = match mode {
                        SubsystemModeSelect::Off => types::acs::subsystem::Mode::Off,
                        SubsystemModeSelect::Safe => types::acs::subsystem::Mode::Safe,
                    };

                    let request = types::ccsds::CcsdsTcPacketOwned::new_with_request(
                        SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
                        TcHeader::new(target_id, types::MessageType::Mode),
                        types::acs::subsystem::request::Request::Mode(
                            types::acs::subsystem::request::ModeRequest::SetMode(subsystem_mode),
                        ),
                    );
                    let sent_tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&request.sp_header);
                    log::info!(
                        "sending {:?} mode request with TC ID {:#010x}",
                        target_id,
                        sent_tc_id.raw()
                    );
                    let request_packet = request.to_vec();
                    client.send_to(&request_packet, addr).unwrap();
                }
            }
            Commands::EventManager(args) => handle_event_manager_command(&client, addr, args),
        }
    }

    let mut recv_buf: Box<[u8; 2048]> = Box::new([0; 2048]);
    log::info!("entering listening loop");
    loop {
        if kill_signal.load(std::sync::atomic::Ordering::Relaxed) {
            log::info!("received kill signal, exiting");
            break;
        }
        match client.recv(recv_buf.as_mut_slice()) {
            Ok(received_bytes) => handle_raw_tm_packet(&recv_buf.as_slice()[0..received_bytes])?,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut
                {
                    continue;
                }
                log::warn!("UDP reception error: {}", e)
            }
        }
    }
    Ok(())
}

/// Injects the given SPI fault directly into minisim's MGM model, bypassing the OBSW.
///
/// Confirms the simulator is actually reachable first (same ping/pong check the OBSW's own
/// internal sim client does, see `SimClientUdp::attempt_connection`), since a fire-and-forget
/// UDP send would otherwise silently do nothing if minisim is not running.
fn inject_mgm_failure(target_id: types::ComponentId, fault: mgm::SpiFault) -> anyhow::Result<()> {
    let sim_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), SIM_CTRL_PORT);
    let sim_socket = UdpSocket::bind("127.0.0.1:0")?;
    sim_socket.set_read_timeout(Some(Duration::from_millis(200)))?;

    let mut reply_buf = [0u8; 4096];
    let ping = SimRequestWithTime::new_with_epoch_time(SimCtrlRequest::Ping);
    sim_socket.send_to(&serde_json::to_vec(&ping)?, sim_addr)?;
    match sim_socket.recv(&mut reply_buf) {
        Ok(len) => {
            let reply: SimReply = serde_json::from_slice(&reply_buf[..len])?;
            if reply != SimReply::SimCtrl(SimCtrlReply::Pong) {
                bail!("unexpected reply while checking minisim connectivity: {reply:?}");
            }
        }
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            bail!("minisim not reachable at {sim_addr} (ping timed out) - is it running?");
        }
        Err(e) => return Err(e.into()),
    }

    let id = match target_id {
        types::ComponentId::AcsMgm0 => mgm::Id::Mgm0,
        types::ComponentId::AcsMgm1 => mgm::Id::Mgm1,
        _ => bail!("SPI fault injection is not supported for {target_id:?}"),
    };
    let request = SimRequestWithTime::new_with_epoch_time(SimRequest::Mgm {
        id,
        request: mgm::Request::SetSpiFault(fault),
    });
    sim_socket.send_to(&serde_json::to_vec(&request)?, sim_addr)?;
    log::info!("injected SPI fault {fault:?} into minisim {target_id:?}");
    Ok(())
}

/// Each component has its own event type, so the sender ID determines how to decode the event.
fn handle_event(sender_id: types::ComponentId, data: &[u8]) {
    fn log_event<E: serde::de::DeserializeOwned + core::fmt::Debug>(
        sender_id: types::ComponentId,
        data: &[u8],
    ) {
        match postcard::from_bytes::<E>(data) {
            Ok(event) => log::info!("Received event from {:?}: {:?}", sender_id, event),
            Err(e) => log::warn!("Failed to deserialize event from {:?}: {}", sender_id, e),
        }
    }
    match sender_id {
        types::ComponentId::Controller => log_event::<types::Event>(sender_id, data),
        types::ComponentId::AcsMgm0 | types::ComponentId::AcsMgm1 => {
            log_event::<types::acs::mgm::Event>(sender_id, data)
        }
        types::ComponentId::AcsMgmAssembly => {
            log_event::<types::acs::mgm_assembly::Event>(sender_id, data)
        }
        types::ComponentId::AcsMgt => log_event::<types::acs::mgt::Event>(sender_id, data),
        types::ComponentId::EpsPcdu => log_event::<types::pcdu::Event>(sender_id, data),
        // TC source events are sent with the ID of the packet source.
        types::ComponentId::UdpServer
        | types::ComponentId::TcpServer
        | types::ComponentId::Ground => log_event::<types::tmtc::Event>(sender_id, data),
        _ => log::warn!(
            "Received event from {:?} with unknown event type",
            sender_id
        ),
    }
}

fn handle_raw_tm_packet(data: &[u8]) -> anyhow::Result<()> {
    match spacepackets::CcsdsPacketReader::new_with_checksum(data) {
        Ok(packet) => {
            let tm_header_result = postcard::take_from_bytes::<types::TmHeader>(packet.user_data());
            if let Err(e) = tm_header_result {
                bail!("Failed to deserialize TM header: {}", e);
            }
            let (tm_header, remainder) = tm_header_result.unwrap();
            if let Some(tc_id) = tm_header.tc_id {
                log::info!(
                    "Received TM with APID {} and from sender {:?} for TC ID {:#010x}",
                    packet.apid(),
                    tm_header.sender_id,
                    tc_id.raw()
                );
            }
            if tm_header.message_type == MessageType::Event {
                handle_event(tm_header.sender_id, remainder);
                return Ok(());
            }
            match tm_header.sender_id {
                types::ComponentId::EpsPcdu => {
                    let response =
                        postcard::from_bytes::<types::pcdu::response::Response>(remainder);
                    log::info!("Received response from PCDU: {:?}", response.unwrap());
                }
                types::ComponentId::Controller => {
                    let response =
                        postcard::from_bytes::<types::control::response::Response>(remainder);
                    log::info!("Received response from controller: {:?}", response.unwrap());
                }
                types::ComponentId::AcsMgmAssembly => {
                    let response = postcard::from_bytes::<
                        types::acs::mgm_assembly::response::Response,
                    >(remainder);
                    log::info!(
                        "Received response from MGM Assembly: {:?}",
                        response.unwrap()
                    );
                }
                types::ComponentId::AcsMgm0 => {
                    let response =
                        postcard::from_bytes::<types::acs::mgm::response::Response>(remainder);
                    log::info!("Received response from MGM0: {:?}", response.unwrap());
                }
                types::ComponentId::AcsMgm1 => {
                    let response =
                        postcard::from_bytes::<types::acs::mgm::response::Response>(remainder);
                    log::info!("Received response from MGM1: {:?}", response.unwrap());
                }
                types::ComponentId::AcsSubsystem => {
                    let response = postcard::from_bytes::<types::acs::subsystem::response::Response>(
                        remainder,
                    );
                    log::info!(
                        "Received response from ACS subsystem: {:?}",
                        response.unwrap()
                    );
                }
                types::ComponentId::EpsSubsystem => todo!(),
                types::ComponentId::UdpServer => todo!(),
                types::ComponentId::TcpServer => todo!(),
                types::ComponentId::Ground => todo!(),
                types::ComponentId::EventManager => {
                    let response =
                        postcard::from_bytes::<types::event_manager::response::Response>(remainder);
                    log::info!(
                        "Received response from event manager: {:?}",
                        response.unwrap()
                    );
                }
                types::ComponentId::AcsController => todo!(),
                types::ComponentId::AcsMgt => {
                    let response =
                        postcard::from_bytes::<types::acs::mgt::response::Response>(remainder);
                    log::info!("Received response from MGT: {:?}", response.unwrap());
                }
            }
        }
        Err(_) => todo!(),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dipole() {
        assert_eq!(
            parse_dipole("-200, 200,1000"),
            Ok(types::acs::mgt::Dipole {
                x: -200,
                y: 200,
                z: 1000
            })
        );
        assert!(parse_dipole("1,2").is_err());
        assert!(parse_dipole("1,2,3,4").is_err());
    }

    #[test]
    fn test_negative_torque_argument() {
        let cli = Cli::try_parse_from(["client", "mgt", "--torque", "-200,200,1000"]).unwrap();
        let Some(Commands::Mgt(args)) = cli.commands else {
            panic!("expected mgt subcommand");
        };
        assert_eq!(args.torque.map(|dipole| dipole.x), Some(-200));
    }
}
