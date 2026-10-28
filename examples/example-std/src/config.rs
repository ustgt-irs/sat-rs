use arbitrary_int::u11;
use lazy_static::lazy_static;
use satrs::spacepackets::{PacketId, PacketType};
use std::{collections::HashSet, net::Ipv4Addr};
use strum::IntoEnumIterator as _;

use num_enum::{IntoPrimitive, TryFromPrimitive};
use satrs::pool::{StaticMemoryPool, StaticPoolConfig};

#[derive(Copy, Clone, PartialEq, Eq, Debug, TryFromPrimitive, IntoPrimitive)]
#[repr(u8)]
pub enum CustomPusServiceId {
    Mode = 200,
    Health = 201,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum RequestTargetId {
    AcsSubsystem = 1,
}

pub const AOCS_APID: u16 = 1;

pub const OBSW_SERVER_ADDR: Ipv4Addr = Ipv4Addr::UNSPECIFIED;
pub const SERVER_PORT: u16 = 7301;

lazy_static! {
    pub static ref PACKET_ID_VALIDATOR: HashSet<PacketId> = {
        let mut set = HashSet::new();
        for id in types::Apid::iter() {
            set.insert(PacketId::new(PacketType::Tc, true, u11::new(id as u16)));
        }
        set
    };
    pub static ref APID_VALIDATOR: HashSet<u16> = {
        let mut set = HashSet::new();
        for id in types::Apid::iter() {
            set.insert(id as u16);
        }
        set
    };
}

pub mod components {
    use satrs::ComponentId;

    pub const NO_SENDER: ComponentId = ComponentId::MAX;
}

pub mod pool {
    use super::*;
    pub fn create_static_pools() -> (StaticMemoryPool, StaticMemoryPool) {
        (
            StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
                vec![
                    (30, 32),
                    (15, 64),
                    (15, 128),
                    (15, 256),
                    (15, 1024),
                    (15, 2048),
                ],
                true,
            )),
            StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
                vec![
                    (30, 32),
                    (15, 64),
                    (15, 128),
                    (15, 256),
                    (15, 1024),
                    (15, 2048),
                ],
                true,
            )),
        )
    }

    pub fn create_sched_tc_pool() -> StaticMemoryPool {
        StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![
                (30, 32),
                (15, 64),
                (15, 128),
                (15, 256),
                (15, 1024),
                (15, 2048),
            ],
            true,
        ))
    }
}

pub mod tasks {
    pub const FREQ_MS_UDP_TMTC: u64 = 200;
    pub const FREQ_MS_AOCS: u64 = 200;
    pub const FREQ_MS_CONTROLLER: u64 = 200;
    pub const SIM_CLIENT_IDLE_DELAY_MS: u64 = 5;
}
