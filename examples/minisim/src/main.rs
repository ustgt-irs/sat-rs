use controller::{SimController, ThreadingModel};
use minisim_types::udp::SIM_CTRL_PORT;
use nexosim::time::MonotonicTime;
use std::sync::mpsc;
use std::thread;
use udp::SimUdpServer;

mod acs;
mod controller;
mod eps;
#[cfg(test)]
mod test_helpers;
mod time;
mod udp;

fn main() {
    let (request_sender, request_receiver) = mpsc::channel();
    let (reply_sender, reply_receiver) = mpsc::channel();
    let t0 = MonotonicTime::EPOCH;
    let mut sim_ctrl =
        SimController::new(ThreadingModel::Default, t0, reply_sender, request_receiver);
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
