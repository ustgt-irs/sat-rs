use acs::{mgm::MagnetometerModel, mgt::MagnetorquerModel};
use controller::{ModelAddrWrapper, SimController};
use eps::PcduModel;
use nexosim::simulation::{Mailbox, SimInit};
use nexosim::time::{MonotonicTime, SystemClock};
use satrs_minisim::acs::mgm::MgmId;
use satrs_minisim::udp::SIM_CTRL_PORT;
use satrs_minisim::{SimReply, SimRequest};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime};
use udp::SimUdpServer;

mod acs;
mod controller;
mod eps;
#[cfg(test)]
mod test_helpers;
mod time;
mod udp;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ThreadingModel {
    Default = 0,
    Single = 1,
}

fn create_sim_controller(
    threading_model: ThreadingModel,
    start_time: MonotonicTime,
    reply_sender: mpsc::Sender<SimReply>,
    request_receiver: mpsc::Receiver<SimRequest>,
) -> SimController {
    // Instantiate models and their mailboxes.
    let mgm_0_model =
        MagnetometerModel::new(MgmId::Mgm0, Duration::from_millis(50), reply_sender.clone());
    let mgm_1_model =
        MagnetometerModel::new(MgmId::Mgm1, Duration::from_millis(50), reply_sender.clone());

    let mgm_0_mailbox = Mailbox::new();
    let mgm_0_addr = mgm_0_mailbox.address();
    let mgm_1_mailbox = Mailbox::new();
    let mgm_1_addr = mgm_1_mailbox.address();
    let pcdu_mailbox = Mailbox::new();
    let pcdu_addr = pcdu_mailbox.address();
    let mgt_mailbox = Mailbox::new();
    let mgt_addr = mgt_mailbox.address();

    let mut pcdu_model = PcduModel::new(reply_sender.clone());
    pcdu_model
        .mgm_0_switch
        .connect(MagnetometerModel::switch_device, &mgm_0_addr);
    pcdu_model
        .mgm_1_switch
        .connect(MagnetometerModel::switch_device, &mgm_1_addr);

    let mut mgt_model = MagnetorquerModel::new(reply_sender.clone());
    // Input connections.
    pcdu_model
        .mgt_switch
        .connect(MagnetorquerModel::switch_device, &mgt_addr);
    // Output connections.
    mgt_model.gen_magnetic_field.connect(
        MagnetometerModel::apply_external_magnetic_field,
        &mgm_0_addr,
    );
    mgt_model.gen_magnetic_field.connect(
        MagnetometerModel::apply_external_magnetic_field,
        &mgm_1_addr,
    );

    // Instantiate the simulator
    let sys_clock = SystemClock::from_system_time(start_time, SystemTime::now());
    let sim_init = if threading_model == ThreadingModel::Single {
        SimInit::with_num_threads(1)
    } else {
        SimInit::new()
    };
    let addrs = ModelAddrWrapper::new(mgm_0_addr, mgm_1_addr, pcdu_addr, mgt_addr);
    let (simulation, scheduler) = sim_init
        .add_model(mgm_0_model, mgm_0_mailbox, "MGM 0 model")
        .add_model(mgm_1_model, mgm_1_mailbox, "MGM 1 model")
        .add_model(pcdu_model, pcdu_mailbox, "PCDU model")
        .add_model(mgt_model, mgt_mailbox, "MGT model")
        .init(start_time)
        .unwrap();
    SimController::new(
        sys_clock,
        request_receiver,
        reply_sender,
        simulation,
        scheduler,
        addrs,
    )
}

fn main() {
    let (request_sender, request_receiver) = mpsc::channel();
    let (reply_sender, reply_receiver) = mpsc::channel();
    let t0 = MonotonicTime::EPOCH;
    let mut sim_ctrl =
        create_sim_controller(ThreadingModel::Default, t0, reply_sender, request_receiver);
    // Configure logger at runtime
    fern::Dispatch::new()
        // Perform allocation-free log formatting
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{} {} {}] {}",
                humantime::format_rfc3339(std::time::SystemTime::now()),
                record.level(),
                record.target(),
                message
            ))
        })
        // Add blanket level filter -
        .level(log::LevelFilter::Debug)
        // - and per-module overrides
        // Output to stdout, files, and other Dispatch configurations
        .chain(std::io::stdout())
        .chain(fern::log_file("output.log").expect("could not open log output file"))
        // Apply globally
        .apply()
        .expect("could not apply logger configuration");

    log::info!("starting simulation thread");
    // This thread schedules the simulator.
    let sim_thread = thread::spawn(move || {
        sim_ctrl.run(t0, 1);
    });

    let mut udp_server =
        SimUdpServer::new(SIM_CTRL_PORT, request_sender, reply_receiver, 200, None)
            .expect("could not create UDP request server");
    log::info!("starting UDP server on port {SIM_CTRL_PORT}");
    // This thread manages the simulator UDP server.
    let udp_tc_thread = thread::spawn(move || {
        udp_server.run();
    });

    sim_thread.join().expect("joining simulation thread failed");
    udp_tc_thread
        .join()
        .expect("joining UDP server thread failed");
}
