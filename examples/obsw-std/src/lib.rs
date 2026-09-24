extern crate alloc;

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use satrs::spacepackets::{CcsdsPacketIdAndPsc, time::cds::CdsTime};
pub use types::ComponentId;
use types::ccsds::{CcsdsTcPacketOwned, CcsdsTmPacketOwned};

pub mod config;

/// Simple type modelling packet stored in the heap. This structure is intended to
/// be used when sending a packet via a message queue, so it also contains the sender ID.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PacketAsVec {
    pub sender_id: ComponentId,
    pub packet: Vec<u8>,
}

impl PacketAsVec {
    pub fn new(sender_id: ComponentId, packet: Vec<u8>) -> Self {
        Self { sender_id, packet }
    }
}

pub struct TimestampHelper {
    stamper: CdsTime,
    time_stamp: [u8; 7],
}

impl TimestampHelper {
    pub fn stamp(&self) -> &[u8] {
        &self.time_stamp
    }

    pub fn update_from_now(&mut self) {
        self.stamper
            .update_from_now()
            .expect("Updating timestamp failed");
        self.stamper
            .write_to_bytes(&mut self.time_stamp)
            .expect("Writing timestamp failed");
    }
}

impl Default for TimestampHelper {
    fn default() -> Self {
        Self {
            stamper: CdsTime::now_with_u16_days().expect("creating time stamper failed"),
            time_stamp: Default::default(),
        }
    }
}

/// Helper structure for periodic HK generation of a single set.
#[derive(Debug)]
pub struct HkHelperSingleSet {
    pub enabled: bool,
    pub frequency: Duration,
    pub last_generated: Option<Instant>,
}

impl HkHelperSingleSet {
    #[inline]
    pub const fn new(enabled: bool, init_frequency: Duration) -> Self {
        Self {
            enabled,
            frequency: init_frequency,
            last_generated: None,
        }
    }

    #[inline]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Check whether a new HK packet needs to be generated.
    pub fn needs_generation(&mut self) -> bool {
        if !self.enabled {
            return false;
        }
        if self.last_generated.is_none() {
            self.last_generated = Some(Instant::now());
            return true;
        }
        let last_generated = self.last_generated.unwrap();
        if Instant::now() - last_generated >= self.frequency {
            self.last_generated = Some(Instant::now());
            return true;
        }
        false
    }
}

#[derive(Debug)]
pub struct TmtcQueues {
    pub tc_rx: mpsc::Receiver<CcsdsTcPacketOwned>,
    pub tm_tx: mpsc::SyncSender<CcsdsTmPacketOwned>,
}

#[derive(Debug)]
pub struct ModeHelper<Mode, TransitionState> {
    pub current: Mode,
    pub target: Option<Mode>,
    pub tc_commander: Option<CcsdsPacketIdAndPsc>,
    pub transition_start: Option<Instant>,
    pub timeout: Duration,
    pub transition_state: TransitionState,
}

impl<Mode: Copy + Clone, TransitionState: Default> ModeHelper<Mode, TransitionState> {
    pub fn new(init_mode: Mode, timeout: Duration) -> Self {
        Self {
            current: init_mode,
            target: Default::default(),
            tc_commander: Default::default(),
            transition_start: None,
            timeout,
            transition_state: Default::default(),
        }
    }

    pub fn start(&mut self, target: Mode) {
        self.target = Some(target);
        self.transition_start = Some(Instant::now());
        self.transition_state = TransitionState::default();
    }

    #[inline]
    pub fn transition_active(&self) -> bool {
        self.target.is_some()
    }

    pub fn timed_out(&self) -> bool {
        if self.target.is_none() {
            return false;
        }
        if let Some(transition_start) = self.transition_start {
            return Instant::now() - transition_start >= self.timeout;
        }
        false
    }

    pub fn finish(&mut self, success: bool) -> Option<CcsdsPacketIdAndPsc> {
        self.target?;
        if success {
            self.current = self.target.take().unwrap();
        } else {
            self.target = None;
        }
        self.transition_state = Default::default();
        self.transition_start = None;
        self.tc_commander.take()
    }
}
