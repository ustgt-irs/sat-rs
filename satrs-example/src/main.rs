use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use eps::{
    PowerSwitchHelper,
    pcdu::{PcduHandler, SerialInterfaceDummy, SerialInterfaceToSim, SerialSimInterfaceWrapper},
};
use interface::{
    sim_client_udp::create_sim_client,
    tcp::{SyncTcpTmSource, TcpTask},
    udp::UdpTmtcServer,
};
use log::info;
use logger::setup_logger;
use satrs::{
    HandlingStatus,
    hal::std::{tcp_server::ServerConfig, udp_server::UdpTcServer},
    spacepackets::time::cds::CdsTime,
};
use satrs_example::{
    TmtcQueues,
    config::{
        OBSW_SERVER_ADDR, PACKET_ID_VALIDATOR, SERVER_PORT,
        tasks::{FREQ_MS_AOCS, FREQ_MS_CONTROLLER, FREQ_MS_UDP_TMTC, SIM_CLIENT_IDLE_DELAY_MS},
    },
};
use tmtc::sender::TmTcSender;
use tmtc::{tc_source::TcSourceTask, tm_sink::TmSink};
use types::{ComponentId, DeviceMode};

use crate::{
    acs::{ctrl, mgm, mgm_assembly, mgt, subsystem},
    controller::Controller,
    eps::pcdu::SwitchSet,
    event_manager::EventManager,
    interface::udp::UdpTmHandlerWithChannel,
    tmtc::tc_source::CcsdsDistributor,
};

