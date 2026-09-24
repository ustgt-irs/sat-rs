use std::time::{Duration, Instant};

use types::pcdu::SwitchId;

use crate::eps::PowerSwitchHelper;

/// This is a helper trait required to make [SwitchAndModeHelper] generic.
///
/// It allows distinguish a powered-off state from one or more powered-on states, so
/// [`SwitchAndModeHelper`] knows which way to drive the switch for a given target mode.
pub trait PowerSwitchedMode: Copy + PartialEq {
    const OFF: Self;
    fn requires_power(&self) -> bool;
}

impl PowerSwitchedMode for types::DeviceMode {
    const OFF: Self = types::DeviceMode::Off;

    fn requires_power(&self) -> bool {
        *self != types::DeviceMode::Off
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
enum SwitchTransitionState {
    #[default]
    Idle,
    PowerSwitching,
    Done,
}

/// Outcome of a single power switch transition.
enum SwitchOutcome {
    Reached(Option<satrs::spacepackets::CcsdsPacketIdAndPsc>),
    Failed(Option<satrs::spacepackets::CcsdsPacketIdAndPsc>),
}

/// Dedicated states for power cycling a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerCycleState<Mode> {
    Idle,
    SwitchingOff { restore_mode: Mode },
    WaitingOff { restore_mode: Mode, since: Instant },
    SwitchingOn { restore_mode: Mode },
}

/// Outcome of a pending mode transition, once [`SwitchAndModeHelper::handle_mode_transition`]
/// has driven it to completion. Carries back whichever TC commanded the transition, if any, so
/// the caller can reply to it -- what that reply looks like is handler-specific, so this stays
/// out of the helper.
pub enum ModeTransitionEvent<Mode> {
    /// The target mode was reached.
    Reached(Option<satrs::spacepackets::CcsdsPacketIdAndPsc>),
    /// The target mode could not be reached.
    Failed(Option<satrs::spacepackets::CcsdsPacketIdAndPsc>),
    /// The power cycle completed and the mode before the power cycle was restored.
    PowerCycleDone,
    /// Power switching failed during the power cycle. The power cycle is not hidden anymore,
    /// so [SwitchAndModeHelper::reported_mode] returns the actual mode again. `restore_mode` is
    /// the mode the power cycle should have restored, which can be used to retry it.
    PowerCycleFailed { restore_mode: Mode },
}

/// Drives the on/off power-switch commanding state machine (Idle -> PowerSwitching -> Done)
/// shared by every device handler that owns a single power switch of its own.
///
/// Handler-specific reactions (sending telemetry, invalidating cached sensor data, reporting to
/// a parent) are not this helper's concern: [`Self::handle_mode_transition`] just reports when a
/// transition finishes (or fails) and leaves what to do about it to the caller.
pub struct SwitchAndModeHelper<Mode: PowerSwitchedMode> {
    mode_helper: satrs_example::ModeHelper<Mode, SwitchTransitionState>,
    switch_helper: PowerSwitchHelper,
    switch_id: SwitchId,
    power_cycle_state: PowerCycleState<Mode>,
    power_cycle_off_duration: Duration,
}

impl<Mode: PowerSwitchedMode> SwitchAndModeHelper<Mode> {
    pub fn new(
        init_mode: Mode,
        timeout: Duration,
        switch_helper: PowerSwitchHelper,
        switch_id: SwitchId,
    ) -> Self {
        Self {
            mode_helper: satrs_example::ModeHelper::new(init_mode, timeout),
            switch_helper,
            switch_id,
            power_cycle_state: PowerCycleState::Idle,
            power_cycle_off_duration: Duration::ZERO,
        }
    }

    #[inline]
    pub fn mode(&self) -> Mode {
        self.mode_helper.current
    }

    #[inline]
    pub fn target(&self) -> Option<Mode> {
        self.mode_helper.target
    }

