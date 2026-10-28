//! # Pool implementation providing memory pools for packet storage.
//!
//! This module provides generic abstractions for memory pools which provide a storage
//! machanism for variable sized data like Telemetry and Telecommand (TMTC) packets. The core
//! abstraction for this is the [PoolProvider] trait.
//!
//! Currently, two concrete [PoolProvider] implementations are provided:
//!
//!  - The [StaticMemoryPool] required [alloc] support but pre-allocated all required memory
//!    and does not perform dynamic run-time allocations for the storage of TMTC packets.
//!  - The [StaticHeaplessMemoryPool] which can be grown by user provided static storage.
//!
//! # Example for the [StaticMemoryPool]
//!
//! ```
//! use satrs::pool::{PoolProvider, StaticMemoryPool, StaticPoolConfig};
//!
//! // 4 buckets of 4 bytes, 2 of 8 bytes and 1 of 16 bytes
//! let pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(vec![(4, 4), (2, 8), (1, 16)], false);
//! let mut mem_pool = StaticMemoryPool::new(pool_cfg);
//! let mut read_buf: [u8; 16] = [0; 16];
//! let mut addr;
//! {
//!     // Add new data to the pool
//!     let mut example_data = [0; 4];
//!     example_data[0] = 42;
//!     let res = mem_pool.add(&example_data);
//!     assert!(res.is_ok());
//!     addr = res.unwrap();
//! }
//!
//! {
//!     // Read the store data back
//!     let res = mem_pool.read(&addr, &mut read_buf);
//!     assert!(res.is_ok());
//!     let read_bytes = res.unwrap();
//!     assert_eq!(read_bytes, 4);
//!     assert_eq!(read_buf[0], 42);
//!     // Modify the stored data
//!     let res = mem_pool.modify(&addr, |buf| {
//!         buf[0] = 12;
//!     });
//!     assert!(res.is_ok());
//! }
//!
//! {
//!     // Read the modified data back
//!     let res = mem_pool.read(&addr, &mut read_buf);
//!     assert!(res.is_ok());
//!     let read_bytes = res.unwrap();
//!     assert_eq!(read_bytes, 4);
//!     assert_eq!(read_buf[0], 12);
//! }
//!
//! // Delete the stored data
//! mem_pool.delete(addr);
//!
//! // Get a free element in the pool with an appropriate size
//! {
//!     let res = mem_pool.free_element(12, |buf| {
//!         buf[0] = 7;
//!     });
//!     assert!(res.is_ok());
//!     addr = res.unwrap();
//! }
//!
//! // Read back the data
//! {
//!     // Read the store data back
//!     let res = mem_pool.read(&addr, &mut read_buf);
//!     assert!(res.is_ok());
//!     let read_bytes = res.unwrap();
//!     assert_eq!(read_bytes, 12);
//!     assert_eq!(read_buf[0], 7);
//! }
//! ```
#[cfg(feature = "alloc")]
pub use alloc_mod::*;
use core::fmt::{Display, Formatter};
use delegate::delegate;
#[cfg(feature = "heapless")]
pub use heapless_mod::*;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use spacepackets::ByteConversionError;
#[cfg(feature = "std")]
use std::error::Error;

type NumBlocks = u16;
pub type PoolAddr = u64;

/// Simple address type used for transactions with the local pool.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct StaticPoolAddr {
    pub(crate) pool_idx: u16,
    pub(crate) packet_idx: NumBlocks,
}

impl StaticPoolAddr {
    pub const INVALID_ADDR: u32 = 0xFFFFFFFF;

    pub fn raw(&self) -> u32 {
        ((self.pool_idx as u32) << 16) | self.packet_idx as u32
    }
}

impl From<StaticPoolAddr> for PoolAddr {
    fn from(value: StaticPoolAddr) -> Self {
        ((value.pool_idx as u64) << 16) | value.packet_idx as u64
    }
}

impl From<PoolAddr> for StaticPoolAddr {
    fn from(value: PoolAddr) -> Self {
        Self {
            pool_idx: ((value >> 16) & 0xff) as u16,
            packet_idx: (value & 0xff) as u16,
        }
    }
}

impl Display for StaticPoolAddr {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "StoreAddr(pool index: {}, packet index: {})",
            self.pool_idx, self.packet_idx
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum StoreIdError {
    InvalidSubpool(u16),
    InvalidPacketIdx(u16),
}

impl Display for StoreIdError {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            StoreIdError::InvalidSubpool(pool) => {
                write!(f, "invalid subpool, index: {pool}")
            }
            StoreIdError::InvalidPacketIdx(packet_idx) => {
                write!(f, "invalid packet index: {packet_idx}")
            }
        }
    }
}

#[cfg(feature = "std")]
impl Error for StoreIdError {}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum PoolError {
    /// Requested data block is too large
    #[error("data to store with size {0} is too large")]
    DataTooLarge(usize),
    /// The store is full. Contains the index of the full subpool
    #[error("store does not have any capacity")]
    StoreFull(u16),
    /// The store can not hold any data.
    #[error("store does not have any capacity")]
    NoCapacity,
    /// Store ID is invalid. This also includes partial errors where only the subpool is invalid
    #[error("invalid store ID: {0}, address: {1:?}")]
    InvalidStoreId(StoreIdError, Option<PoolAddr>),
    /// Valid subpool and packet index, but no data is stored at the given address
    #[error("no data exists at address {0:?}")]
    DataDoesNotExist(PoolAddr),
    #[error("byte conversion error: {0}")]
    ByteConversion(#[from] ByteConversionError),
    #[error("lock error")]
    LockError,
    /// Internal or configuration errors
    #[error("lock error")]
    InternalError(u32),
}

/// Generic trait for pool providers which provide memory pools for variable sized packet data.
///
/// It specifies a basic API to [Self::add], [Self::modify], [Self::read] and [Self::delete] data
/// in the store at its core. The API was designed so internal optimizations can be performed
/// more easily and that is is also possible to make the pool structure [Sync] without the whole
/// pool structure being wrapped inside a lock.
pub trait PoolProvider {
    /// Add new data to the pool. The provider should attempt to reserve a memory block with the
    /// appropriate size and then copy the given data to the block. Yields a [PoolAddr] which can
    /// be used to access the data stored in the pool
    fn add(&mut self, data: &[u8]) -> Result<PoolAddr, PoolError>;

    /// The provider should attempt to reserve a free memory block with the appropriate size first.
    /// It then executes a user-provided closure and passes a mutable reference to that memory
    /// block to the closure. This allows the user to write data to the memory block.
    /// The function should yield a [PoolAddr] which can be used to access the data stored in the
    /// pool.
    fn free_element<W: FnMut(&mut [u8])>(
        &mut self,
        len: usize,
        writer: W,
    ) -> Result<PoolAddr, PoolError>;

    /// Modify data added previously using a given [PoolAddr]. The provider should use the store
    /// address to determine if a memory block exists for that address. If it does, it should
    /// call the user-provided closure and pass a mutable reference to the memory block
    /// to the closure. This allows the user to modify the memory block.
    fn modify<U: FnMut(&mut [u8])>(&mut self, addr: &PoolAddr, updater: U)
    -> Result<(), PoolError>;

    /// The provider should copy the data from the memory block to the user-provided buffer if
    /// it exists.
    fn read(&self, addr: &PoolAddr, buf: &mut [u8]) -> Result<usize, PoolError>;

    /// Delete data inside the pool given a [PoolAddr].
    fn delete(&mut self, addr: PoolAddr) -> Result<(), PoolError>;

    fn clear(&mut self) -> Result<(), PoolError>;

    fn has_element_at(&self, addr: &PoolAddr) -> Result<bool, PoolError>;

    /// Retrieve the length of the data at the given store address.
    fn len_of_data(&self, addr: &PoolAddr) -> Result<usize, PoolError>;

    #[cfg(feature = "alloc")]
    fn read_as_vec(&self, addr: &PoolAddr) -> Result<alloc::vec::Vec<u8>, PoolError> {
        let mut vec = alloc::vec![0; self.len_of_data(addr)?];
        self.read(addr, &mut vec)?;
        Ok(vec)
    }
}

/// Extension trait which adds guarded pool access classes.
pub trait PoolProviderWithGuards: PoolProvider {
    /// This function behaves like [PoolProvider::read], but consumes the provided address
    /// and returns a RAII conformant guard object.
    ///
    /// Unless the guard [PoolRwGuard::release] method is called, the data for the
    /// given address will be deleted automatically when the guard is dropped.
    /// This can prevent memory leaks. Users can read the data and release the guard
    /// if the data in the store is valid for further processing. If the data is faulty, no
    /// manual deletion is necessary when returning from a processing function prematurely.
    fn read_with_guard(&mut self, addr: PoolAddr) -> PoolGuard<'_, Self>;

