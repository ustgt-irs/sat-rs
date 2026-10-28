//! #  HK generation helpers
//!
//! Each helper contains the minimal state to support the periodic generation of housekeeping
//! packets. Call [SingleSetHkHelperStd::needs_generation] periodically, for example once per
//! task cycle. When it returns `true`, generate the HK set and send it, the helper has already
//! reset its clock for the next period.
//!
//! Pick a helper based on what clock is available:
//!
//! - [SingleSetHkHelperStd]: `std::time::Instant`, behind the `std` feature.
#![cfg_attr(
    feature = "embassy-time",
    doc = "- [SingleSetHkHelperEmbassy]: `embassy_time::Instant`, behind the `embassy-time` feature."
)]
//! - [SingleSetHkHelperCountdown]: any clock implementing [Countdown], for example a
//!   `fugit`-based monotonic.
//!
//! If your software object has multiple HK sets, you can simply put the helpers inside a dynamic
//! list like [alloc::vec::Vec], [heapless::vec::Vec] or a hash map.
#![deny(missing_docs)]
use crate::time::Countdown;

/// Generic single-set HK helper for any clock, backed by a [Countdown] implementation.
///
/// Useful for clocks not covered by [SingleSetHkHelperStd] or other feature-specific backends,
/// for example a `fugit`-based monotonic. Users implement [Countdown] for their clock and hand
/// it in.
pub struct SingleSetHkHelperCountdown<C: Countdown> {
    countdown: C,
    enabled: bool,
}

impl<C: Countdown> SingleSetHkHelperCountdown<C> {
    /// Create a new, enabled helper wrapping the given countdown.
    pub fn new(countdown: C) -> Self {
        Self {
            countdown,
            enabled: true,
        }
    }

    /// Reference to the wrapped countdown.
    pub fn countdown(&self) -> &C {
        &self.countdown
    }

    /// Mutable reference to the wrapped countdown.
    pub fn countdown_mut(&mut self) -> &mut C {
        &mut self.countdown
    }

    /// Whether the helper is enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Enable or disable the helper.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Returns true if the HK set needs regeneration, resetting the countdown.
    ///
    /// Always returns false while disabled, leaving the countdown untouched.
    pub fn needs_generation(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        if self.countdown.has_expired() {
            self.countdown.reset();
            return true;
        }
        false
    }
}

/// Single-set HK helper backed by [std::time::Instant].
#[cfg(feature = "std")]
pub struct SingleSetHkHelperStd {
    interval: core::time::Duration,
    last_generated: std::time::Instant,
    enabled: bool,
}

#[cfg(feature = "std")]
impl SingleSetHkHelperStd {
    /// Create a new, enabled helper with the given interval, starting the clock now.
    pub fn new(initial_interval: core::time::Duration) -> Self {
        Self {
            interval: initial_interval,
            last_generated: std::time::Instant::now(),
            enabled: true,
        }
    }

    /// Update the generation interval.
    pub fn update_interval(&mut self, interval: core::time::Duration) {
        self.interval = interval;
    }

    /// Current generation interval.
    pub fn interval(&self) -> core::time::Duration {
        self.interval
    }

    /// Whether the helper is enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Enable or disable the helper.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Returns true if the HK set needs regeneration.
    ///
    /// Always returns false while disabled, leaving the clock untouched.
    pub fn needs_generation(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        let now = std::time::Instant::now();
        if now - self.last_generated > self.interval {
            self.last_generated = now;
            return true;
        }
        false
    }
}

/// Single-set HK helper backed by [embassy_time::Instant].
#[cfg(feature = "embassy-time")]
pub struct SingleSetHkHelperEmbassy {
    interval: embassy_time::Duration,
    last_generated: embassy_time::Instant,
    enabled: bool,
}

#[cfg(feature = "embassy-time")]
impl SingleSetHkHelperEmbassy {
    /// Create a new, enabled helper with the given interval, starting the clock now.
    pub fn new(initial_interval: embassy_time::Duration) -> Self {
        Self {
            interval: initial_interval,
            last_generated: embassy_time::Instant::now(),
            enabled: true,
        }
    }

    /// Update the generation interval.
    pub fn update_interval(&mut self, interval: embassy_time::Duration) {
        self.interval = interval;
    }

    /// Current generation interval.
    pub fn interval(&self) -> embassy_time::Duration {
        self.interval
    }

    /// Whether the helper is enabled.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Enable or disable the helper.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Returns true if the HK set needs regeneration.
    ///
    /// Always returns false while disabled, leaving the clock untouched.
    pub fn needs_generation(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        let now = embassy_time::Instant::now();
        if now - self.last_generated > self.interval {
            self.last_generated = now;
            return true;
        }
        false
    }
}