    /// Mode which should be reported to other components. A power cycle is hidden from them,
    /// so this is the mode which is restored after the power cycle while one is active.
    pub fn reported_mode(&self) -> Mode {
        match self.power_cycle_state {
            PowerCycleState::SwitchingOff { restore_mode }
            | PowerCycleState::WaitingOff { restore_mode, .. }
            | PowerCycleState::SwitchingOn { restore_mode } => restore_mode,
            PowerCycleState::Idle => self.mode(),
        }
    }

    #[inline]
    pub fn power_cycle_active(&self) -> bool {
        self.power_cycle_state != PowerCycleState::Idle
    }

    /// Starts a new transition, aborting a running power cycle.
    pub fn start_transition(
        &mut self,
        target_mode: Mode,
        tc_commander: Option<satrs::spacepackets::CcsdsPacketIdAndPsc>,
    ) {
        self.power_cycle_state = PowerCycleState::Idle;
        self.start_transition_internal(target_mode, tc_commander);
    }

    /// Switches the device off, keeps it off for `off_duration` and then switches it to
    /// `restore_mode`. Reaching the intermediate off mode does not generate an event.
    pub fn start_power_cycle(&mut self, restore_mode: Mode, off_duration: Duration) {
        self.power_cycle_state = PowerCycleState::SwitchingOff { restore_mode };
        self.power_cycle_off_duration = off_duration;
        self.start_transition_internal(Mode::OFF, None);
    }

    fn start_transition_internal(
        &mut self,
        target_mode: Mode,
        tc_commander: Option<satrs::spacepackets::CcsdsPacketIdAndPsc>,
    ) {
        self.mode_helper.tc_commander = tc_commander;
        self.mode_helper.start(target_mode);
    }

    /// This is the main API that the periodic handler of a device handler should call.
    ///
    /// It handles the switch commanding and returns relevant events.
    pub fn handle_mode_transition(&mut self) -> Option<ModeTransitionEvent<Mode>> {
        // The most probable case: Nothing to do.
        if self.target().is_none() && !self.power_cycle_active() {
            return None;
        }
        // Handle this as an extra step so the switch transition after this can proceed.
        self.handle_waiting_for_off_when_power_cycling();
        // Core logic: Command the switches, check whether target switch state was reached.
        // Note the ?: if a switch transition is on-going, we might do an early return.
        let outcome = self.handle_switch_transition()?;
        // Regular mode transition without power cycling.
        if self.power_cycle_state == PowerCycleState::Idle {
            return Some(match outcome {
                SwitchOutcome::Reached(tc_commander) => ModeTransitionEvent::Reached(tc_commander),
                SwitchOutcome::Failed(tc_commander) => ModeTransitionEvent::Failed(tc_commander),
            });
        }
        // Power cycling, where a bit more logic is required.
        // Handle the error case first.
        if let SwitchOutcome::Failed(_) = outcome {
            let restore_mode = self.reported_mode();
            self.power_cycle_state = PowerCycleState::Idle;
            return Some(ModeTransitionEvent::PowerCycleFailed { restore_mode });
        }
        // At this point: The switching was succesfull, so we only match on the
        // power cycle state.
        match self.power_cycle_state {
            // No switching going on for thse cases.
            PowerCycleState::Idle | PowerCycleState::WaitingOff { .. } => None,
            PowerCycleState::SwitchingOff { restore_mode } => {
                self.power_cycle_state = PowerCycleState::WaitingOff {
                    restore_mode,
                    since: Instant::now(),
                };
                None
            }
            PowerCycleState::SwitchingOn { .. } => {
                // Power is back and we are done.
                self.power_cycle_state = PowerCycleState::Idle;
                Some(ModeTransitionEvent::PowerCycleDone)
            }
        }
    }

    fn handle_waiting_for_off_when_power_cycling(&mut self) {
        if let PowerCycleState::WaitingOff {
            restore_mode,
            since,
        } = self.power_cycle_state
            && since.elapsed() >= self.power_cycle_off_duration
        {
            self.power_cycle_state = PowerCycleState::SwitchingOn { restore_mode };
            self.start_transition_internal(restore_mode, None);
        }
    }