    /// This function behaves like [PoolProvider::modify], but consumes the provided
    /// address and returns a RAII conformant guard object.
    ///
    /// Unless the guard [PoolRwGuard::release] method is called, the data for the
    /// given address will be deleted automatically when the guard is dropped.
    /// This can prevent memory leaks. Users can read (and modify) the data and release the guard
    /// if the data in the store is valid for further processing. If the data is faulty, no
    /// manual deletion is necessary when returning from a processing function prematurely.
    fn modify_with_guard(&mut self, addr: PoolAddr) -> PoolRwGuard<'_, Self>;
}

pub struct PoolGuard<'a, MemProvider: PoolProvider + ?Sized> {
    pool: &'a mut MemProvider,
    pub addr: PoolAddr,
    no_deletion: bool,
    deletion_failed_error: Option<PoolError>,
}

/// This helper object can be used to safely access pool data without worrying about memory
/// leaks.
impl<'a, MemProvider: PoolProvider> PoolGuard<'a, MemProvider> {
    pub fn new(pool: &'a mut MemProvider, addr: PoolAddr) -> Self {
        Self {
            pool,
            addr,
            no_deletion: false,
            deletion_failed_error: None,
        }
    }

    pub fn read(&self, buf: &mut [u8]) -> Result<usize, PoolError> {
        self.pool.read(&self.addr, buf)
    }

    #[cfg(feature = "alloc")]
    pub fn read_as_vec(&self) -> Result<alloc::vec::Vec<u8>, PoolError> {
        self.pool.read_as_vec(&self.addr)
    }

    /// Releasing the pool guard will disable the automatic deletion of the data when the guard
    /// is dropped.
    pub fn release(&mut self) {
        self.no_deletion = true;
    }
}

impl<MemProvider: PoolProvider + ?Sized> Drop for PoolGuard<'_, MemProvider> {
    fn drop(&mut self) {
        if !self.no_deletion
            && let Err(e) = self.pool.delete(self.addr)
        {
            self.deletion_failed_error = Some(e);
        }
    }
}

pub struct PoolRwGuard<'a, MemProvider: PoolProvider + ?Sized> {
    guard: PoolGuard<'a, MemProvider>,
}

impl<'a, MemProvider: PoolProvider> PoolRwGuard<'a, MemProvider> {
    pub fn new(pool: &'a mut MemProvider, addr: PoolAddr) -> Self {
        Self {
            guard: PoolGuard::new(pool, addr),
        }
    }

    pub fn update<U: FnMut(&mut [u8])>(&mut self, updater: &mut U) -> Result<(), PoolError> {
        self.guard.pool.modify(&self.guard.addr, updater)
    }

    delegate!(
        to self.guard {
            pub fn read(&self, buf: &mut [u8]) -> Result<usize, PoolError>;
            /// Releasing the pool guard will disable the automatic deletion of the data when the guard
            /// is dropped.
            pub fn release(&mut self);
        }
    );
}

type UsedBlockSize = usize;
pub const STORE_FREE: UsedBlockSize = UsedBlockSize::MAX;
pub const MAX_BLOCK_SIZE: UsedBlockSize = STORE_FREE - 1;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct SubpoolConfig {
    num_blocks: NumBlocks,
    block_size: usize,
}

impl SubpoolConfig {
    pub const fn new(num_blocks: NumBlocks, block_size: usize) -> Self {
        Self {
            num_blocks,
            block_size,
        }
    }
}

#[cfg(feature = "heapless")]
pub mod heapless_mod {
    use super::*;

    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    #[cfg_attr(feature = "defmt", derive(defmt::Format))]
    pub struct PoolIsFull;

    /// Helper macro to generate static buffers for the [crate::pool::StaticHeaplessMemoryPool].
    #[macro_export]
    macro_rules! static_subpool {
        ($pool_name: ident, $sizes_list_name: ident, $num_blocks: expr, $block_size: expr) => {
            static $pool_name: static_cell::ConstStaticCell<[u8; $num_blocks * $block_size]> =
                static_cell::ConstStaticCell::new([0; $num_blocks * $block_size]);
            static $sizes_list_name: static_cell::ConstStaticCell<[usize; $num_blocks]> =
                static_cell::ConstStaticCell::new([$crate::pool::STORE_FREE; $num_blocks]);
        };
        ($pool_name: ident, $sizes_list_name: ident, $num_blocks: expr, $block_size: expr, $meta_data: meta) => {
            #[$meta_data]
            static $pool_name: static_cell::ConstStaticCell<[u8; $num_blocks * $block_size]> =
                static_cell::ConstStaticCell::new([0; $num_blocks * $block_size]);
            #[$meta_data]
            static $sizes_list_name: static_cell::ConstStaticCell<[usize; $num_blocks]> =
                static_cell::ConstStaticCell::new([$crate::pool::STORE_FREE; $num_blocks]);
        };
    }

    /// A static memory pool similar to [super::StaticMemoryPool] which does not reply on
    /// heap allocations.
    ///
    /// This implementation is empty after constructions and must be grown with user-provided
    /// static mutable memory using the [Self::grow] method. The (maximum) number of subpools
    /// has to specified via a generic parameter.
    ///
    /// The [crate::static_subpool] macro can be used to avoid some boilerplate code for
    /// specifying the static memory blocks for the pool.
    ///
    /// ```
    /// use satrs::pool::{PoolProvider, StaticHeaplessMemoryPool};
    /// use satrs::static_subpool;
    ///
    /// const SUBPOOL_SMALL_NUM_BLOCKS: u16 = 16;
    /// const SUBPOOL_SMALL_BLOCK_SIZE: usize = 32;
    /// static_subpool!(
    ///     SUBPOOL_SMALL,
    ///     SUBPOOL_SMALL_SIZES,
    ///     SUBPOOL_SMALL_NUM_BLOCKS as usize,
    ///     SUBPOOL_SMALL_BLOCK_SIZE
    /// );
    /// const SUBPOOL_LARGE_NUM_BLOCKS: u16 = 8;
    /// const SUBPOOL_LARGE_BLOCK_SIZE: usize = 64;
    /// static_subpool!(
    ///     SUBPOOL_LARGE,
    ///     SUBPOOL_LARGE_SIZES,
    ///     SUBPOOL_LARGE_NUM_BLOCKS as usize,
    ///     SUBPOOL_LARGE_BLOCK_SIZE
    /// );
    ///
    /// let mut mem_pool: StaticHeaplessMemoryPool<2> = StaticHeaplessMemoryPool::new(true);
    /// mem_pool.grow(
    ///     SUBPOOL_SMALL.take(),
    ///     SUBPOOL_SMALL_SIZES.take(),
    ///     SUBPOOL_SMALL_NUM_BLOCKS,
    ///     false
    /// ).unwrap();
    /// mem_pool.grow(
    ///     SUBPOOL_LARGE.take(),
    ///     SUBPOOL_LARGE_SIZES.take(),
    ///     SUBPOOL_LARGE_NUM_BLOCKS,
    ///     false
    /// ).unwrap();
    ///
    /// let mut read_buf: [u8; 16] = [0; 16];
    /// let mut addr;
    /// {
    ///     // Add new data to the pool
    ///     let mut example_data = [0; 4];
    ///     example_data[0] = 42;
    ///     let res = mem_pool.add(&example_data);
    ///     assert!(res.is_ok());
    ///     addr = res.unwrap();
    /// }
    ///
    /// {
    ///     // Read the store data back
    ///     let res = mem_pool.read(&addr, &mut read_buf);
    ///     assert!(res.is_ok());
    ///     let read_bytes = res.unwrap();
    ///     assert_eq!(read_bytes, 4);
    ///     assert_eq!(read_buf[0], 42);
    ///     // Modify the stored data
    ///     let res = mem_pool.modify(&addr, |buf| {
    ///         buf[0] = 12;
    ///     });
    ///     assert!(res.is_ok());
    /// }
    ///
    /// {
    ///     // Read the modified data back
    ///     let res = mem_pool.read(&addr, &mut read_buf);
    ///     assert!(res.is_ok());
    ///     let read_bytes = res.unwrap();
    ///     assert_eq!(read_bytes, 4);
    ///     assert_eq!(read_buf[0], 12);
    /// }
    ///
    /// // Delete the stored data
    /// mem_pool.delete(addr);
    /// ```
    pub struct StaticHeaplessMemoryPool<const MAX_NUM_SUBPOOLS: usize> {
        pool: heapless::Vec<(SubpoolConfig, &'static mut [u8]), MAX_NUM_SUBPOOLS>,
        sizes_lists: heapless::Vec<&'static mut [UsedBlockSize], MAX_NUM_SUBPOOLS>,
        spill_to_higher_subpools: bool,
    }

    impl<const MAX_NUM_SUBPOOLS: usize> StaticHeaplessMemoryPool<MAX_NUM_SUBPOOLS> {
        pub fn new(spill_to_higher_subpools: bool) -> Self {
            Self {
                pool: heapless::Vec::new(),
                sizes_lists: heapless::Vec::new(),
                spill_to_higher_subpools,
            }
        }

