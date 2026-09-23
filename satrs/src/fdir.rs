//! # FDIR (Fault Detection, Isolation and Recovery) helpers
//!
//! A fault counter tracks a monotonic fault count, decrements it over time when faults stop
//! occurring, and reports when a configured failure threshold has been exceeded. This is the
//! typical building block used to turn a stream of transient error reports into a single
//! "component is faulty" decision without reacting to the first isolated error.
//!
//! The design follows the FSFW `FaultCounter`:
//! <https://egit.irs.uni-stuttgart.de/KSat/fsfw/src/branch/main/src/fsfw/fdir/FaultCounter.h>
//!
//! Pick a variant based on what clock is available:
//!
//! - [FaultCounterStd]: `std::time::Instant`, behind the `std` feature.
#![cfg_attr(
    feature = "embassy-time",
    doc = "- [FaultCounterEmbassy]: `embassy_time::Instant`, behind the `embassy-time` feature."
)]
//!
//! [RecoveryFdir] builds on top of that. It decides whether a component is power cycled or
//! marked faulty when one of its fault counters exceeds its threshold, and keeps the health
//! table up to date during the recovery. It follows the FSFW `DeviceHandlerFailureIsolation`.
#![deny(missing_docs)]

#[cfg(feature = "std")]
use crate::health::{HealthState, HealthTableProvider};

/// Events related to the recovery of a component. Components are expected to embed this into
/// their own event type.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RecoveryEvent {
    /// The component health is [crate::health::HealthState::NeedsRecovery] and it is being
    /// power cycled.
    Started,
    /// The power cycle completed and the component is healthy again.
    Done,
    /// The power cycle failed. This costs a recovery attempt like any other fault, so it is
    /// followed by either a new recovery or [RecoveryEvent::ThresholdExceeded].
    Failed,
    /// The component was recovered too often, it was marked faulty.
    ThresholdExceeded,
}

/// Outcome of [RecoveryFdir::handle_fault] and [RecoveryFdir::recovery_failed].
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FaultResponse {
    /// The component is already faulty, recovering or externally controlled, so nothing was
    /// changed.
    Ignored,
    /// The health was set to [crate::health::HealthState::NeedsRecovery]. The component should
    /// be power cycled.
    Recover,
    /// The component was recovered too often and its health was set to
    /// [crate::health::HealthState::Faulty]. The component should be switched off.
    SetFaulty,
}

/// Fault counter backed by [std::time::Instant].
#[cfg(feature = "std")]
#[derive(Debug, Clone)]
pub struct FaultCounterStd {
    fault_count: u32,
    failure_threshold: u32,
    decrement_after: core::time::Duration,
    last_decrement: Option<std::time::Instant>,
}

#[cfg(feature = "std")]
impl FaultCounterStd {
    /// Create a new [`FaultCounterStd`].
    ///
    /// - `failure_threshold`: threshold above which [`Self::above_threshold`] returns `true` and
    ///   resets the internal count.
    /// - `decrement_after`: minimum duration between automatic decrements performed by
    ///   [`Self::try_decrement`].
    pub fn new(failure_threshold: u32, decrement_after: core::time::Duration) -> Self {
        Self {
            fault_count: 0,
            failure_threshold,
            decrement_after,
            last_decrement: None,
        }
    }

    /// Current fault count.
    pub fn fault_count(&self) -> u32 {
        self.fault_count
    }

    /// Increase the fault count by `1`.
    ///
    /// If the counter was previously `0`, this starts a new decrement clock.
    pub fn increment(&mut self) {
        if self.fault_count == 0 {
            self.last_decrement = Some(std::time::Instant::now());
        }
        self.fault_count += 1;
    }

    /// Increase the fault count by `n`.
    pub fn increment_n(&mut self, n: u32) {
        for _ in 0..n {
            self.increment();
        }
    }

    fn has_decrement_timedout(&self) -> bool {
        match self.last_decrement {
            Some(last_decrement) => last_decrement.elapsed() >= self.decrement_after,
            None => false,
        }
    }

    /// Decrease the fault count by `1` if the decrement timeout elapsed.
    ///
    /// Returns `true` if a decrement was performed, `false` otherwise. A decrement is only
    /// performed when the counter is non-zero and at least `decrement_after` has elapsed since
    /// the last decrement.
    pub fn try_decrement(&mut self) -> bool {
        if self.fault_count == 0 || !self.has_decrement_timedout() {
            return false;
        }
        self.last_decrement = Some(std::time::Instant::now());
        self.fault_count -= 1;
        true
    }

