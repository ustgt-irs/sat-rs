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
#[cfg(feature = "std")]
pub mod executable;
pub mod fdir;
pub mod hal;
pub mod health;
/// Helpers to track when housekeeping sets need to be regenerated.
pub mod hk;
pub mod legacy;
pub mod mode;
#[cfg(feature = "std")]
pub mod mode_tree;
pub mod params;
pub mod pool;
pub mod queue;
pub mod request;
#[cfg(feature = "alloc")]
pub mod scheduling;
#[cfg(feature = "alloc")]
pub mod subsystem;
pub mod time;
pub mod tmtc;

pub use spacepackets;

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