        /// Grow the memory pool using statically allocated memory.
        ///
        /// Please note that this API assumes the static memory was initialized properly by the
        /// user. The sizes list in particular must be set to the [super::STORE_FREE] value
        /// by the user for the data structure to work properly. This method will take care of
        /// setting all the values inside the sizes list depending on the passed parameters.
        ///
        /// # Parameters
        ///
        /// * `subpool_memory` - Static memory for a particular subpool to store the actual data.
        /// * `sizes_list` - Static sizes list structure to store the size of the data which is
        ///   actually stored.
        /// * `num_blocks ` - The number of memory blocks inside the subpool.
        /// * `set_sizes_list_to_all_free` - If this is set to true, the method will take care
        ///   of setting all values in the sizes list to [super::STORE_FREE]. This does not have
        ///   to be done if the user initializes the sizes list to that value themselves.
        pub fn grow(
            &mut self,
            subpool_memory: &'static mut [u8],
            sizes_list: &'static mut [UsedBlockSize],
            num_blocks: NumBlocks,
            set_sizes_list_to_all_free: bool,
        ) -> Result<(), PoolIsFull> {
            assert_eq!(
                (subpool_memory.len() % num_blocks as usize),
                0,
                "pool slice length must be multiple of number of blocks"
            );
            assert_eq!(
                num_blocks as usize,
                sizes_list.len(),
                "used block size list slice must be of same length as number of blocks"
            );
            let subpool_config = SubpoolConfig {
                num_blocks,
                block_size: subpool_memory.len() / num_blocks as usize,
            };
            self.pool
                .push((subpool_config, subpool_memory))
                .map_err(|_| PoolIsFull)?;
            if set_sizes_list_to_all_free {
                sizes_list.fill(STORE_FREE);
            }
            self.sizes_lists.push(sizes_list).map_err(|_| PoolIsFull)?;
            Ok(())
        }

        fn reserve(&mut self, data_len: usize) -> Result<StaticPoolAddr, PoolError> {
            if self.pool.is_empty() {
                return Err(PoolError::NoCapacity);
            }
            let mut subpool_idx = self.find_subpool(data_len, 0)?;

            if self.spill_to_higher_subpools {
                while let Err(PoolError::StoreFull(_)) = self.find_empty(subpool_idx) {
                    if (subpool_idx + 1) as usize == self.sizes_lists.len() {
                        return Err(PoolError::StoreFull(subpool_idx));
                    }
                    subpool_idx += 1;
                }
            }

            let (slot, size_slot_ref) = self.find_empty(subpool_idx)?;
            *size_slot_ref = data_len;
            Ok(StaticPoolAddr {
                pool_idx: subpool_idx,
                packet_idx: slot,
            })
        }

        fn addr_check(&self, addr: &StaticPoolAddr) -> Result<usize, PoolError> {
            self.validate_addr(addr)?;
            let pool_idx = addr.pool_idx as usize;
            let size_list = self.sizes_lists.get(pool_idx).unwrap();
            let curr_size = size_list[addr.packet_idx as usize];
            if curr_size == STORE_FREE {
                return Err(PoolError::DataDoesNotExist(PoolAddr::from(*addr)));
            }
            Ok(curr_size)
        }

        fn validate_addr(&self, addr: &StaticPoolAddr) -> Result<(), PoolError> {
            let pool_idx = addr.pool_idx as usize;
            if pool_idx >= self.pool.len() {
                return Err(PoolError::InvalidStoreId(
                    StoreIdError::InvalidSubpool(addr.pool_idx),
                    Some(PoolAddr::from(*addr)),
                ));
            }
            if addr.packet_idx >= self.pool[addr.pool_idx as usize].0.num_blocks {
                return Err(PoolError::InvalidStoreId(
                    StoreIdError::InvalidPacketIdx(addr.packet_idx),
                    Some(PoolAddr::from(*addr)),
                ));
            }
            Ok(())
        }

        fn find_subpool(&self, req_size: usize, start_at_subpool: u16) -> Result<u16, PoolError> {
            for (i, &(pool_cfg, _)) in self.pool.iter().enumerate() {
                if i < start_at_subpool as usize {
                    continue;
                }
                if pool_cfg.block_size >= req_size {
                    return Ok(i as u16);
                }
            }
            Err(PoolError::DataTooLarge(req_size))
        }

        fn write(&mut self, addr: &StaticPoolAddr, data: &[u8]) -> Result<(), PoolError> {
            let packet_pos = self.raw_pos(addr).ok_or(PoolError::InternalError(0))?;
            let (_elem_size, subpool) = self
                .pool
                .get_mut(addr.pool_idx as usize)
                .ok_or(PoolError::InternalError(1))?;
            let pool_slice = &mut subpool[packet_pos..packet_pos + data.len()];
            pool_slice.copy_from_slice(data);
            Ok(())
        }

        fn find_empty(&mut self, subpool: u16) -> Result<(u16, &mut usize), PoolError> {
            if let Some(size_list) = self.sizes_lists.get_mut(subpool as usize) {
                for (i, elem_size) in size_list.iter_mut().enumerate() {
                    if *elem_size == STORE_FREE {
                        return Ok((i as u16, elem_size));
                    }
                }
            } else {
                return Err(PoolError::InvalidStoreId(
                    StoreIdError::InvalidSubpool(subpool),
                    None,
                ));
            }
            Err(PoolError::StoreFull(subpool))
        }

        fn raw_pos(&self, addr: &StaticPoolAddr) -> Option<usize> {
            let (pool_cfg, _) = self.pool.get(addr.pool_idx as usize)?;
            Some(addr.packet_idx as usize * pool_cfg.block_size)
        }
    }

    impl<const MAX_NUM_SUBPOOLS: usize> PoolProvider for StaticHeaplessMemoryPool<MAX_NUM_SUBPOOLS> {
        fn add(&mut self, data: &[u8]) -> Result<PoolAddr, PoolError> {
            let data_len = data.len();
            if data_len > MAX_BLOCK_SIZE {
                return Err(PoolError::DataTooLarge(data_len));
            }
            let addr = self.reserve(data_len)?;
            self.write(&addr, data)?;
            Ok(addr.into())
        }

        fn free_element<W: FnMut(&mut [u8])>(
            &mut self,
            len: usize,
            mut writer: W,
        ) -> Result<PoolAddr, PoolError> {
            if len > MAX_BLOCK_SIZE {
                return Err(PoolError::DataTooLarge(len));
            }
            let addr = self.reserve(len)?;
            let raw_pos = self.raw_pos(&addr).unwrap();
            let block =
                &mut self.pool.get_mut(addr.pool_idx as usize).unwrap().1[raw_pos..raw_pos + len];
            writer(block);
            Ok(addr.into())
        }

        fn modify<U: FnMut(&mut [u8])>(
            &mut self,
            addr: &PoolAddr,
            mut updater: U,
        ) -> Result<(), PoolError> {
            let addr = StaticPoolAddr::from(*addr);
            let curr_size = self.addr_check(&addr)?;
            let raw_pos = self.raw_pos(&addr).unwrap();
            let block = &mut self.pool.get_mut(addr.pool_idx as usize).unwrap().1
                [raw_pos..raw_pos + curr_size];
            updater(block);
            Ok(())
        }

        fn read(&self, addr: &PoolAddr, buf: &mut [u8]) -> Result<usize, PoolError> {
            let addr = StaticPoolAddr::from(*addr);
            let curr_size = self.addr_check(&addr)?;
            if buf.len() < curr_size {
                return Err(ByteConversionError::ToSliceTooSmall {
                    found: buf.len(),
                    expected: curr_size,
                }
                .into());
            }
            let raw_pos = self.raw_pos(&addr).unwrap();
            let block =
                &self.pool.get(addr.pool_idx as usize).unwrap().1[raw_pos..raw_pos + curr_size];
            //block.copy_from_slice(&src);
            buf[..curr_size].copy_from_slice(block);
            Ok(curr_size)
        }

        fn delete(&mut self, addr: PoolAddr) -> Result<(), PoolError> {
            let addr = StaticPoolAddr::from(addr);
            self.addr_check(&addr)?;
            let subpool_cfg = self.pool.get(addr.pool_idx as usize).unwrap().0;
            let raw_pos = self.raw_pos(&addr).unwrap();
            let block = &mut self.pool.get_mut(addr.pool_idx as usize).unwrap().1
                [raw_pos..raw_pos + subpool_cfg.block_size];
            let size_list = self.sizes_lists.get_mut(addr.pool_idx as usize).unwrap();
            size_list[addr.packet_idx as usize] = STORE_FREE;
            block.fill(0);
            Ok(())
        }

        fn clear(&mut self) -> Result<(), PoolError> {
            for size in self.sizes_lists.iter_mut() {
                size.fill(STORE_FREE);
            }
            Ok(())
        }

        fn has_element_at(&self, addr: &PoolAddr) -> Result<bool, PoolError> {
            let addr = StaticPoolAddr::from(*addr);
            self.validate_addr(&addr)?;
            let pool_idx = addr.pool_idx as usize;
            let size_list = self.sizes_lists.get(pool_idx).unwrap();
            let curr_size = size_list[addr.packet_idx as usize];
            if curr_size == STORE_FREE {
                return Ok(false);
            }
            Ok(true)
        }

        fn len_of_data(&self, addr: &PoolAddr) -> Result<usize, PoolError> {
            let addr = StaticPoolAddr::from(*addr);
            self.validate_addr(&addr)?;
            let pool_idx = addr.pool_idx as usize;
            let size_list = self.sizes_lists.get(pool_idx).unwrap();
            let size = size_list[addr.packet_idx as usize];
            Ok(match size {
                STORE_FREE => 0,
                _ => size,
            })
        }
    }