    fn handle_switch_transition(&mut self) -> Option<SwitchOutcome> {
        let target_mode = self.mode_helper.target?;
        let switch_target_on = target_mode.requires_power();
        if self.mode_helper.transition_state == SwitchTransitionState::Idle {
            let result = if switch_target_on {
                self.switch_helper.send_switch_on_cmd(self.switch_id)
            } else {
                self.switch_helper.send_switch_off_cmd(self.switch_id)
            };
            if result.is_err() {
                // Could not send switch command.. still continue with transition.
                log::error!(
                    "failed to send switch {} command",
                    if switch_target_on { "on" } else { "off" }
                );
            }
            self.mode_helper.transition_state = SwitchTransitionState::PowerSwitching;
        }
        if self.mode_helper.transition_state == SwitchTransitionState::PowerSwitching {
            if self.switch_helper.is_switch_on(self.switch_id) == switch_target_on {
                log::info!("switch is {}", if switch_target_on { "on" } else { "off" });
                self.mode_helper.transition_state = SwitchTransitionState::Done;
            } else if self.mode_helper.timed_out() {
                return Some(SwitchOutcome::Failed(self.mode_helper.finish(false)));
            }
        }
        if self.mode_helper.transition_state == SwitchTransitionState::Done {
            return Some(SwitchOutcome::Reached(self.mode_helper.finish(true)));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, mpsc};

    use arbitrary_int::u11;
    use satrs::spacepackets::{CcsdsPacketIdAndPsc, SpacePacketHeader};
    use types::{
        DeviceMode,
        pcdu::{SwitchRequest, SwitchState, SwitchStateBinary},
    };

    use crate::eps::pcdu::{SharedSwitchSet, SwitchMap, SwitchSet};

    use super::*;

    const TIMEOUT: Duration = Duration::from_millis(50);

    struct Testbench {
        helper: SwitchAndModeHelper<DeviceMode>,
        switch_rx: mpsc::Receiver<SwitchRequest>,
        shared_switch_set: SharedSwitchSet,
    }

    impl Testbench {
        fn new() -> Self {
            let (switch_tx, switch_rx) = mpsc::sync_channel(10);
            let mut switch_map = SwitchMap::new();
            switch_map.insert(SwitchId::Mgm0, SwitchState::Off);
            let shared_switch_set: SharedSwitchSet =
                Arc::new(Mutex::new(SwitchSet::new(switch_map)));
            Self {
                helper: SwitchAndModeHelper::new(
                    DeviceMode::Off,
                    TIMEOUT,
                    PowerSwitchHelper::new(switch_tx, shared_switch_set.clone()),
                    SwitchId::Mgm0,
                ),
                switch_rx,
                shared_switch_set,
            }
        }

        fn set_switch_state(&self, state: SwitchState) {
            self.shared_switch_set
                .lock()
                .unwrap()
                .set_switch_state(SwitchId::Mgm0, state);
        }

        fn switch_requests(&self) -> Vec<SwitchStateBinary> {
            self.switch_rx
                .try_iter()
                .map(|req| req.target_state)
                .collect()
        }

        /// Drives a transition to `Normal` to completion.
        fn switch_to_normal(&mut self) {
            self.helper.start_transition(DeviceMode::Normal, None);
            self.set_switch_state(SwitchState::On);
            assert!(matches!(
                self.helper.handle_mode_transition(),
                Some(ModeTransitionEvent::Reached(None))
            ));
            self.switch_requests();
        }

        /// Starts a power cycle from `Normal` and drives it until the device is off.
        fn power_cycle_until_off(&mut self, off_duration: Duration) {
            self.switch_to_normal();
            self.helper
                .start_power_cycle(DeviceMode::Normal, off_duration);
            assert!(self.helper.handle_mode_transition().is_none());
            assert_eq!(self.switch_requests(), [SwitchStateBinary::Off]);
            self.set_switch_state(SwitchState::Off);
            assert!(self.helper.handle_mode_transition().is_none());
            assert_eq!(self.helper.mode(), DeviceMode::Off);
        }
    }

    fn tc_id() -> CcsdsPacketIdAndPsc {
        CcsdsPacketIdAndPsc::new_from_ccsds_packet(&SpacePacketHeader::new_from_apid(u11::new(1)))
    }

    #[test]
    fn test_no_transition() {
        let mut tb = Testbench::new();
        assert_eq!(tb.helper.mode(), DeviceMode::Off);
        assert_eq!(tb.helper.target(), None);
        assert!(tb.helper.handle_mode_transition().is_none());
        assert!(tb.switch_requests().is_empty());
        assert!(!tb.helper.power_cycle_active());
        assert_eq!(tb.helper.reported_mode(), DeviceMode::Off);
    }

    #[test]
    fn test_switch_on() {
        let mut tb = Testbench::new();
        tb.helper
            .start_transition(DeviceMode::Normal, Some(tc_id()));
        assert!(tb.helper.handle_mode_transition().is_none());
        assert_eq!(tb.switch_requests(), [SwitchStateBinary::On]);
        assert_eq!(tb.helper.mode(), DeviceMode::Off);
        assert_eq!(tb.helper.target(), Some(DeviceMode::Normal));

        tb.set_switch_state(SwitchState::On);
        match tb.helper.handle_mode_transition() {
            Some(ModeTransitionEvent::Reached(Some(id))) => assert_eq!(id, tc_id()),
            _ => panic!("expected mode reached event with TC commander"),
        }
        assert_eq!(tb.helper.mode(), DeviceMode::Normal);
        assert_eq!(tb.helper.target(), None);
        assert!(tb.switch_requests().is_empty());
    }

    #[test]
    fn test_switch_off() {
        let mut tb = Testbench::new();
        tb.switch_to_normal();
        tb.helper.start_transition(DeviceMode::Off, None);
        assert!(tb.helper.handle_mode_transition().is_none());
        assert_eq!(tb.switch_requests(), [SwitchStateBinary::Off]);
        tb.set_switch_state(SwitchState::Off);
        assert!(matches!(
            tb.helper.handle_mode_transition(),
            Some(ModeTransitionEvent::Reached(None))
        ));
        assert_eq!(tb.helper.mode(), DeviceMode::Off);
    }

    #[test]
    fn test_switch_already_in_target_state() {
        let mut tb = Testbench::new();
        tb.set_switch_state(SwitchState::On);
        tb.helper.start_transition(DeviceMode::On, None);
        assert!(matches!(
            tb.helper.handle_mode_transition(),
            Some(ModeTransitionEvent::Reached(None))
        ));
        // The switch command is still sent.
        assert_eq!(tb.switch_requests(), [SwitchStateBinary::On]);
        assert_eq!(tb.helper.mode(), DeviceMode::On);
    }

    #[test]
    fn test_switch_timeout() {
        let mut tb = Testbench::new();
        tb.helper
            .start_transition(DeviceMode::Normal, Some(tc_id()));
        assert!(tb.helper.handle_mode_transition().is_none());
        std::thread::sleep(TIMEOUT);
        match tb.helper.handle_mode_transition() {
            Some(ModeTransitionEvent::Failed(Some(id))) => assert_eq!(id, tc_id()),
            _ => panic!("expected mode failed event with TC commander"),
        }
        assert_eq!(tb.helper.mode(), DeviceMode::Off);
        assert_eq!(tb.helper.target(), None);
    }

    #[test]
    fn test_power_cycle() {
        let mut tb = Testbench::new();
        tb.power_cycle_until_off(Duration::ZERO);
        assert!(tb.helper.power_cycle_active());

        // The off duration elapsed, so switching on starts right away.
        assert!(tb.helper.handle_mode_transition().is_none());
        assert_eq!(tb.switch_requests(), [SwitchStateBinary::On]);
        assert_eq!(tb.helper.target(), Some(DeviceMode::Normal));
        tb.set_switch_state(SwitchState::On);
        assert!(matches!(
            tb.helper.handle_mode_transition(),
            Some(ModeTransitionEvent::PowerCycleDone)
        ));
        assert_eq!(tb.helper.mode(), DeviceMode::Normal);
        assert!(!tb.helper.power_cycle_active());
    }

    #[test]
    fn test_power_cycle_reports_restored_mode() {
        let mut tb = Testbench::new();
        tb.switch_to_normal();
        tb.helper
            .start_power_cycle(DeviceMode::Normal, Duration::from_secs(60));
        assert_eq!(tb.helper.reported_mode(), DeviceMode::Normal);
        tb.helper.handle_mode_transition();
        tb.set_switch_state(SwitchState::Off);
        tb.helper.handle_mode_transition();
        assert_eq!(tb.helper.mode(), DeviceMode::Off);
        assert_eq!(tb.helper.reported_mode(), DeviceMode::Normal);
    }

    #[test]
    fn test_power_cycle_reports_restored_mode_while_switching_on() {
        let mut tb = Testbench::new();
        tb.power_cycle_until_off(Duration::ZERO);
        assert!(tb.helper.handle_mode_transition().is_none());
        assert_eq!(tb.helper.target(), Some(DeviceMode::Normal));
        assert_eq!(tb.helper.mode(), DeviceMode::Off);
        assert_eq!(tb.helper.reported_mode(), DeviceMode::Normal);
    }

    #[test]
    fn test_power_cycle_waits_off_duration() {
        let mut tb = Testbench::new();
        tb.power_cycle_until_off(Duration::from_secs(60));
        for _ in 0..3 {
            assert!(tb.helper.handle_mode_transition().is_none());
        }
        assert!(tb.switch_requests().is_empty());
        assert_eq!(tb.helper.target(), None);
        assert!(tb.helper.power_cycle_active());
    }

    #[test]
    fn test_power_cycle_switch_off_timeout() {
        let mut tb = Testbench::new();
        tb.switch_to_normal();
        tb.helper
            .start_power_cycle(DeviceMode::Normal, Duration::ZERO);
        assert!(tb.helper.handle_mode_transition().is_none());
        std::thread::sleep(TIMEOUT);
        assert!(matches!(
            tb.helper.handle_mode_transition(),
            Some(ModeTransitionEvent::PowerCycleFailed {
                restore_mode: DeviceMode::Normal
            })
        ));
        assert_eq!(tb.helper.mode(), DeviceMode::Normal);
        assert!(!tb.helper.power_cycle_active());
    }

    #[test]
    fn test_power_cycle_switch_on_timeout() {
        let mut tb = Testbench::new();
        tb.power_cycle_until_off(Duration::ZERO);
        assert!(tb.helper.handle_mode_transition().is_none());
        std::thread::sleep(TIMEOUT);
        assert!(matches!(
            tb.helper.handle_mode_transition(),
            Some(ModeTransitionEvent::PowerCycleFailed {
                restore_mode: DeviceMode::Normal
            })
        ));
        assert_eq!(tb.helper.mode(), DeviceMode::Off);
        assert!(!tb.helper.power_cycle_active());
        // The failed power cycle is not hidden anymore.
        assert_eq!(tb.helper.reported_mode(), DeviceMode::Off);
    }

    #[test]
    fn test_transition_aborts_power_cycle() {
        let mut tb = Testbench::new();
        tb.power_cycle_until_off(Duration::from_secs(60));
        tb.helper.start_transition(DeviceMode::On, Some(tc_id()));
        assert!(!tb.helper.power_cycle_active());
        assert_eq!(tb.helper.reported_mode(), DeviceMode::Off);
        tb.set_switch_state(SwitchState::On);
        // A regular transition event instead of a power cycle event.
        assert!(matches!(
            tb.helper.handle_mode_transition(),
            Some(ModeTransitionEvent::Reached(Some(_)))
        ));
        assert_eq!(tb.helper.mode(), DeviceMode::On);
    }
}