    /// Check whether the counter exceeded the failure threshold.
    ///
    /// Returns `true` when `fault_count > failure_threshold`. In that case, the counter is reset
    /// to `0`.
    pub fn above_threshold(&mut self) -> bool {
        if self.fault_count > self.failure_threshold {
            self.fault_count = 0;
            return true;
        }
        false
    }

    /// Convenience helper to increment once and immediately check the threshold.
    pub fn increment_and_check(&mut self) -> bool {
        self.increment();
        self.above_threshold()
    }

    /// Clear the counter and decrement timing state.
    pub fn clear(&mut self) {
        self.fault_count = 0;
        self.last_decrement = None;
    }

    /// Update the failure threshold used by [`Self::above_threshold`].
    pub fn set_failure_threshold(&mut self, threshold: u32) {
        self.failure_threshold = threshold;
    }

    /// Update the minimum interval between automatic decrements.
    pub fn set_decrement_after(&mut self, duration: core::time::Duration) {
        self.decrement_after = duration;
    }
}

/// Fault counter backed by [embassy_time::Instant].
#[cfg(feature = "embassy-time")]
#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct FaultCounterEmbassy {
    fault_count: u32,
    failure_threshold: u32,
    decrement_after: embassy_time::Duration,
    last_decrement: Option<embassy_time::Instant>,
}

#[cfg(feature = "embassy-time")]
impl FaultCounterEmbassy {
    /// Create a new [`FaultCounterEmbassy`].
    ///
    /// - `failure_threshold`: threshold above which [`Self::above_threshold`] returns `true` and
    ///   resets the internal count.
    /// - `decrement_after`: minimum duration between automatic decrements performed by
    ///   [`Self::try_decrement`].
    pub fn new(failure_threshold: u32, decrement_after: embassy_time::Duration) -> Self {
        Self {
            fault_count: 0,
            failure_threshold,
            decrement_after,
            last_decrement: None,
        }
    }

    /// Current fault count.
    pub fn fault_count(&self) -> u32 {
        self.fault_count
    }

    /// Increase the fault count by `1`.
    ///
    /// If the counter was previously `0`, this starts a new decrement clock.
    pub fn increment(&mut self) {
        if self.fault_count == 0 {
            self.last_decrement = Some(embassy_time::Instant::now());
        }
        self.fault_count += 1;
    }

    /// Increase the fault count by `n`.
    pub fn increment_n(&mut self, n: u32) {
        for _ in 0..n {
            self.increment();
        }
    }

    fn has_decrement_timedout(&self) -> bool {
        match self.last_decrement {
            Some(last_decrement) => {
                embassy_time::Instant::now().duration_since(last_decrement) >= self.decrement_after
            }
            None => false,
        }
    }

    /// Decrease the fault count by `1` if the decrement timeout elapsed.
    ///
    /// Returns `true` if a decrement was performed, `false` otherwise. A decrement is only
    /// performed when the counter is non-zero and at least `decrement_after` has elapsed since
    /// the last decrement.
    pub fn try_decrement(&mut self) -> bool {
        if self.fault_count == 0 || !self.has_decrement_timedout() {
            return false;
        }
        self.last_decrement = Some(embassy_time::Instant::now());
        self.fault_count -= 1;
        true
    }

    /// Check whether the counter exceeded the failure threshold.
    ///
    /// Returns `true` when `fault_count > failure_threshold`. In that case, the counter is reset
    /// to `0`.
    pub fn above_threshold(&mut self) -> bool {
        if self.fault_count > self.failure_threshold {
            self.fault_count = 0;
            return true;
        }
        false
    }

    /// Convenience helper to increment once and immediately check the threshold.
    pub fn increment_and_check(&mut self) -> bool {
        self.increment();
        self.above_threshold()
    }

    /// Clear the counter and decrement timing state.
    pub fn clear(&mut self) {
        self.fault_count = 0;
        self.last_decrement = None;
    }

    /// Update the failure threshold used by [`Self::above_threshold`].
    pub fn set_failure_threshold(&mut self, threshold: u32) {
        self.failure_threshold = threshold;
    }