    impl<const MAX_NUM_SUBPOOLS: usize> PoolProviderWithGuards
        for StaticHeaplessMemoryPool<MAX_NUM_SUBPOOLS>
    {
        fn modify_with_guard(&mut self, addr: PoolAddr) -> PoolRwGuard<'_, Self> {
            PoolRwGuard::new(self, addr)
        }

        fn read_with_guard(&mut self, addr: PoolAddr) -> PoolGuard<'_, Self> {
            PoolGuard::new(self, addr)
        }
    }
}

#[cfg(feature = "alloc")]
mod alloc_mod {
    use super::*;
    use crate::pool::{PoolAddr, PoolError, StoreIdError};
    use alloc::vec;
    use alloc::vec::Vec;
    use spacepackets::ByteConversionError;
    #[cfg(feature = "std")]
    use std::sync::{Arc, RwLock};

    #[cfg(feature = "std")]
    pub type SharedStaticMemoryPool = Arc<RwLock<StaticMemoryPool>>;

    /// Configuration structure of the [static memory pool][StaticMemoryPool]
    ///
    /// # Parameters
    ///
    /// * `cfg` - Vector of tuples which represent a subpool. The first entry in the tuple specifies
    ///   the number of memory blocks in the subpool, the second entry the size of the blocks
    /// * `spill_to_higher_subpools` - Specifies whether data will be spilled to higher subpools
    ///   if the next fitting subpool is full. This is useful to ensure the pool remains useful
    ///   for all data sizes as long as possible. However, an undesirable side-effect might be
    ///   the chocking of larger subpools by underdimensioned smaller subpools.
    #[derive(Debug, Clone)]
    pub struct StaticPoolConfig {
        cfg: Vec<SubpoolConfig>,
        spill_to_higher_subpools: bool,
    }

    impl StaticPoolConfig {
        pub fn new(cfg: Vec<SubpoolConfig>, spill_to_higher_subpools: bool) -> Self {
            StaticPoolConfig {
                cfg,
                spill_to_higher_subpools,
            }
        }

        pub fn new_from_subpool_cfg_tuples(
            cfg: Vec<(NumBlocks, usize)>,
            spill_to_higher_subpools: bool,
        ) -> Self {
            StaticPoolConfig {
                cfg: cfg
                    .iter()
                    .map(|(num_blocks, block_size)| SubpoolConfig::new(*num_blocks, *block_size))
                    .collect(),
                spill_to_higher_subpools,
            }
        }

        pub fn subpool_cfg(&self) -> &Vec<SubpoolConfig> {
            &self.cfg
        }

        pub fn sanitize(&mut self) -> usize {
            self.cfg.retain(|&subpool_cfg| {
                subpool_cfg.num_blocks > 0 && subpool_cfg.block_size < MAX_BLOCK_SIZE
            });
            self.cfg.sort_unstable_by(|&cfg0, &cfg1| {
                cfg0.block_size.partial_cmp(&cfg1.block_size).unwrap()
            });
            self.cfg.len()
        }
    }

    /// Pool implementation providing sub-pools with fixed size memory blocks.
    ///
    /// This is a simple memory pool implementation which pre-allocates all subpools using a given
    /// pool configuration. After the pre-allocation, no dynamic memory allocation will be
    /// performed during run-time. This makes the implementation suitable for real-time
    /// applications and embedded environments.
    ///
    /// The subpool bucket sizes only denote the maximum possible data size being stored inside
    /// them and the pool implementation will still track the size of the data stored inside it.
    /// The implementation will generally determine the best fitting subpool for given data to
    /// add. Currently, the pool does not support spilling to larger subpools if the closest
    /// fitting subpool is full. This might be added in the future.
    ///
    /// Transactions with the [pool][StaticMemoryPool] are done using a generic
    /// [address][PoolAddr] type. Adding any data to the pool will yield a store address.
    /// Modification and read operations are done using a reference to a store address. Deletion
    /// will consume the store address.
    #[derive(Debug)]
    pub struct StaticMemoryPool {
        pool_cfg: StaticPoolConfig,
        pool: Vec<Vec<u8>>,
        sizes_lists: Vec<Vec<UsedBlockSize>>,
    }

    impl StaticMemoryPool {
        /// Create a new local pool from the [given configuration][StaticPoolConfig]. This function
        /// will sanitize the given configuration as well.
        pub fn new(mut cfg: StaticPoolConfig) -> StaticMemoryPool {
            let subpools_num = cfg.sanitize();
            let mut local_pool = StaticMemoryPool {
                pool_cfg: cfg,
                pool: Vec::with_capacity(subpools_num),
                sizes_lists: Vec::with_capacity(subpools_num),
            };
            for &subpool_cfg in local_pool.pool_cfg.cfg.iter() {
                let next_pool_len = subpool_cfg.num_blocks as usize * subpool_cfg.block_size;
                local_pool.pool.push(vec![0; next_pool_len]);
                let next_sizes_list_len = subpool_cfg.num_blocks as usize;
                local_pool
                    .sizes_lists
                    .push(vec![STORE_FREE; next_sizes_list_len]);
            }
            local_pool
        }

        fn addr_check(&self, addr: &StaticPoolAddr) -> Result<usize, PoolError> {
            self.validate_addr(addr)?;
            let pool_idx = addr.pool_idx as usize;
            let size_list = self.sizes_lists.get(pool_idx).unwrap();
            let curr_size = size_list[addr.packet_idx as usize];
            if curr_size == STORE_FREE {
                return Err(PoolError::DataDoesNotExist(PoolAddr::from(*addr)));
            }
            Ok(curr_size)
        }

        fn validate_addr(&self, addr: &StaticPoolAddr) -> Result<(), PoolError> {
            let pool_idx = addr.pool_idx as usize;
            if pool_idx >= self.pool_cfg.cfg.len() {
                return Err(PoolError::InvalidStoreId(
                    StoreIdError::InvalidSubpool(addr.pool_idx),
                    Some(PoolAddr::from(*addr)),
                ));
            }
            if addr.packet_idx >= self.pool_cfg.cfg[addr.pool_idx as usize].num_blocks {
                return Err(PoolError::InvalidStoreId(
                    StoreIdError::InvalidPacketIdx(addr.packet_idx),
                    Some(PoolAddr::from(*addr)),
                ));
            }
            Ok(())
        }

        fn reserve(&mut self, data_len: usize) -> Result<StaticPoolAddr, PoolError> {
            let mut subpool_idx = self.find_subpool(data_len, 0)?;

            if self.pool_cfg.spill_to_higher_subpools {
                while let Err(PoolError::StoreFull(_)) = self.find_empty(subpool_idx) {
                    if (subpool_idx + 1) as usize == self.sizes_lists.len() {
                        return Err(PoolError::StoreFull(subpool_idx));
                    }
                    subpool_idx += 1;
                }
            }

            let (slot, size_slot_ref) = self.find_empty(subpool_idx)?;
            *size_slot_ref = data_len;
            Ok(StaticPoolAddr {
                pool_idx: subpool_idx,
                packet_idx: slot,
            })
        }

        fn find_subpool(&self, req_size: usize, start_at_subpool: u16) -> Result<u16, PoolError> {
            for (i, &config) in self.pool_cfg.cfg.iter().enumerate() {
                if i < start_at_subpool as usize {
                    continue;
                }
                if config.block_size >= req_size {
                    return Ok(i as u16);
                }
            }
            Err(PoolError::DataTooLarge(req_size))
        }

        fn write(&mut self, addr: &StaticPoolAddr, data: &[u8]) -> Result<(), PoolError> {
            let packet_pos = self.raw_pos(addr).ok_or(PoolError::InternalError(0))?;
            let subpool = self
                .pool
                .get_mut(addr.pool_idx as usize)
                .ok_or(PoolError::InternalError(1))?;
            let pool_slice = &mut subpool[packet_pos..packet_pos + data.len()];
            pool_slice.copy_from_slice(data);
            Ok(())
        }

        fn find_empty(&mut self, subpool: u16) -> Result<(u16, &mut usize), PoolError> {
            if let Some(size_list) = self.sizes_lists.get_mut(subpool as usize) {
                for (i, elem_size) in size_list.iter_mut().enumerate() {
                    if *elem_size == STORE_FREE {
                        return Ok((i as u16, elem_size));
                    }
                }
            } else {
                return Err(PoolError::InvalidStoreId(
                    StoreIdError::InvalidSubpool(subpool),
                    None,
                ));
            }
            Err(PoolError::StoreFull(subpool))
        }

        fn raw_pos(&self, addr: &StaticPoolAddr) -> Option<usize> {
            let cfg = self.pool_cfg.cfg.get(addr.pool_idx as usize)?;
            Some(addr.packet_idx as usize * cfg.block_size)
        }
    }

    impl PoolProvider for StaticMemoryPool {
        fn add(&mut self, data: &[u8]) -> Result<PoolAddr, PoolError> {
            let data_len = data.len();
            if data_len > MAX_BLOCK_SIZE {
                return Err(PoolError::DataTooLarge(data_len));
            }
            let addr = self.reserve(data_len)?;
            self.write(&addr, data)?;
            Ok(addr.into())
        }

