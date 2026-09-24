use std::{sync::mpsc, time::Duration};

use nexosim::{
    model::{Context, Model},
    ports::Output,
};
use satrs_minisim::{eps::PcduReply, SimReply};
use types::pcdu::{SwitchId, SwitchMapBinaryWrapper, SwitchStateBinary};

pub const SWITCH_INFO_DELAY_MS: u64 = 10;

pub struct PcduModel {
    pub switcher_map: SwitchMapBinaryWrapper,
    pub mgm_0_switch: Output<SwitchStateBinary>,
    pub mgm_1_switch: Output<SwitchStateBinary>,
    pub mgt_switch: Output<SwitchStateBinary>,
    pub reply_sender: mpsc::Sender<SimReply>,
}

impl PcduModel {
    pub fn new(reply_sender: mpsc::Sender<SimReply>) -> Self {
        Self {
            switcher_map: Default::default(),
            mgm_0_switch: Output::new(),
            mgm_1_switch: Output::new(),
            mgt_switch: Output::new(),
            reply_sender,
        }
    }

    pub async fn request_switch_info(&mut self, _: (), cx: &mut Context<Self>) {
        cx.schedule_event(
            Duration::from_millis(SWITCH_INFO_DELAY_MS),
            Self::send_switch_info,
            (),
        )
        .expect("requesting switch info failed");
    }

    pub fn send_switch_info(&mut self) {
        let reply = SimReply::from(PcduReply::SwitchInfo(self.switcher_map.0.clone()));
        self.reply_sender.send(reply).unwrap();
    }

    pub async fn switch_device(&mut self, switch_and_target_state: (SwitchId, SwitchStateBinary)) {
        log::info!(
            "switching {:?} to {:?}",
            switch_and_target_state.0,
            switch_and_target_state.1
        );
        let val = self
            .switcher_map
            .0
            .get_mut(&switch_and_target_state.0)
            .unwrap_or_else(|| panic!("switch {:?} not found", switch_and_target_state.0));
        *val = switch_and_target_state.1;
        match switch_and_target_state.0 {
            SwitchId::Mgm0 => {
                self.mgm_0_switch.send(switch_and_target_state.1).await;
            }
            SwitchId::Mgm1 => {
                self.mgm_1_switch.send(switch_and_target_state.1).await;
            }
            SwitchId::Mgt => {
                self.mgt_switch.send(switch_and_target_state.1).await;
            }
        }
    }
}

impl Model for PcduModel {}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::time::Duration;

    use satrs_minisim::{eps::PcduRequest, SimRequestWithTime};
    use types::pcdu::SwitchMapBinary;

    use crate::test_helpers::SimTestbench;

    fn switch_device(
        sim_testbench: &mut SimTestbench,
        switch: SwitchId,
        target: SwitchStateBinary,
    ) {
        sim_testbench.send_and_step(PcduRequest::SwitchDevice {
            switch,
            state: target,
        });
    }

    pub(crate) fn switch_device_off(sim_testbench: &mut SimTestbench, switch: SwitchId) {
        switch_device(sim_testbench, switch, SwitchStateBinary::Off);
    }
    pub(crate) fn switch_device_on(sim_testbench: &mut SimTestbench, switch: SwitchId) {
        switch_device(sim_testbench, switch, SwitchStateBinary::On);
    }

    pub(crate) fn get_all_off_switch_map() -> SwitchMapBinary {
        SwitchMapBinaryWrapper::default().0
    }

    fn unwrap_switch_map(sim_reply: SimReply) -> SwitchMapBinary {
        let SimReply::Pcdu(PcduReply::SwitchInfo(switch_map)) = sim_reply else {
            panic!("unexpected reply {sim_reply:?}");
        };
        switch_map
    }

    fn check_switch_state(sim_testbench: &mut SimTestbench, expected_switch_map: &SwitchMapBinary) {
        let sim_reply = sim_testbench
            .request_reply(PcduRequest::RequestSwitchInfo)
            .expect("no PCDU reply received");
        assert_eq!(unwrap_switch_map(sim_reply), *expected_switch_map);
    }

    fn test_pcdu_switching_single_switch(switch: SwitchId, target: SwitchStateBinary) {
        let mut sim_testbench = SimTestbench::new();
        switch_device(&mut sim_testbench, switch, target);
        let mut switcher_map = get_all_off_switch_map();
        *switcher_map.get_mut(&switch).unwrap() = target;
        check_switch_state(&mut sim_testbench, &switcher_map);
    }

    #[test]
    fn test_pcdu_switcher_request() {
        let mut sim_testbench = SimTestbench::new();
        let request = SimRequestWithTime::new_with_epoch_time(PcduRequest::RequestSwitchInfo);
        sim_testbench
            .send_request(request)
            .expect("sending PCDU request failed");
        sim_testbench.handle_sim_requests_time_agnostic();
        sim_testbench.step_until(Duration::from_millis(1)).unwrap();
        assert!(sim_testbench.try_receive_next_reply().is_none());

        // The reply is delayed by SWITCH_INFO_DELAY_MS.
        sim_testbench.step_until(Duration::from_millis(25)).unwrap();
        let sim_reply = sim_testbench
            .try_receive_next_reply()
            .expect("no PCDU reply received");
        assert_eq!(unwrap_switch_map(sim_reply), get_all_off_switch_map());
    }

    #[test]
    fn test_pcdu_switching_mgm_on() {
        test_pcdu_switching_single_switch(SwitchId::Mgm0, SwitchStateBinary::On);
    }

    #[test]
    fn test_pcdu_switching_mgt_on() {
        test_pcdu_switching_single_switch(SwitchId::Mgt, SwitchStateBinary::On);
    }

    #[test]
    fn test_pcdu_switching_mgt_off() {
        test_pcdu_switching_single_switch(SwitchId::Mgt, SwitchStateBinary::On);
        test_pcdu_switching_single_switch(SwitchId::Mgt, SwitchStateBinary::Off);
    }
}