    /// Update the minimum interval between automatic decrements.
    pub fn set_decrement_after(&mut self, duration: embassy_time::Duration) {
        self.decrement_after = duration;
    }
}

/// Escalates faults of a component to a power cycle recovery first, and to a faulty
/// component if it has to be recovered too often.
///
/// The component itself runs the power cycle while [Self::needs_recovery] returns `true` and
/// reports the outcome with [Self::recovery_done] or [Self::recovery_failed]. Setting
/// [HealthState::NeedsRecovery] from outside, for example by ground, triggers a recovery as well.
#[cfg(feature = "std")]
#[derive(Debug, Clone)]
pub struct RecoveryFdir<HealthTable: HealthTableProvider> {
    id: crate::ComponentId,
    health_table: HealthTable,
    recovery_counter: FaultCounterStd,
}

#[cfg(feature = "std")]
impl<HealthTable: HealthTableProvider> RecoveryFdir<HealthTable> {
    /// Create a new [RecoveryFdir] for component `id`.
    ///
    /// The component is marked faulty when it would be recovered more than `recovery_threshold`
    /// times, with the recovery count being decremented every `recovery_decrement_after`.
    pub fn new(
        id: crate::ComponentId,
        health_table: HealthTable,
        recovery_threshold: u32,
        recovery_decrement_after: core::time::Duration,
    ) -> Self {
        Self {
            id,
            health_table,
            recovery_counter: FaultCounterStd::new(recovery_threshold, recovery_decrement_after),
        }
    }

    /// Health of the component. Absent entries are returned as `None`.
    pub fn health(&self) -> Option<HealthState> {
        self.health_table.health(self.id)
    }

    /// Set the health of the component.
    pub fn set_health(&mut self, health: HealthState) {
        self.health_table.set_health(self.id, health);
    }

    /// Should be called periodically to decrement the recovery counter.
    pub fn periodic_operation(&mut self) {
        self.recovery_counter.try_decrement();
    }

    /// Should be called when a fault counter of the component exceeded its threshold.
    pub fn handle_fault(&mut self) -> FaultResponse {
        // Ground may have taken manual control, or already given up on this component.
        // Autonomous FDIR should not override that decision. An already faulty or recovering
        // component must not be escalated again. For example, this would allow a faulty component
        // to become healthy again, because the recovery counter was reset when it became faulty.
        if matches!(
            self.health(),
            Some(HealthState::ExternalControl)
                | Some(HealthState::PermanentFaulty)
                | Some(HealthState::Faulty)
                | Some(HealthState::NeedsRecovery)
        ) {
            return FaultResponse::Ignored;
        }
        self.escalate()
    }

    /// Every recovery attempt counts. If there were too many attempts, the component is marked
    /// faulty. Otherwise, the component should be recovered (again).
    fn escalate(&mut self) -> FaultResponse {
        if self.recovery_counter.increment_and_check() {
            self.set_health(HealthState::Faulty);
            return FaultResponse::SetFaulty;
        }
        self.set_health(HealthState::NeedsRecovery);
        FaultResponse::Recover
    }

    /// The component should be power cycled.
    pub fn needs_recovery(&self) -> bool {
        self.health() == Some(HealthState::NeedsRecovery)
    }

    /// The power cycle completed, or was not required. Sets the health back to healthy.
    pub fn recovery_done(&mut self) {
        // The health might have changed during the recovery.
        if self.needs_recovery() {
            self.set_health(HealthState::Healthy);
        }
    }

