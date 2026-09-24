use crate::pus::mode::ModeServiceWrapper;
use derive_new::new;
use satrs::spacepackets::time::{cds, TimeWriter};

use super::{
    action::ActionServiceWrapper, event::EventServiceWrapper, hk::HkServiceWrapper,
    scheduler::SchedulingServiceWrapper, test::TestCustomServiceWrapper, DirectPusService,
    HandlingStatus, TargetedPusService,
};

#[derive(new)]
pub struct PusStack {
    pub test_srv: TestCustomServiceWrapper,
    pub hk_srv_wrapper: HkServiceWrapper,
    pub event_srv: EventServiceWrapper,
    pub action_srv_wrapper: ActionServiceWrapper,
    pub schedule_srv: SchedulingServiceWrapper,
    pub mode_srv: ModeServiceWrapper,
}

impl PusStack {
    pub fn periodic_operation(&mut self) {
        // Release all telecommands which reached their release time before calling the service
        // handlers.
        self.schedule_srv.release_tcs();
        let timestamp = cds::CdsTime::now_with_u16_days()
            .expect("time stamp generation error")
            .to_vec()
            .unwrap();
        let mut loop_count = 0_u32;
        // Hot loop which will run continuously until all request and reply handling is done.
        loop {
            let mut nothing_to_do = true;
            Self::direct_service_checker(&mut self.test_srv, &timestamp, &mut nothing_to_do);
            Self::direct_service_checker(&mut self.schedule_srv, &timestamp, &mut nothing_to_do);
            Self::direct_service_checker(&mut self.event_srv, &timestamp, &mut nothing_to_do);
            Self::targeted_service_checker(
                &mut self.action_srv_wrapper,
                &timestamp,
                &mut nothing_to_do,
            );
            Self::targeted_service_checker(
                &mut self.hk_srv_wrapper,
                &timestamp,
                &mut nothing_to_do,
            );
            Self::targeted_service_checker(&mut self.mode_srv, &timestamp, &mut nothing_to_do);
            if nothing_to_do {
                // Timeout checking is only done once.
                self.action_srv_wrapper.check_for_request_timeouts();
                self.hk_srv_wrapper.check_for_request_timeouts();
                self.mode_srv.check_for_request_timeouts();
                break;
            }
            // Safety mechanism to avoid infinite loops.
            loop_count += 1;
            if loop_count >= 500 {
                log::warn!("reached PUS stack loop count 500, breaking");
                break;
            }
        }
    }

    pub fn direct_service_checker<S: DirectPusService>(
        service: &mut S,
        timestamp: &[u8],
        nothing_to_do: &mut bool,
    ) {
        let handling_status = service.poll_and_handle_next_tc(timestamp);
        if handling_status == HandlingStatus::HandledOne {
            *nothing_to_do = false;
        }
    }

    pub fn targeted_service_checker<S: TargetedPusService>(
        service: &mut S,
        timestamp: &[u8],
        nothing_to_do: &mut bool,
    ) {
        let request_handling = service.poll_and_handle_next_tc_default_handler(timestamp);
        let reply_handling = service.poll_and_handle_next_reply_default_handler(timestamp);
        if request_handling == HandlingStatus::HandledOne
            || reply_handling == HandlingStatus::HandledOne
        {
            *nothing_to_do = false;
        }
    }
}
