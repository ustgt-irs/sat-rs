//! # sat-rs: A helper library to build on-board software for remote systems
//!
//! The [satrs-book](https://documentation.irs.uni-stuttgart.de/projects/sat-rs/book/) contains
//! high-level information about this library.
#![no_std]
#![cfg_attr(docsrs, feature(doc_cfg))]
#[cfg(any(feature = "alloc", test))]
extern crate alloc;
#[cfg(feature = "alloc")]
extern crate downcast_rs;
#[cfg(any(feature = "std", test))]
extern crate std;

pub mod ccsds;
pub mod encoding;
pub mod fdir;
pub mod hal;
pub mod health;
pub mod hk;
pub mod legacy;
pub mod mode;
#[cfg(feature = "std")]
pub mod mode_tree;
pub mod params;
pub mod pool;
pub mod queue;
pub mod request;
#[cfg(feature = "std")]
pub mod subsystem;
pub mod time;
pub mod tmtc;

// Only the serde helpers of the alloc and heapless event types use postcard. Cargo features
// cannot express that condition, so this silences the unused dependency lint.
#[cfg(all(feature = "serde", not(any(feature = "alloc", feature = "heapless"))))]
use postcard as _;

pub use spacepackets;
// Used by the exported pool macros, so users do not need their own static_cell dependency.
#[doc(hidden)]
pub use static_cell;

use spacepackets::PacketId;

/// Generic handling status for an object which is able to continuosly handle a queue to handle
/// request or replies until the queue is empty.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum HandlingStatus {
    HandledOne,
    Empty,
}

/// Generic component ID type.
pub type ComponentId = u32;

pub trait ValidatorU16Id {
    fn validate(&self, id: u16) -> bool;
}

#[cfg(feature = "alloc")]
impl ValidatorU16Id for alloc::vec::Vec<u16> {
    fn validate(&self, id: u16) -> bool {
        self.contains(&id)
    }
}

#[cfg(feature = "alloc")]
impl ValidatorU16Id for hashbrown::HashSet<u16> {
    fn validate(&self, id: u16) -> bool {
        self.contains(&id)
    }
}

impl ValidatorU16Id for u16 {
    fn validate(&self, id: u16) -> bool {
        id == *self
    }
}

impl ValidatorU16Id for &u16 {
    fn validate(&self, id: u16) -> bool {
        id == **self
    }
}

impl ValidatorU16Id for [u16] {
    fn validate(&self, id: u16) -> bool {
        self.binary_search(&id).is_ok()
    }
}

impl ValidatorU16Id for &[u16] {
    fn validate(&self, id: u16) -> bool {
        self.binary_search(&id).is_ok()
    }
}

#[cfg(feature = "alloc")]
impl ValidatorU16Id for alloc::vec::Vec<spacepackets::PacketId> {
    fn validate(&self, packet_id: u16) -> bool {
        self.contains(&PacketId::from(packet_id))
    }
}

#[cfg(feature = "alloc")]
impl ValidatorU16Id for hashbrown::HashSet<spacepackets::PacketId> {
    fn validate(&self, packet_id: u16) -> bool {
        self.contains(&PacketId::from(packet_id))
    }
}

#[cfg(feature = "std")]
impl ValidatorU16Id for std::collections::HashSet<PacketId> {
    fn validate(&self, packet_id: u16) -> bool {
        self.contains(&PacketId::from(packet_id))
    }
}

impl ValidatorU16Id for [PacketId] {
    fn validate(&self, packet_id: u16) -> bool {
        self.binary_search(&PacketId::from(packet_id)).is_ok()
    }
}

impl ValidatorU16Id for &[PacketId] {
    fn validate(&self, packet_id: u16) -> bool {
        self.binary_search(&PacketId::from(packet_id)).is_ok()
    }
}