mod acs;
mod ccsds;
mod controller;
mod device_mode;
mod eps;
mod event_manager;
mod interface;
mod logger;
mod tmtc;
fn main() {
    static KILL_SIGNAL: AtomicBool = AtomicBool::new(false);

    setup_logger().expect("setting up logging with fern failed");
    println!("Runng OBSW example");
    ctrlc::set_handler(move || {
        log::info!("Received Ctrl-C, shutting down");
        KILL_SIGNAL.store(true, Ordering::Relaxed);
    })
    .expect("Error setting Ctrl-C handler");

    let (tc_source_tx, tc_source_rx) = mpsc::sync_channel(50);
    let (tm_sink_tx, tm_sink_rx) = mpsc::sync_channel(50);
    let (tm_server_tx, tm_server_rx) = mpsc::sync_channel(50);

    let (sim_request_tx, sim_request_rx) = mpsc::channel();
    let (mgm_0_sim_reply_tx, mgm_0_sim_reply_rx) = mpsc::channel();
    let (mgm_1_sim_reply_tx, mgm_1_sim_reply_rx) = mpsc::channel();
    let (pcdu_sim_reply_tx, pcdu_sim_reply_rx) = mpsc::channel();
    let mut opt_sim_client = create_sim_client(sim_request_rx);

    let (mgm_0_handler_tc_tx, mgm_0_handler_tc_rx) = mpsc::sync_channel(10);
    let (mgm_1_handler_tc_tx, mgm_1_handler_tc_rx) = mpsc::sync_channel(10);
    let (mgm_assembly_tc_tx, mgm_assembly_tc_rx) = mpsc::sync_channel(10);
    let (acs_subsystem_tc_tx, acs_subsystem_tc_rx) = mpsc::sync_channel(10);
    let (pcdu_handler_tc_tx, pcdu_handler_tc_rx) = mpsc::sync_channel(30);
    let (controller_tc_tx, controller_tc_rx) = mpsc::sync_channel(10);
    let (event_manager_tc_tx, event_manager_tc_rx) = mpsc::sync_channel(10);

    let (mgt_request_tx, mgt_request_rx) = mpsc::sync_channel(5);
    let (mgt_report_tx, mgt_report_rx) = mpsc::sync_channel(5);

    let (acs_ctrl_request_tx, acs_ctrl_request_rx) = mpsc::sync_channel(5);
    let (acs_ctrl_response_tx, acs_ctrl_response_rx) = mpsc::sync_channel(5);

    // These message handles need to go into the MGM assembly and ACS subsystem.
    let (mgm_assembly_request_tx, mgm_assembly_request_rx) = mpsc::sync_channel(5);
    let (mgm_assembly_report_tx, mgm_assembly_report_rx) = mpsc::sync_channel(5);

    // These message handles need to go into the MGM assembly and MGM devices.
    let (mgm_0_mode_request_tx, mgm_0_mode_request_rx) = mpsc::sync_channel(5);
    let (mgm_1_mode_request_tx, mgm_1_mode_request_rx) = mpsc::sync_channel(5);
    let (mgm_0_mode_report_tx, mgm_0_mode_report_rx) = mpsc::sync_channel(5);
    let (mgm_1_mode_report_tx, mgm_1_mode_report_rx) = mpsc::sync_channel(5);

    let (pcdu_handler_mode_tx, _pcdu_handler_mode_rx) = mpsc::sync_channel(5);

    let (event_ctrl_tx, event_ctrl_rx) = mpsc::sync_channel(10);
    let (mgm_event_tx, mgm_event_rx) = mpsc::sync_channel(10);
    let (mgm_assembly_event_tx, mgm_assembly_event_rx) = mpsc::sync_channel(10);
    let (pcdu_event_tx, pcdu_event_rx) = mpsc::sync_channel(10);
    let (tc_source_event_tx, tc_source_event_rx) = mpsc::sync_channel(10);
    let mut event_manager = EventManager::new(
        event_manager_tc_rx,
        event_ctrl_rx,
        mgm_event_rx,
        mgm_assembly_event_rx,
        pcdu_event_rx,
        tc_source_event_rx,
        tm_sink_tx.clone(),
    );

    let mut controller = Controller::new(controller_tc_rx, tm_sink_tx.clone(), event_ctrl_tx);

    let ccsds_distributor = CcsdsDistributor::default();
    let mut tc_source = TcSourceTask::new(tc_source_rx, ccsds_distributor, tc_source_event_tx);
    tc_source.add_target(ComponentId::EpsPcdu, pcdu_handler_tc_tx);
    tc_source.add_target(ComponentId::Controller, controller_tc_tx);
    tc_source.add_target(ComponentId::AcsMgm0, mgm_0_handler_tc_tx);
    tc_source.add_target(ComponentId::AcsMgm1, mgm_1_handler_tc_tx);
    tc_source.add_target(ComponentId::AcsMgmAssembly, mgm_assembly_tc_tx);
    tc_source.add_target(ComponentId::AcsSubsystem, acs_subsystem_tc_tx);
    tc_source.add_target(ComponentId::EventManager, event_manager_tc_tx);

    let tc_sender = TmTcSender::Normal(tc_source_tx.clone());
    let udp_tm_handler = UdpTmHandlerWithChannel {
        tm_rx: tm_server_rx,
    };

    let sock_addr = SocketAddr::new(IpAddr::V4(OBSW_SERVER_ADDR), SERVER_PORT);
    let udp_tc_server = UdpTcServer::new(
        ComponentId::UdpServer as u32,
        sock_addr,
        2048,
        tc_sender.clone(),
    )
    .expect("creating UDP TMTC server failed");
    let mut udp_tmtc_server = UdpTmtcServer {
        udp_tc_server,
        tm_handler: udp_tm_handler.into(),
    };

    let tcp_server_cfg = ServerConfig::new(
        ComponentId::TcpServer as u32,
        sock_addr,
        Duration::from_millis(400),
        4096,
        8192,
    );
    let sync_tm_tcp_source = SyncTcpTmSource::new(200);
    let mut tcp_server = TcpTask::new(
        tcp_server_cfg,
        sync_tm_tcp_source.clone(),
        tc_sender,
        PACKET_ID_VALIDATOR.clone(),
    )
    .expect("tcp server creation failed");

    let mut tm_sink = TmSink::new(sync_tm_tcp_source, tm_sink_rx, tm_server_tx);

    let shared_switch_set = Arc::new(Mutex::new(SwitchSet::new_with_init_switches_unknown()));
    let (switch_request_tx, switch_request_rx) = mpsc::sync_channel(20);
    let switch_helper = PowerSwitchHelper::new(switch_request_tx, shared_switch_set.clone());

    // Global FDIR health table, shared by all software objects.
    let health_table = satrs::health::HealthTableMapSync::default();

    let shared_mgm_0_set = Arc::default();
    let shared_mgm_1_set = Arc::default();
    let (mgm_0_spi_interface, mgm_1_spi_interface) = if let Some(sim_client) =
        opt_sim_client.as_mut()
    {
        sim_client.add_reply_recipient(satrs_minisim::ComponentId::Mgm0Lis3Mdl, mgm_0_sim_reply_tx);
        sim_client.add_reply_recipient(satrs_minisim::ComponentId::Mgm1Lis3Mdl, mgm_1_sim_reply_tx);
        (
            mgm::SpiCommunication::Sim(mgm::SpiSimInterface {
                id: mgm::MgmId::_0,
                sim_request_tx: sim_request_tx.clone(),
                sim_reply_rx: mgm_0_sim_reply_rx,
            }),
            mgm::SpiCommunication::Sim(mgm::SpiSimInterface {
                id: mgm::MgmId::_1,
                sim_request_tx: sim_request_tx.clone(),
                sim_reply_rx: mgm_1_sim_reply_rx,
            }),
        )
    } else {
        (
            mgm::SpiCommunication::Dummy(mgm::SpiDummyInterface::default()),
            mgm::SpiCommunication::Dummy(mgm::SpiDummyInterface::default()),
        )
    };
    let mut mgm_0_handler = mgm::MgmHandlerLis3Mdl::new(
        mgm::MgmId::_0,
        TmtcQueues {
            tc_rx: mgm_0_handler_tc_rx,
            tm_tx: tm_sink_tx.clone(),
        },
        switch_helper.clone(),
        mgm_0_spi_interface,
        shared_mgm_0_set,
        mgm::ModeLeafHelper {
            request_rx: mgm_0_mode_request_rx,
            report_tx: mgm_0_mode_report_tx,
        },
        Duration::from_millis(1000),
        health_table.clone(),
        mgm_event_tx.clone(),
    );
    let mut mgm_1_handler = mgm::MgmHandlerLis3Mdl::new(
        mgm::MgmId::_1,
        TmtcQueues {
            tc_rx: mgm_1_handler_tc_rx,
            tm_tx: tm_sink_tx.clone(),
        },
        switch_helper.clone(),
        mgm_1_spi_interface,
        shared_mgm_1_set,
        mgm::ModeLeafHelper {
            request_rx: mgm_1_mode_request_rx,
            report_tx: mgm_1_mode_report_tx,
        },
        Duration::from_millis(1000),
        health_table.clone(),
        mgm_event_tx,
    );
    let mut mgm_assembly = mgm_assembly::Assembly::new(
        mgm_assembly::ParentQueueHelper {
            request_rx: mgm_assembly_request_rx,
            report_tx: mgm_assembly_report_tx,
        },
        mgm_assembly::ChildrenQueueHelper {
            request_tx_queues: [mgm_0_mode_request_tx, mgm_1_mode_request_tx],
            report_rx_queues: [mgm_0_mode_report_rx, mgm_1_mode_report_rx],
        },
        TmtcQueues {
            tc_rx: mgm_assembly_tc_rx,
            tm_tx: tm_sink_tx.clone(),
        },
        Duration::from_millis(2000),
        mgm_assembly_event_tx,
    );

    let mut acs_controller = ctrl::Controller::new(ctrl::ModeLeafHelper {
        request_rx: acs_ctrl_request_rx,
        report_tx: acs_ctrl_response_tx,
    });

    let mut acs_mgt = mgt::Mgt::new(mgt::ModeLeafHelper {
        request_rx: mgt_request_rx,
        report_tx: mgt_report_tx,
    });

    let mut acs_subsystem = subsystem::Subsystem::new(
        subsystem::ModeRequestSenders {
            mode_request_ctrl: acs_ctrl_request_tx,
            mode_request_mgm_assy: mgm_assembly_request_tx,
            mode_request_mgt: mgt_request_tx,
        },
        subsystem::ModeReportReceivers {
            mode_response_ctrl: acs_ctrl_response_rx,
            mode_response_mgm_assy: mgm_assembly_report_rx,
            mode_response_mgt: mgt_report_rx,
        },
        TmtcQueues {
            tc_rx: acs_subsystem_tc_rx,
            tm_tx: tm_sink_tx.clone(),
        },
    );

    let pcdu_serial_interface = if let Some(sim_client) = opt_sim_client.as_mut() {
        sim_client.add_reply_recipient(satrs_minisim::ComponentId::Pcdu, pcdu_sim_reply_tx);
        SerialSimInterfaceWrapper::Sim(SerialInterfaceToSim::new(
            sim_request_tx.clone(),
            pcdu_sim_reply_rx,
        ))
    } else {
        SerialSimInterfaceWrapper::Dummy(SerialInterfaceDummy::default())
    };
    let mut pcdu_handler = PcduHandler::new(
        pcdu_handler_tc_rx,
        tm_sink_tx.clone(),
        switch_request_rx,
        pcdu_serial_interface,
        shared_switch_set,
        DeviceMode::Normal,
        pcdu_event_tx,
    );

    // The PCDU is a critical component which should be in normal mode immediately.
    pcdu_handler_mode_tx
        .send(types::pcdu::request::Request::Mode(DeviceMode::Normal))
        .expect("sending initial mode request failed");

    info!("Starting TMTC and UDP task");
    let jh_udp_tmtc = thread::Builder::new()
        .name("TMTC & UDP".to_string())
        .spawn(move || {
            info!("Running UDP server on port {SERVER_PORT}");
            loop {
                if KILL_SIGNAL.load(Ordering::Relaxed) {
                    break;
                }
                udp_tmtc_server.periodic_operation();
                tc_source.periodic_operation();
                thread::sleep(Duration::from_millis(FREQ_MS_UDP_TMTC));
            }
        })
        .unwrap();

    info!("Starting TCP task");
    let jh_tcp = thread::Builder::new()
        .name("TCP".to_string())
        .spawn(move || {
            info!("Running TCP server on port {SERVER_PORT}");
            loop {
                if KILL_SIGNAL.load(Ordering::Relaxed) {
                    break;
                }
                tcp_server.periodic_operation();
            }
        })
        .unwrap();

    info!("Starting TM funnel task");
    let jh_tm_funnel = thread::Builder::new()
        .name("TM SINK".to_string())
        .spawn(move || {
            loop {
                if KILL_SIGNAL.load(Ordering::Relaxed) {
                    break;
                }
                tm_sink.operation();
            }
        })
        .unwrap();

    let mut opt_jh_sim_client = None;
    if let Some(mut sim_client) = opt_sim_client {
        info!("Starting UDP sim client task");
        opt_jh_sim_client = Some(
            thread::Builder::new()
                .name("SIM ADAPTER".to_string())
                .spawn(move || {
                    loop {
                        if KILL_SIGNAL.load(Ordering::Relaxed) {
                            break;
                        }
                        if sim_client.operation() == HandlingStatus::Empty {
                            std::thread::sleep(Duration::from_millis(SIM_CLIENT_IDLE_DELAY_MS));
                        }
                    }
                })
                .unwrap(),
        );
    }

    info!("Starting AOCS thread");
    let jh_aocs = thread::Builder::new()
        .name("AOCS".to_string())
        .spawn(move || {
            loop {
                if KILL_SIGNAL.load(Ordering::Relaxed) {
                    break;
                }
                mgm_0_handler.periodic_operation();
                mgm_1_handler.periodic_operation();
                mgm_assembly.periodic_operation();
                acs_controller.periodic_operation();
                acs_mgt.periodic_operation();
                acs_subsystem.periodic_operation();
                thread::sleep(Duration::from_millis(FREQ_MS_AOCS));
            }
        })
        .unwrap();

    info!("Starting EPS thread");
    let jh_eps = thread::Builder::new()
        .name("EPS".to_string())
        .spawn(move || {
            loop {
                if KILL_SIGNAL.load(Ordering::Relaxed) {
                    break;
                }
                // TODO: We should introduce something like a fixed timeslot helper to allow a more
                // declarative API. It would also be very useful for the AOCS task.
                //
                // TODO: The fixed timeslot handler exists.. use it.
                // TODO: Why not just use sync code in the PCDU handler, and fully delay there?
                pcdu_handler.periodic_operation(crate::eps::pcdu::OpCode::RegularOp);
                thread::sleep(Duration::from_millis(50));
                pcdu_handler.periodic_operation(crate::eps::pcdu::OpCode::PollAndRecvReplies);
                thread::sleep(Duration::from_millis(50));
                pcdu_handler.periodic_operation(crate::eps::pcdu::OpCode::PollAndRecvReplies);
                thread::sleep(Duration::from_millis(300));
            }
        })
        .unwrap();

    info!("Starting controller thread");
    let jh_controller_thread = thread::Builder::new()
        .name("CTRL".to_string())
        .spawn(move || {
            loop {
                if KILL_SIGNAL.load(Ordering::Relaxed) {
                    break;
                }
                controller.periodic_operation();
                event_manager.periodic_operation();
                thread::sleep(Duration::from_millis(FREQ_MS_CONTROLLER));
            }
        })
        .unwrap();

    jh_udp_tmtc
        .join()
        .expect("Joining UDP TMTC server thread failed");
    jh_tcp
        .join()
        .expect("Joining TCP TMTC server thread failed");
    jh_tm_funnel
        .join()
        .expect("Joining TM Funnel thread failed");
    if let Some(jh_sim_client) = opt_jh_sim_client {
        jh_sim_client
            .join()
            .expect("Joining SIM client thread failed");
    }
    jh_aocs.join().expect("Joining AOCS thread failed");
    jh_eps.join().expect("Joining EPS thread failed");
    jh_controller_thread
        .join()
        .expect("Joining PUS handler thread failed");
}

pub fn update_time(time_provider: &mut CdsTime, timestamp: &mut [u8]) {
    time_provider
        .update_from_now()
        .expect("Could not get current time");
    time_provider
        .write_to_bytes(timestamp)
        .expect("Writing timestamp failed");
}