        fn free_element<W: FnMut(&mut [u8])>(
            &mut self,
            len: usize,
            mut writer: W,
        ) -> Result<PoolAddr, PoolError> {
            if len > MAX_BLOCK_SIZE {
                return Err(PoolError::DataTooLarge(len));
            }
            let addr = self.reserve(len)?;
            let raw_pos = self.raw_pos(&addr).unwrap();
            let block =
                &mut self.pool.get_mut(addr.pool_idx as usize).unwrap()[raw_pos..raw_pos + len];
            writer(block);
            Ok(addr.into())
        }

        fn modify<U: FnMut(&mut [u8])>(
            &mut self,
            addr: &PoolAddr,
            mut updater: U,
        ) -> Result<(), PoolError> {
            let addr = StaticPoolAddr::from(*addr);
            let curr_size = self.addr_check(&addr)?;
            let raw_pos = self.raw_pos(&addr).unwrap();
            let block = &mut self.pool.get_mut(addr.pool_idx as usize).unwrap()
                [raw_pos..raw_pos + curr_size];
            updater(block);
            Ok(())
        }

        fn read(&self, addr: &PoolAddr, buf: &mut [u8]) -> Result<usize, PoolError> {
            let addr = StaticPoolAddr::from(*addr);
            let curr_size = self.addr_check(&addr)?;
            if buf.len() < curr_size {
                return Err(ByteConversionError::ToSliceTooSmall {
                    found: buf.len(),
                    expected: curr_size,
                }
                .into());
            }
            let raw_pos = self.raw_pos(&addr).unwrap();
            let block =
                &self.pool.get(addr.pool_idx as usize).unwrap()[raw_pos..raw_pos + curr_size];
            //block.copy_from_slice(&src);
            buf[..curr_size].copy_from_slice(block);
            Ok(curr_size)
        }

        fn delete(&mut self, addr: PoolAddr) -> Result<(), PoolError> {
            let addr = StaticPoolAddr::from(addr);
            self.addr_check(&addr)?;
            let block_size = self
                .pool_cfg
                .cfg
                .get(addr.pool_idx as usize)
                .unwrap()
                .block_size;
            let raw_pos = self.raw_pos(&addr).unwrap();
            let block = &mut self.pool.get_mut(addr.pool_idx as usize).unwrap()
                [raw_pos..raw_pos + block_size];
            let size_list = self.sizes_lists.get_mut(addr.pool_idx as usize).unwrap();
            size_list[addr.packet_idx as usize] = STORE_FREE;
            block.fill(0);
            Ok(())
        }

        fn has_element_at(&self, addr: &PoolAddr) -> Result<bool, PoolError> {
            let addr = StaticPoolAddr::from(*addr);
            self.validate_addr(&addr)?;
            let pool_idx = addr.pool_idx as usize;
            let size_list = self.sizes_lists.get(pool_idx).unwrap();
            let curr_size = size_list[addr.packet_idx as usize];
            if curr_size == STORE_FREE {
                return Ok(false);
            }
            Ok(true)
        }

        fn len_of_data(&self, addr: &PoolAddr) -> Result<usize, PoolError> {
            let addr = StaticPoolAddr::from(*addr);
            self.validate_addr(&addr)?;
            let pool_idx = addr.pool_idx as usize;
            let size_list = self.sizes_lists.get(pool_idx).unwrap();
            let size = size_list[addr.packet_idx as usize];
            Ok(match size {
                STORE_FREE => 0,
                _ => size,
            })
        }

        fn clear(&mut self) -> Result<(), PoolError> {
            for size in self.sizes_lists.iter_mut() {
                size.fill(STORE_FREE);
            }
            Ok(())
        }
    }

    impl PoolProviderWithGuards for StaticMemoryPool {
        fn modify_with_guard(&mut self, addr: PoolAddr) -> PoolRwGuard<'_, Self> {
            PoolRwGuard::new(self, addr)
        }

        fn read_with_guard(&mut self, addr: PoolAddr) -> PoolGuard<'_, Self> {
            PoolGuard::new(self, addr)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    fn basic_small_pool() -> StaticMemoryPool {
        // 4 buckets of 4 bytes, 2 of 8 bytes and 1 of 16 bytes
        let pool_cfg =
            StaticPoolConfig::new_from_subpool_cfg_tuples(vec![(4, 4), (2, 8), (1, 16)], false);
        StaticMemoryPool::new(pool_cfg)
    }

