//! Application code shared by the STM32H7 Nucleo applications. Each application wraps the
//! async functions inside tasks of its own executor.
#![no_std]
extern crate alloc;

pub mod leds;
pub mod net;
pub mod sim_client;
pub mod tmtc;