    /// The power cycle failed. This costs a recovery attempt like any other fault.
    pub fn recovery_failed(&mut self) -> FaultResponse {
        // The health might have changed during the recovery.
        if !self.needs_recovery() {
            return FaultResponse::Ignored;
        }
        self.escalate()
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn threshold_not_exceeded_below_limit() {
        let mut fc = FaultCounterStd::new(2, Duration::from_secs(60));
        assert!(!fc.increment_and_check());
        assert!(!fc.increment_and_check());
        assert_eq!(fc.fault_count(), 2);
    }

    #[test]
    fn threshold_exceeded_resets_counter() {
        let mut fc = FaultCounterStd::new(2, Duration::from_secs(60));
        fc.increment_n(3);
        assert!(fc.above_threshold());
        assert_eq!(fc.fault_count(), 0);
        assert!(!fc.above_threshold());
    }

    #[test]
    fn decrement_only_after_timeout() {
        let mut fc = FaultCounterStd::new(5, Duration::from_millis(20));
        fc.increment();
        assert!(!fc.try_decrement());
        thread::sleep(Duration::from_millis(30));
        assert!(fc.try_decrement());
        assert_eq!(fc.fault_count(), 0);
    }

    #[test]
    fn decrement_noop_when_empty() {
        let mut fc = FaultCounterStd::new(5, Duration::from_millis(1));
        thread::sleep(Duration::from_millis(2));
        assert!(!fc.try_decrement());
    }

    #[test]
    fn increment_after_empty_resets_decrement_timing() {
        let mut fc = FaultCounterStd::new(5, Duration::from_millis(20));
        fc.increment();
        thread::sleep(Duration::from_millis(30));
        assert!(fc.try_decrement());
        // Counter is 0 again, incrementing should require a fresh decrement_after wait.
        fc.increment();
        assert!(!fc.try_decrement());
    }

    fn recovery_fdir() -> RecoveryFdir<crate::health::HealthTableMapSync> {
        recovery_fdir_with_threshold(1)
    }

    fn recovery_fdir_with_threshold(
        recovery_threshold: u32,
    ) -> RecoveryFdir<crate::health::HealthTableMapSync> {
        RecoveryFdir::new(
            1,
            crate::health::HealthTableMapSync::default(),
            recovery_threshold,
            Duration::from_secs(60),
        )
    }

    #[test]
    fn first_fault_triggers_recovery() {
        let mut fdir = recovery_fdir();
        assert_eq!(fdir.handle_fault(), FaultResponse::Recover);
        assert!(fdir.needs_recovery());
        fdir.recovery_done();
        assert_eq!(fdir.health(), Some(HealthState::Healthy));
    }

    #[test]
    fn repeated_recovery_sets_faulty() {
        let mut fdir = recovery_fdir();
        assert_eq!(fdir.handle_fault(), FaultResponse::Recover);
        fdir.recovery_done();
        assert_eq!(fdir.handle_fault(), FaultResponse::SetFaulty);
        assert_eq!(fdir.health(), Some(HealthState::Faulty));
    }

    #[test]
    fn faulty_component_stays_faulty() {
        let mut fdir = recovery_fdir();
        fdir.handle_fault();
        fdir.recovery_done();
        assert_eq!(fdir.handle_fault(), FaultResponse::SetFaulty);
        // The recovery counter was reset, but this must not trigger a new recovery.
        assert_eq!(fdir.handle_fault(), FaultResponse::Ignored);
        assert_eq!(fdir.health(), Some(HealthState::Faulty));
    }

    #[test]
    fn failed_recovery_is_retried() {
        let mut fdir = recovery_fdir_with_threshold(2);
        fdir.handle_fault();
        assert_eq!(fdir.recovery_failed(), FaultResponse::Recover);
        assert!(fdir.needs_recovery());
        assert_eq!(fdir.recovery_failed(), FaultResponse::SetFaulty);
        assert_eq!(fdir.health(), Some(HealthState::Faulty));
    }

    #[test]
    fn health_is_not_overridden() {
        let mut fdir = recovery_fdir();
        for health in [
            HealthState::ExternalControl,
            HealthState::PermanentFaulty,
            HealthState::Faulty,
            HealthState::NeedsRecovery,
        ] {
            fdir.set_health(health);
            assert_eq!(fdir.handle_fault(), FaultResponse::Ignored);
            assert_eq!(fdir.health(), Some(health));
        }
    }

    #[test]
    fn health_changed_during_recovery_is_kept() {
        let mut fdir = recovery_fdir();
        fdir.handle_fault();
        fdir.set_health(HealthState::ExternalControl);
        fdir.recovery_done();
        assert_eq!(fdir.health(), Some(HealthState::ExternalControl));
        assert_eq!(fdir.recovery_failed(), FaultResponse::Ignored);
        assert_eq!(fdir.health(), Some(HealthState::ExternalControl));
    }

    #[test]
    fn clear_resets_state() {
        let mut fc = FaultCounterStd::new(1, Duration::from_secs(60));
        fc.increment_n(2);
        fc.clear();
        assert_eq!(fc.fault_count(), 0);
        assert!(!fc.above_threshold());
    }
}
