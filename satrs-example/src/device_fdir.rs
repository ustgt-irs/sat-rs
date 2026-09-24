use std::collections::VecDeque;
use std::time::Duration;

use satrs::fdir::{FaultCounterStd, FaultResponse, RecoveryEvent, RecoveryFdir};
use satrs::health::{HealthState, HealthTableMapSync};
use types::{ComponentId, DeviceMode};

use crate::device_mode::SwitchAndModeHelper;

// The component is marked faulty if it would be recovered more than RECOVERY_THRESHOLD times
// before the counter is decremented again.
pub const RECOVERY_THRESHOLD: u32 = 2;
pub const RECOVERY_DECREMENT_AFTER: Duration = Duration::from_secs(60);
/// Time the device stays unpowered during a power cycle, so it can fully discharge.
pub const RECOVERY_OFF_DURATION: Duration = Duration::from_millis(500);

/// Generic FDIR events. The device handler maps them to its own event type.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FdirEvent {
    FaultThresholdExceeded,
    Recovery(RecoveryEvent),
}

/// Fault counting and power cycle recovery for device handlers which own the power switch of
/// their device.
///
/// The handler detects faults itself and reports them with [Self::register_fault]. This helper
/// then decides whether the device is power cycled, or marked faulty and switched off, and drives
/// the [SwitchAndModeHelper] of the handler accordingly. The handler retrieves the resulting
/// events with [Self::next_event].
pub struct DeviceFdir {
    name: &'static str,
    fault_counter: FaultCounterStd,
    recovery: RecoveryFdir<HealthTableMapSync>,
    pub recovery_off_duration: Duration,
    events: VecDeque<FdirEvent>,
}

impl DeviceFdir {
    pub fn new(
        name: &'static str,
        component_id: ComponentId,
        health_table: HealthTableMapSync,
        fault_counter: FaultCounterStd,
    ) -> Self {
        Self {
            name,
            fault_counter,
            recovery: RecoveryFdir::new(
                component_id.into(),
                health_table,
                RECOVERY_THRESHOLD,
                RECOVERY_DECREMENT_AFTER,
            ),
            recovery_off_duration: RECOVERY_OFF_DURATION,
            events: VecDeque::new(),
        }
    }

    #[cfg(test)]
    pub fn fault_count(&self) -> u32 {
        self.fault_counter.fault_count()
    }

    pub fn set_health(&mut self, health: HealthState) {
        self.recovery.set_health(health);
    }

    pub fn next_event(&mut self) -> Option<FdirEvent> {
        self.events.pop_front()
    }

    /// Should be called once per cycle, before the mode transition is handled. Starts a power
    /// cycle if the health was set to [HealthState::NeedsRecovery] by the FDIR or by ground.
    pub fn periodic_operation(&mut self, modes: &mut SwitchAndModeHelper<DeviceMode>) {
        self.recovery.periodic_operation();
        self.check_needs_recovery(modes);
    }

    pub fn register_success(&mut self) {
        self.fault_counter.try_decrement();
    }

    /// If the fault threshold is exceeded, the device is power cycled. If it was power cycled
    /// too often, the component is marked faulty and commanded off instead.
    pub fn register_fault(&mut self, modes: &mut SwitchAndModeHelper<DeviceMode>) {
        if !self.fault_counter.increment_and_check() {
            return;
        }
        match self.recovery.handle_fault() {
            FaultResponse::Ignored => {
                log::info!(
                    "{}: fault threshold exceeded, but component is already faulty, \
                     recovering or externally controlled",
                    self.name
                );
            }
            FaultResponse::Recover => {
                log::warn!(
                    "{}: fault threshold exceeded, power cycling device",
                    self.name
                );
                self.events.push_back(FdirEvent::FaultThresholdExceeded);
                self.check_needs_recovery(modes);
            }
            FaultResponse::SetFaulty => {
                log::error!(
                    "{}: fault threshold exceeded after too many recoveries, marking \
                     component faulty",
                    self.name
                );
                self.events.push_back(FdirEvent::FaultThresholdExceeded);
                self.events
                    .push_back(FdirEvent::Recovery(RecoveryEvent::ThresholdExceeded));
                self.switch_off_faulty_device(modes);
            }
        }
    }

    /// Mode commands from ground or the parent abort a running recovery. Must be called before
    /// the commanded transition is started.
    pub fn handle_mode_command(&mut self, modes: &SwitchAndModeHelper<DeviceMode>) {
        if modes.power_cycle_active() {
            log::warn!("{}: mode command aborts power cycle recovery", self.name);
            // Otherwise, the recovery would restart right away.
            self.recovery.recovery_done();
        }
    }

    pub fn handle_power_cycle_done(&mut self) {
        log::info!("{}: power cycle recovery done", self.name);
        // Faults registered while the device was switched off do not count anymore.
        self.fault_counter.clear();
        self.recovery.recovery_done();
        self.events
            .push_back(FdirEvent::Recovery(RecoveryEvent::Done));
    }

    /// A failed power cycle costs a recovery attempt like any other fault.
    pub fn handle_power_cycle_failed(
        &mut self,
        modes: &mut SwitchAndModeHelper<DeviceMode>,
        restore_mode: DeviceMode,
    ) {
        self.events
            .push_back(FdirEvent::Recovery(RecoveryEvent::Failed));
        match self.recovery.recovery_failed() {
            FaultResponse::Recover => {
                log::warn!("{}: power cycle recovery failed, retrying", self.name);
                self.start_recovery(modes, restore_mode);
            }
            FaultResponse::SetFaulty => {
                log::error!(
                    "{}: power cycle recovery failed too often, marking component faulty",
                    self.name
                );
                self.events
                    .push_back(FdirEvent::Recovery(RecoveryEvent::ThresholdExceeded));
                self.switch_off_faulty_device(modes);
            }
            // Ground changed the health during the recovery and is in charge now.
            FaultResponse::Ignored => (),
        }
    }

    fn check_needs_recovery(&mut self, modes: &mut SwitchAndModeHelper<DeviceMode>) {
        if modes.power_cycle_active() || modes.target().is_some() || !self.recovery.needs_recovery()
        {
            return;
        }
        if modes.mode() == DeviceMode::Off {
            // Nothing to power cycle, the next switch-on is a fresh start anyway.
            log::info!("{}: device is off, no recovery required", self.name);
            self.recovery.recovery_done();
            return;
        }
        let restore_mode = modes.mode();
        self.start_recovery(modes, restore_mode);
    }

    fn start_recovery(
        &mut self,
        modes: &mut SwitchAndModeHelper<DeviceMode>,
        restore_mode: DeviceMode,
    ) {
        log::warn!("{}: starting power cycle recovery", self.name);
        modes.start_power_cycle(restore_mode, self.recovery_off_duration);
        self.events
            .push_back(FdirEvent::Recovery(RecoveryEvent::Started));
    }

    fn switch_off_faulty_device(&mut self, modes: &mut SwitchAndModeHelper<DeviceMode>) {
        // Do not restart an already pending Off transition, which would reset the transition
        // state machine before it can finish.
        if modes.target() != Some(DeviceMode::Off) {
            log::warn!("{}: commanding device off due to fault", self.name);
            modes.start_transition(DeviceMode::Off, None);
        }
    }
}
