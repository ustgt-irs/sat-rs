use std::sync::mpsc;
use std::time::Duration;

use crate::pus::create_verification_reporter;
use crate::tmtc::sender::TmTcSender;
use log::info;
use satrs::pool::{PoolProvider, StaticMemoryPool};
use satrs::pus::scheduler::{PusSchedulerAlloc, TcInfo};
use satrs::pus::scheduler_srv::PusSchedServiceHandler;
use satrs::pus::verification::VerificationReporter;
use satrs::pus::{
    DirectPusPacketHandlerResult, EcssTcAndToken, EcssTcCacher, MpscTcReceiver,
    PartialPusHandlingError, PusServiceHelper,
};
use satrs::spacepackets::ecss::PusServiceId;
use satrs::tmtc::{PacketAsVec, PacketInPool, PacketSenderWithSharedPool};
use satrs::ComponentId;
use satrs_example::ids::sched::PUS_SCHED;

use super::{DirectPusService, HandlingStatus};

pub trait TcReleaseProvider {
    fn release(&mut self, sender_id: ComponentId, enabled: bool, info: &TcInfo, tc: &[u8]) -> bool;
}

impl TcReleaseProvider for PacketSenderWithSharedPool {
    fn release(
        &mut self,
        sender_id: ComponentId,
        enabled: bool,
        _info: &TcInfo,
        tc: &[u8],
    ) -> bool {
        if enabled {
            let shared_pool = self.shared_pool.get_mut();
            // Transfer TC from scheduler TC pool to shared TC pool.
            let released_tc_addr = shared_pool
                .0
                .write()
                .expect("locking pool failed")
                .add(tc)
                .expect("adding TC to shared pool failed");
            self.sender
                .send(PacketInPool::new(sender_id, released_tc_addr))
                .expect("sending TC to TC source failed");
        }
        true
    }
}

impl TcReleaseProvider for mpsc::SyncSender<PacketAsVec> {
    fn release(
        &mut self,
        sender_id: ComponentId,
        enabled: bool,
        _info: &TcInfo,
        tc: &[u8],
    ) -> bool {
        if enabled {
            // Send released TC to centralized TC source.
            self.send(PacketAsVec::new(sender_id, tc.to_vec()))
                .expect("sending TC to TC source failed");
        }
        true
    }
}

#[allow(dead_code)]
pub enum TcReleaser {
    Static(PacketSenderWithSharedPool),
    Heap(mpsc::SyncSender<PacketAsVec>),
}

impl TcReleaseProvider for TcReleaser {
    fn release(&mut self, sender_id: ComponentId, enabled: bool, info: &TcInfo, tc: &[u8]) -> bool {
        match self {
            TcReleaser::Static(sender) => sender.release(sender_id, enabled, info, tc),
            TcReleaser::Heap(sender) => sender.release(sender_id, enabled, info, tc),
        }
    }
}

pub struct SchedulingServiceWrapper {
    pub pus_11_handler: PusSchedServiceHandler<
        MpscTcReceiver,
        TmTcSender,
        EcssTcCacher,
        VerificationReporter,
        PusSchedulerAlloc,
    >,
    pub sched_tc_pool: StaticMemoryPool,
    pub releaser_buf: [u8; 4096],
    pub tc_releaser: TcReleaser,
}

impl DirectPusService for SchedulingServiceWrapper {
    const SERVICE_ID: u8 = PusServiceId::Verification as u8;

    const SERVICE_STR: &'static str = "verification";

    fn poll_and_handle_next_tc(&mut self, time_stamp: &[u8]) -> HandlingStatus {
        let error_handler = |partial_error: &PartialPusHandlingError| {
            log::warn!(
                "PUS {}({}) partial error: {:?}",
                Self::SERVICE_ID,
                Self::SERVICE_STR,
                partial_error
            );
        };

        let result = self.pus_11_handler.poll_and_handle_next_tc(
            error_handler,
            time_stamp,
            &mut self.sched_tc_pool,
        );
        if let Err(e) = result {
            log::warn!(
                "PUS {}({}) error: {:?}",
                Self::SERVICE_ID,
                Self::SERVICE_STR,
                e
            );
            // To avoid permanent loops on continuous errors.
            return HandlingStatus::Empty;
        }
        match result.unwrap() {
            DirectPusPacketHandlerResult::Handled(handling_status) => return handling_status,
            DirectPusPacketHandlerResult::CustomSubservice(subservice, _) => {
                log::warn!(
                    "PUS {}({}) subservice {} not implemented",
                    Self::SERVICE_ID,
                    Self::SERVICE_STR,
                    subservice
                );
            }
            DirectPusPacketHandlerResult::SubserviceNotImplemented(subservice, _) => {
                log::warn!(
                    "PUS {}({}) subservice {} not implemented",
                    Self::SERVICE_ID,
                    Self::SERVICE_STR,
                    subservice
                );
            }
        }
        HandlingStatus::HandledOne
    }
}

impl SchedulingServiceWrapper {
    pub fn release_tcs(&mut self) {
        let id = self.pus_11_handler.service_helper.id();
        let releaser = |enabled: bool, info: &TcInfo, tc: &[u8]| -> bool {
            self.tc_releaser.release(id, enabled, info, tc)
        };

        self.pus_11_handler
            .scheduler_mut()
            .update_time_from_now()
            .unwrap();
        let released_tcs = self
            .pus_11_handler
            .scheduler_mut()
            .release_telecommands_with_buffer(
                releaser,
                &mut self.sched_tc_pool,
                &mut self.releaser_buf,
            )
            .expect("releasing TCs failed");
        if released_tcs > 0 {
            info!("{released_tcs} TC(s) released from scheduler");
        }
    }
}

pub fn create_scheduler_service(
    tm_sender: TmTcSender,
    tc_in_mem_converter: EcssTcCacher,
    tc_releaser: TcReleaser,
    pus_sched_rx: mpsc::Receiver<EcssTcAndToken>,
    sched_tc_pool: StaticMemoryPool,
) -> SchedulingServiceWrapper {
    let scheduler = PusSchedulerAlloc::new_with_current_init_time(Duration::from_secs(5))
        .expect("Creating PUS Scheduler failed");
    let pus_11_handler = PusSchedServiceHandler::new(
        PusServiceHelper::new(
            PUS_SCHED.id(),
            pus_sched_rx,
            tm_sender,
            create_verification_reporter(PUS_SCHED.id(), PUS_SCHED.apid),
            tc_in_mem_converter,
        ),
        scheduler,
    );
    SchedulingServiceWrapper {
        pus_11_handler,
        sched_tc_pool,
        releaser_buf: [0; 4096],
        tc_releaser,
    }
}