    #[test]
    fn test_cfg() {
        // Values where number of buckets is 0 or size is too large should be removed
        let mut pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(0, 0), (1, 0), (2, MAX_BLOCK_SIZE)],
            false,
        );
        pool_cfg.sanitize();
        assert_eq!(*pool_cfg.subpool_cfg(), vec![SubpoolConfig::new(1, 0)]);

        // Entries should be ordered according to bucket size
        pool_cfg =
            StaticPoolConfig::new_from_subpool_cfg_tuples(vec![(16, 6), (32, 3), (8, 12)], false);
        pool_cfg.sanitize();
        assert_eq!(
            *pool_cfg.subpool_cfg(),
            vec![
                SubpoolConfig::new(32, 3),
                SubpoolConfig::new(16, 6),
                SubpoolConfig::new(8, 12)
            ]
        );

        // Unstable sort is used, so order of entries with same block length should not matter
        pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(12, 12), (14, 16), (10, 12)],
            false,
        );
        pool_cfg.sanitize();
        assert!(
            *pool_cfg.subpool_cfg()
                == vec![
                    SubpoolConfig::new(12, 12),
                    SubpoolConfig::new(10, 12),
                    SubpoolConfig::new(14, 16)
                ]
                || *pool_cfg.subpool_cfg()
                    == vec![
                        SubpoolConfig::new(10, 12),
                        SubpoolConfig::new(12, 12),
                        SubpoolConfig::new(14, 16)
                    ]
        );
    }

    fn generic_test_add_and_read<const BUF_SIZE: usize>(pool_provider: &mut impl PoolProvider) {
        let mut test_buf: [u8; BUF_SIZE] = [0; BUF_SIZE];
        for (i, val) in test_buf.iter_mut().enumerate() {
            *val = i as u8;
        }
        let mut other_buf: [u8; BUF_SIZE] = [0; BUF_SIZE];
        let addr = pool_provider.add(&test_buf).expect("adding data failed");
        // Read back data and verify correctness
        let res = pool_provider.read(&addr, &mut other_buf);
        assert!(res.is_ok());
        let read_len = res.unwrap();
        assert_eq!(read_len, BUF_SIZE);
        for (i, &val) in other_buf.iter().enumerate() {
            assert_eq!(val, i as u8);
        }
    }

    fn generic_test_add_smaller_than_full_slot(pool_provider: &mut impl PoolProvider) {
        let test_buf: [u8; 12] = [0; 12];
        let addr = pool_provider.add(&test_buf).expect("adding data failed");
        let res = pool_provider
            .read(&addr, &mut [0; 12])
            .expect("Read back failed");
        assert_eq!(res, 12);
    }

    fn generic_test_delete(pool_provider: &mut impl PoolProvider) {
        // let mut local_pool = basic_small_pool();
        let test_buf: [u8; 16] = [0; 16];
        let addr = pool_provider.add(&test_buf).expect("Adding data failed");
        // Delete the data
        let res = pool_provider.delete(addr);
        assert!(res.is_ok());
        let mut writer = |buf: &mut [u8]| {
            assert_eq!(buf.len(), 12);
        };
        // Verify that the slot is free by trying to get a reference to it
        let res = pool_provider.free_element(12, &mut writer);
        assert!(res.is_ok());
        let addr = res.unwrap();
        assert_eq!(
            addr,
            u64::from(StaticPoolAddr {
                pool_idx: 2,
                packet_idx: 0
            })
        );
    }

    fn generic_test_modify(pool_provider: &mut impl PoolProvider) {
        let mut test_buf: [u8; 16] = [0; 16];
        for (i, val) in test_buf.iter_mut().enumerate() {
            *val = i as u8;
        }
        let addr = pool_provider.add(&test_buf).expect("Adding data failed");

        {
            // Verify that the slot is free by trying to get a reference to it
            pool_provider
                .modify(&addr, &mut |buf: &mut [u8]| {
                    buf[0] = 0;
                    buf[1] = 0x42;
                })
                .expect("Modifying data failed");
        }

        pool_provider
            .read(&addr, &mut test_buf)
            .expect("Reading back data failed");
        assert_eq!(test_buf[0], 0);
        assert_eq!(test_buf[1], 0x42);
        assert_eq!(test_buf[2], 2);
        assert_eq!(test_buf[3], 3);
    }

    fn generic_test_consecutive_reservation(pool_provider: &mut impl PoolProvider) {
        // Reserve two smaller blocks consecutively and verify that the third reservation fails
        let res = pool_provider.free_element(8, |_| {});
        assert!(res.is_ok());
        let addr0 = res.unwrap();
        let res = pool_provider.free_element(8, |_| {});
        assert!(res.is_ok());
        let addr1 = res.unwrap();
        let res = pool_provider.free_element(8, |_| {});
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert_eq!(err, PoolError::StoreFull(1));

        // Verify that the two deletions are successful
        assert!(pool_provider.delete(addr0).is_ok());
        assert!(pool_provider.delete(addr1).is_ok());
    }

    fn generic_test_read_does_not_exist(pool_provider: &mut impl PoolProvider) {
        // Try to access data which does not exist
        let res = pool_provider.read(
            &StaticPoolAddr {
                packet_idx: 0,
                pool_idx: 0,
            }
            .into(),
            &mut [],
        );
        assert!(res.is_err());
        assert!(matches!(
            res.unwrap_err(),
            PoolError::DataDoesNotExist { .. }
        ));
    }

    fn generic_test_store_full(pool_provider: &mut impl PoolProvider) {
        let test_buf: [u8; 16] = [0; 16];
        assert!(pool_provider.add(&test_buf).is_ok());
        // The subpool is now full and the call should fail accordingly
        let res = pool_provider.add(&test_buf);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(matches!(err, PoolError::StoreFull { .. }));
        if let PoolError::StoreFull(subpool) = err {
            assert_eq!(subpool, 2);
        }
    }

    fn generic_test_invalid_pool_idx(pool_provider: &mut impl PoolProvider) {
        let addr = StaticPoolAddr {
            pool_idx: 3,
            packet_idx: 0,
        }
        .into();
        let res = pool_provider.read(&addr, &mut []);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(matches!(
            err,
            PoolError::InvalidStoreId(StoreIdError::InvalidSubpool(3), Some(_))
        ));
    }

    fn generic_test_invalid_packet_idx(pool_provider: &mut impl PoolProvider) {
        let addr = StaticPoolAddr {
            pool_idx: 2,
            packet_idx: 1,
        };
        assert_eq!(addr.raw(), 0x00020001);
        let res = pool_provider.read(&addr.into(), &mut []);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert!(matches!(
            err,
            PoolError::InvalidStoreId(StoreIdError::InvalidPacketIdx(1), Some(_))
        ));
    }

    fn generic_test_add_too_large(pool_provider: &mut impl PoolProvider) {
        let data_too_large = [0; 20];
        let res = pool_provider.add(&data_too_large);
        assert!(res.is_err());
        let err = res.unwrap_err();
        assert_eq!(err, PoolError::DataTooLarge(20));
    }

    fn generic_test_data_too_large_1(pool_provider: &mut impl PoolProvider) {
        let res = pool_provider.free_element(MAX_BLOCK_SIZE + 1, |_| {});
        assert!(res.is_err());
        assert_eq!(
            res.unwrap_err(),
            PoolError::DataTooLarge(MAX_BLOCK_SIZE + 1)
        );
    }

    fn generic_test_free_element_too_large(pool_provider: &mut impl PoolProvider) {
        // Try to request a slot which is too large
        let res = pool_provider.free_element(20, |_| {});
        assert!(res.is_err());
        assert_eq!(res.unwrap_err(), PoolError::DataTooLarge(20));
    }

    fn generic_test_pool_guard_deletion_man_creation(pool_provider: &mut impl PoolProvider) {
        let test_buf: [u8; 16] = [0; 16];
        let addr = pool_provider.add(&test_buf).expect("Adding data failed");
        let read_guard = PoolGuard::new(pool_provider, addr);
        drop(read_guard);
        assert!(
            !pool_provider
                .has_element_at(&addr)
                .expect("Invalid address")
        );
    }

    fn generic_test_pool_guard_deletion(pool_provider: &mut impl PoolProviderWithGuards) {
        let test_buf: [u8; 16] = [0; 16];
        let addr = pool_provider.add(&test_buf).expect("Adding data failed");
        let read_guard = pool_provider.read_with_guard(addr);
        drop(read_guard);
        assert!(
            !pool_provider
                .has_element_at(&addr)
                .expect("Invalid address")
        );
    }

    fn generic_test_pool_guard_with_release(pool_provider: &mut impl PoolProviderWithGuards) {
        let test_buf: [u8; 16] = [0; 16];
        let addr = pool_provider.add(&test_buf).expect("Adding data failed");
        let mut read_guard = PoolGuard::new(pool_provider, addr);
        read_guard.release();
        drop(read_guard);
        assert!(
            pool_provider
                .has_element_at(&addr)
                .expect("Invalid address")
        );
    }

    fn generic_test_pool_modify_guard_man_creation(
        pool_provider: &mut impl PoolProviderWithGuards,
    ) {
        let test_buf: [u8; 16] = [0; 16];
        let addr = pool_provider.add(&test_buf).expect("Adding data failed");
        let mut rw_guard = PoolRwGuard::new(pool_provider, addr);
        rw_guard.update(&mut |_| {}).expect("modify failed");
        drop(rw_guard);
        assert!(
            !pool_provider
                .has_element_at(&addr)
                .expect("Invalid address")
        );
    }

    fn generic_test_pool_modify_guard(pool_provider: &mut impl PoolProviderWithGuards) {
        let test_buf: [u8; 16] = [0; 16];
        let addr = pool_provider.add(&test_buf).expect("Adding data failed");
        let mut rw_guard = pool_provider.modify_with_guard(addr);
        rw_guard.update(&mut |_| {}).expect("modify failed");
        drop(rw_guard);
        assert!(
            !pool_provider
                .has_element_at(&addr)
                .expect("Invalid address")
        );
    }

    fn generic_modify_pool_index_above_0(pool_provider: &mut impl PoolProvider) {
        let test_buf_0: [u8; 4] = [1; 4];
        let test_buf_1: [u8; 4] = [2; 4];
        let test_buf_2: [u8; 4] = [3; 4];
        let test_buf_3: [u8; 4] = [4; 4];
        let addr0 = pool_provider.add(&test_buf_0).expect("Adding data failed");
        let addr1 = pool_provider.add(&test_buf_1).expect("Adding data failed");
        let addr2 = pool_provider.add(&test_buf_2).expect("Adding data failed");
        let addr3 = pool_provider.add(&test_buf_3).expect("Adding data failed");
        pool_provider
            .modify(&addr0, |buf| {
                assert_eq!(buf, test_buf_0);
            })
            .expect("Modifying data failed");
        pool_provider
            .modify(&addr1, |buf| {
                assert_eq!(buf, test_buf_1);
            })
            .expect("Modifying data failed");
        pool_provider
            .modify(&addr2, |buf| {
                assert_eq!(buf, test_buf_2);
            })
            .expect("Modifying data failed");
        pool_provider
            .modify(&addr3, |buf| {
                assert_eq!(buf, test_buf_3);
            })
            .expect("Modifying data failed");
    }

    fn generic_test_spills_to_higher_subpools(pool_provider: &mut impl PoolProvider) {
        pool_provider.free_element(8, |_| {}).unwrap();
        pool_provider.free_element(8, |_| {}).unwrap();
        let mut in_larger_subpool_now = pool_provider.free_element(8, |_| {});
        assert!(in_larger_subpool_now.is_ok());
        let generic_addr = in_larger_subpool_now.unwrap();
        let pool_addr = StaticPoolAddr::from(generic_addr);
        assert_eq!(pool_addr.pool_idx, 1);
        assert_eq!(pool_addr.packet_idx, 0);
        assert!(pool_provider.has_element_at(&generic_addr).unwrap());
        in_larger_subpool_now = pool_provider.free_element(8, |_| {});
        assert!(in_larger_subpool_now.is_ok());
        let generic_addr = in_larger_subpool_now.unwrap();
        let pool_addr = StaticPoolAddr::from(generic_addr);
        assert_eq!(pool_addr.pool_idx, 1);
        assert_eq!(pool_addr.packet_idx, 1);
        assert!(pool_provider.has_element_at(&generic_addr).unwrap());
    }

    fn generic_test_spillage_fails_as_well(pool_provider: &mut impl PoolProvider) {
        pool_provider.free_element(8, |_| {}).unwrap();
        pool_provider.free_element(8, |_| {}).unwrap();
        let should_fail = pool_provider.free_element(8, |_| {});
        assert!(should_fail.is_err());
        if let Err(err) = should_fail {
            assert_eq!(err, PoolError::StoreFull(1));
        } else {
            panic!("unexpected store address");
        }
    }

    fn generic_test_spillage_works_across_multiple_subpools(pool_provider: &mut impl PoolProvider) {
        pool_provider.free_element(8, |_| {}).unwrap();
        pool_provider.free_element(12, |_| {}).unwrap();
        let in_larger_subpool_now = pool_provider.free_element(8, |_| {});
        assert!(in_larger_subpool_now.is_ok());
        let generic_addr = in_larger_subpool_now.unwrap();
        let pool_addr = StaticPoolAddr::from(generic_addr);
        assert_eq!(pool_addr.pool_idx, 2);
        assert_eq!(pool_addr.packet_idx, 0);
        assert!(pool_provider.has_element_at(&generic_addr).unwrap());
    }

    fn generic_test_spillage_fails_across_multiple_subpools(pool_provider: &mut impl PoolProvider) {
        pool_provider.free_element(8, |_| {}).unwrap();
        pool_provider.free_element(12, |_| {}).unwrap();
        pool_provider.free_element(16, |_| {}).unwrap();
        let should_fail = pool_provider.free_element(8, |_| {});
        assert!(should_fail.is_err());
        if let Err(err) = should_fail {
            assert_eq!(err, PoolError::StoreFull(2));
        } else {
            panic!("unexpected store address");
        }
    }

    #[test]
    fn test_add_and_read() {
        let mut local_pool = basic_small_pool();
        generic_test_add_and_read::<16>(&mut local_pool);
    }

    #[test]
    fn test_add_smaller_than_full_slot() {
        let mut local_pool = basic_small_pool();
        generic_test_add_smaller_than_full_slot(&mut local_pool);
    }

    #[test]
    fn test_delete() {
        let mut local_pool = basic_small_pool();
        generic_test_delete(&mut local_pool);
    }

    #[test]
    fn test_modify() {
        let mut local_pool = basic_small_pool();
        generic_test_modify(&mut local_pool);
    }

    #[test]
    fn test_consecutive_reservation() {
        let mut local_pool = basic_small_pool();
        generic_test_consecutive_reservation(&mut local_pool);
    }

    #[test]
    fn test_read_does_not_exist() {
        let mut local_pool = basic_small_pool();
        generic_test_read_does_not_exist(&mut local_pool);
    }

    #[test]
    fn test_store_full() {
        let mut local_pool = basic_small_pool();
        generic_test_store_full(&mut local_pool);
    }

    #[test]
    fn test_invalid_pool_idx() {
        let mut local_pool = basic_small_pool();
        generic_test_invalid_pool_idx(&mut local_pool);
    }

    #[test]
    fn test_invalid_packet_idx() {
        let mut local_pool = basic_small_pool();
        generic_test_invalid_packet_idx(&mut local_pool);
    }

    #[test]
    fn test_add_too_large() {
        let mut local_pool = basic_small_pool();
        generic_test_add_too_large(&mut local_pool);
    }

    #[test]
    fn test_data_too_large_1() {
        let mut local_pool = basic_small_pool();
        generic_test_data_too_large_1(&mut local_pool);
    }

    #[test]
    fn test_free_element_too_large() {
        let mut local_pool = basic_small_pool();
        generic_test_free_element_too_large(&mut local_pool);
    }

    #[test]
    fn test_pool_guard_deletion_man_creation() {
        let mut local_pool = basic_small_pool();
        generic_test_pool_guard_deletion_man_creation(&mut local_pool);
    }

    #[test]
    fn test_pool_guard_deletion() {
        let mut local_pool = basic_small_pool();
        generic_test_pool_guard_deletion(&mut local_pool);
    }

    #[test]
    fn test_pool_guard_with_release() {
        let mut local_pool = basic_small_pool();
        generic_test_pool_guard_with_release(&mut local_pool);
    }

    #[test]
    fn test_pool_modify_guard_man_creation() {
        let mut local_pool = basic_small_pool();
        generic_test_pool_modify_guard_man_creation(&mut local_pool);
    }

    #[test]
    fn test_pool_modify_guard() {
        let mut local_pool = basic_small_pool();
        generic_test_pool_modify_guard(&mut local_pool);
    }

    #[test]
    fn modify_pool_index_above_0() {
        let mut local_pool = basic_small_pool();
        generic_modify_pool_index_above_0(&mut local_pool);
    }

    #[test]
    fn test_spills_to_higher_subpools() {
        let subpool_config_vec = vec![SubpoolConfig::new(2, 8), SubpoolConfig::new(2, 16)];
        let pool_cfg = StaticPoolConfig::new(subpool_config_vec, true);
        let mut local_pool = StaticMemoryPool::new(pool_cfg);
        generic_test_spills_to_higher_subpools(&mut local_pool);
    }

    #[test]
    fn test_spillage_fails_as_well() {
        let pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(vec![(1, 8), (1, 16)], true);
        let mut local_pool = StaticMemoryPool::new(pool_cfg);
        generic_test_spillage_fails_as_well(&mut local_pool);
    }

    #[test]
    fn test_spillage_works_across_multiple_subpools() {
        let pool_cfg =
            StaticPoolConfig::new_from_subpool_cfg_tuples(vec![(1, 8), (1, 12), (1, 16)], true);
        let mut local_pool = StaticMemoryPool::new(pool_cfg);
        generic_test_spillage_works_across_multiple_subpools(&mut local_pool);
    }

    #[test]
    fn test_spillage_fails_across_multiple_subpools() {
        let pool_cfg =
            StaticPoolConfig::new_from_subpool_cfg_tuples(vec![(1, 8), (1, 12), (1, 16)], true);
        let mut local_pool = StaticMemoryPool::new(pool_cfg);
        generic_test_spillage_fails_across_multiple_subpools(&mut local_pool);
    }

    #[cfg(feature = "heapless")]
    mod heapless_tests {
        use super::*;
        use crate::static_subpool;

        const SUBPOOL_1_BLOCK_SIZE: usize = 4;
        const SUBPOOL_1_NUM_ELEMENTS: u16 = 4;

        const SUBPOOL_2_NUM_ELEMENTS: u16 = 2;
        const SUBPOOL_2_BLOCK_SIZE: usize = 8;

        const SUBPOOL_3_NUM_ELEMENTS: u16 = 1;
        const SUBPOOL_3_BLOCK_SIZE: usize = 16;

        const SUBPOOL_4_NUM_ELEMENTS: u16 = 2;
        const SUBPOOL_4_BLOCK_SIZE: usize = 16;

        const SUBPOOL_5_NUM_ELEMENTS: u16 = 1;
        const SUBPOOL_5_BLOCK_SIZE: usize = 8;

        const SUBPOOL_6_NUM_ELEMENTS: u16 = 1;
        const SUBPOOL_6_BLOCK_SIZE: usize = 12;

        macro_rules! make_heapless_pool {
            ($prefix:ident) => {{
                paste::paste! {
                    static [<$prefix _SUBPOOL_1>]: static_cell::ConstStaticCell<
                        [u8; SUBPOOL_1_NUM_ELEMENTS as usize * SUBPOOL_1_BLOCK_SIZE],
                    > = static_cell::ConstStaticCell::new(
                        [0; SUBPOOL_1_NUM_ELEMENTS as usize * SUBPOOL_1_BLOCK_SIZE],
                    );

                    static [<$prefix _SUBPOOL_1_SIZES>]: std::sync::Mutex<
                        std::cell::UnsafeCell<[usize; SUBPOOL_1_NUM_ELEMENTS as usize]>
                    > = std::sync::Mutex::new(
                        std::cell::UnsafeCell::new([STORE_FREE; SUBPOOL_1_NUM_ELEMENTS as usize])
                    );

                    static [<$prefix _SUBPOOL_2>]: static_cell::ConstStaticCell<
                        [u8; SUBPOOL_2_NUM_ELEMENTS as usize * SUBPOOL_2_BLOCK_SIZE],
                    > = static_cell::ConstStaticCell::new(
                        [0; SUBPOOL_2_NUM_ELEMENTS as usize * SUBPOOL_2_BLOCK_SIZE],
                    );

                    static [<$prefix _SUBPOOL_2_SIZES>]: static_cell::ConstStaticCell<
                        [usize; SUBPOOL_2_NUM_ELEMENTS as usize],
                    > = static_cell::ConstStaticCell::new(
                        [STORE_FREE; SUBPOOL_2_NUM_ELEMENTS as usize]
                    );

                    static [<$prefix _SUBPOOL_3>]: static_cell::ConstStaticCell<
                        [u8; SUBPOOL_3_NUM_ELEMENTS as usize * SUBPOOL_3_BLOCK_SIZE],
                    > = static_cell::ConstStaticCell::new(
                        [0; SUBPOOL_3_NUM_ELEMENTS as usize * SUBPOOL_3_BLOCK_SIZE],
                    );

                    static [<$prefix _SUBPOOL_3_SIZES>]: static_cell::ConstStaticCell<
                        [usize; SUBPOOL_3_NUM_ELEMENTS as usize],
                    > = static_cell::ConstStaticCell::new(
                        [STORE_FREE; SUBPOOL_3_NUM_ELEMENTS as usize]
                    );

                    let mut heapless_pool: StaticHeaplessMemoryPool<3> =
                        StaticHeaplessMemoryPool::new(false);

                    heapless_pool
                        .grow(
                            [<$prefix _SUBPOOL_1>].take(),
                            unsafe { &mut *[<$prefix _SUBPOOL_1_SIZES>].lock().unwrap().get() },
                            SUBPOOL_1_NUM_ELEMENTS,
                            true
                        )
                        .unwrap();

                    heapless_pool
                        .grow(
                            [<$prefix _SUBPOOL_2>].take(),
                            [<$prefix _SUBPOOL_2_SIZES>].take(),
                            SUBPOOL_2_NUM_ELEMENTS,
                            true
                        )
                        .unwrap();

                    heapless_pool
                        .grow(
                            [<$prefix _SUBPOOL_3>].take(),
                            [<$prefix _SUBPOOL_3_SIZES>].take(),
                            SUBPOOL_3_NUM_ELEMENTS,
                            true
                        )
                        .unwrap();

                    heapless_pool
                }
            }};
        }

        #[test]
        fn test_heapless_add_and_read() {
            let mut pool_provider = make_heapless_pool!(T0);
            generic_test_add_and_read::<16>(&mut pool_provider);
        }

        #[test]
        fn test_add_smaller_than_full_slot() {
            let mut pool_provider = make_heapless_pool!(T1);
            generic_test_add_smaller_than_full_slot(&mut pool_provider);
        }

        #[test]
        fn test_delete() {
            let mut pool_provider = make_heapless_pool!(T2);
            generic_test_delete(&mut pool_provider);
        }

        #[test]
        fn test_modify() {
            let mut pool_provider = make_heapless_pool!(T3);
            generic_test_modify(&mut pool_provider);
        }

        #[test]
        fn test_consecutive_reservation() {
            let mut pool_provider = make_heapless_pool!(T4);
            generic_test_consecutive_reservation(&mut pool_provider);
        }

        #[test]
        fn test_read_does_not_exist() {
            let mut pool_provider = make_heapless_pool!(T5);
            generic_test_read_does_not_exist(&mut pool_provider);
        }

        #[test]
        fn test_store_full() {
            let mut pool_provider = make_heapless_pool!(T6);
            generic_test_store_full(&mut pool_provider);
        }

        #[test]
        fn test_invalid_pool_idx() {
            let mut pool_provider = make_heapless_pool!(T7);
            generic_test_invalid_pool_idx(&mut pool_provider);
        }

        #[test]
        fn test_invalid_packet_idx() {
            let mut pool_provider = make_heapless_pool!(T8);
            generic_test_invalid_packet_idx(&mut pool_provider);
        }

        #[test]
        fn test_add_too_large() {
            let mut pool_provider = make_heapless_pool!(T9);
            generic_test_add_too_large(&mut pool_provider);
        }

        #[test]
        fn test_data_too_large_1() {
            let mut pool_provider = make_heapless_pool!(T10);
            generic_test_data_too_large_1(&mut pool_provider);
        }

        #[test]
        fn test_free_element_too_large() {
            let mut pool_provider = make_heapless_pool!(T11);
            generic_test_free_element_too_large(&mut pool_provider);
        }

        #[test]
        fn test_pool_guard_deletion_man_creation() {
            let mut pool_provider = make_heapless_pool!(T12);
            generic_test_pool_guard_deletion_man_creation(&mut pool_provider);
        }

        #[test]
        fn test_pool_guard_deletion() {
            let mut pool_provider = make_heapless_pool!(T13);
            generic_test_pool_guard_deletion(&mut pool_provider);
        }

        #[test]
        fn test_pool_guard_with_release() {
            let mut pool_provider = make_heapless_pool!(T14);
            generic_test_pool_guard_with_release(&mut pool_provider);
        }

        #[test]
        fn test_pool_modify_guard_man_creation() {
            let mut pool_provider = make_heapless_pool!(T15);
            generic_test_pool_modify_guard_man_creation(&mut pool_provider);
        }

        #[test]
        fn test_pool_modify_guard() {
            let mut pool_provider = make_heapless_pool!(T16);
            generic_test_pool_modify_guard(&mut pool_provider);
        }

        #[test]
        fn modify_pool_index_above_0() {
            let mut pool_provider = make_heapless_pool!(T17);
            generic_modify_pool_index_above_0(&mut pool_provider);
        }

        #[test]
        fn test_spills_to_higher_subpools() {
            static_subpool!(
                SUBPOOL_2_T18,
                SUBPOOL_2_SIZES_T18,
                SUBPOOL_2_NUM_ELEMENTS as usize,
                SUBPOOL_2_BLOCK_SIZE
            );
            static_subpool!(
                SUBPOOL_4_T18,
                SUBPOOL_4_SIZES_T18,
                SUBPOOL_4_NUM_ELEMENTS as usize,
                SUBPOOL_4_BLOCK_SIZE
            );
            let mut heapless_pool: StaticHeaplessMemoryPool<2> =
                StaticHeaplessMemoryPool::new(true);
            assert!(
                heapless_pool
                    .grow(
                        SUBPOOL_2_T18.take(),
                        SUBPOOL_2_SIZES_T18.take(),
                        SUBPOOL_2_NUM_ELEMENTS,
                        true
                    )
                    .is_ok()
            );
            assert!(
                heapless_pool
                    .grow(
                        SUBPOOL_4_T18.take(),
                        SUBPOOL_4_SIZES_T18.take(),
                        SUBPOOL_4_NUM_ELEMENTS,
                        true
                    )
                    .is_ok()
            );
            generic_test_spills_to_higher_subpools(&mut heapless_pool);
        }

        #[test]
        fn test_spillage_fails_as_well() {
            let mut heapless_pool: StaticHeaplessMemoryPool<2> =
                StaticHeaplessMemoryPool::new(true);
            static_subpool!(
                SUBPOOL_5,
                SUBPOOL_5_SIZES,
                SUBPOOL_5_NUM_ELEMENTS as usize,
                SUBPOOL_5_BLOCK_SIZE
            );
            static_subpool!(
                SUBPOOL_3,
                SUBPOOL_3_SIZES,
                SUBPOOL_3_NUM_ELEMENTS as usize,
                SUBPOOL_3_BLOCK_SIZE
            );
            assert!(
                heapless_pool
                    .grow(
                        SUBPOOL_5.take(),
                        SUBPOOL_5_SIZES.take(),
                        SUBPOOL_5_NUM_ELEMENTS,
                        true
                    )
                    .is_ok()
            );
            assert!(
                heapless_pool
                    .grow(
                        SUBPOOL_3.take(),
                        SUBPOOL_3_SIZES.take(),
                        SUBPOOL_3_NUM_ELEMENTS,
                        true
                    )
                    .is_ok()
            );
            generic_test_spillage_fails_as_well(&mut heapless_pool);
        }

        #[test]
        fn test_spillage_works_across_multiple_subpools() {
            let mut heapless_pool: StaticHeaplessMemoryPool<3> =
                StaticHeaplessMemoryPool::new(true);
            static_subpool!(
                SUBPOOL_3,
                SUBPOOL_3_SIZES,
                SUBPOOL_3_NUM_ELEMENTS as usize,
                SUBPOOL_3_BLOCK_SIZE
            );
            static_subpool!(
                SUBPOOL_5,
                SUBPOOL_5_SIZES,
                SUBPOOL_5_NUM_ELEMENTS as usize,
                SUBPOOL_5_BLOCK_SIZE
            );
            static_subpool!(
                SUBPOOL_6,
                SUBPOOL_6_SIZES,
                SUBPOOL_6_NUM_ELEMENTS as usize,
                SUBPOOL_6_BLOCK_SIZE
            );
            assert!(
                heapless_pool
                    .grow(
                        SUBPOOL_5.take(),
                        SUBPOOL_5_SIZES.take(),
                        SUBPOOL_5_NUM_ELEMENTS,
                        true
                    )
                    .is_ok()
            );
            assert!(
                heapless_pool
                    .grow(
                        SUBPOOL_6.take(),
                        SUBPOOL_6_SIZES.take(),
                        SUBPOOL_6_NUM_ELEMENTS,
                        true
                    )
                    .is_ok()
            );
            assert!(
                heapless_pool
                    .grow(
                        SUBPOOL_3.take(),
                        SUBPOOL_3_SIZES.take(),
                        SUBPOOL_3_NUM_ELEMENTS,
                        true
                    )
                    .is_ok()
            );
            generic_test_spillage_works_across_multiple_subpools(&mut heapless_pool);
        }

        #[test]
        fn test_spillage_fails_across_multiple_subpools() {
            static_subpool!(
                SUBPOOL_3,
                SUBPOOL_3_SIZES,
                SUBPOOL_3_NUM_ELEMENTS as usize,
                SUBPOOL_3_BLOCK_SIZE
            );
            static_subpool!(
                SUBPOOL_5,
                SUBPOOL_5_SIZES,
                SUBPOOL_5_NUM_ELEMENTS as usize,
                SUBPOOL_5_BLOCK_SIZE
            );
            static_subpool!(
                SUBPOOL_6,
                SUBPOOL_6_SIZES,
                SUBPOOL_6_NUM_ELEMENTS as usize,
                SUBPOOL_6_BLOCK_SIZE
            );
            let mut heapless_pool: StaticHeaplessMemoryPool<3> =
                StaticHeaplessMemoryPool::new(true);
            assert!(
                heapless_pool
                    .grow(
                        SUBPOOL_5.take(),
                        SUBPOOL_5_SIZES.take(),
                        SUBPOOL_5_NUM_ELEMENTS,
                        true
                    )
                    .is_ok()
            );
            assert!(
                heapless_pool
                    .grow(
                        SUBPOOL_6.take(),
                        SUBPOOL_6_SIZES.take(),
                        SUBPOOL_6_NUM_ELEMENTS,
                        true
                    )
                    .is_ok()
            );
            assert!(
                heapless_pool
                    .grow(
                        SUBPOOL_3.take(),
                        SUBPOOL_3_SIZES.take(),
                        SUBPOOL_3_NUM_ELEMENTS,
                        true
                    )
                    .is_ok()
            );
            generic_test_spillage_fails_across_multiple_subpools(&mut heapless_pool);
        }
    }
}
