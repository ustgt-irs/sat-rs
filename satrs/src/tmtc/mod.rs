//! Telemetry and Telecommanding (TMTC) module. Contains packet routing components with special
//! support for CCSDS and ECSS packets.
//!
//! It is recommended to read the [sat-rs book chapter](https://robamu.github.io/sat-rs/book/communication.html)
//! about communication first. The TMTC abstractions provided by this framework are based on the
//! assumption that all telemetry is sent to a special handler object called the TM sink while
//! all received telecommands are sent to a special handler object called TC source. Using
//! a design like this makes it simpler to add new TC packet sources or new telemetry generators:
//! They only need to send the received and generated data to these objects.
use crate::queue::GenericSendError;
use crate::{
    ComponentId,
    pool::{PoolAddr, PoolError},
};
#[cfg(feature = "std")]
pub use alloc_mod::*;
use core::fmt::Debug;
#[cfg(feature = "alloc")]
use downcast_rs::{Downcast, impl_downcast};
use spacepackets::{
    SpHeader,
    ecss::{
        tc::PusTcReader,
        tm::{PusTmCreator, PusTmReader},
    },
};
#[cfg(feature = "std")]
use std::sync::mpsc;
#[cfg(feature = "std")]
pub use std_mod::*;

pub mod tm_helper;

/// Simple type modelling packet stored inside a pool structure. This structure is intended to
/// be used when sending a packet via a message queue, so it also contains the sender ID.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PacketInPool {
    pub sender_id: ComponentId,
    pub store_addr: PoolAddr,
}

impl PacketInPool {
    pub fn new(sender_id: ComponentId, store_addr: PoolAddr) -> Self {
        Self {
            sender_id,
            store_addr,
        }
    }
}

/// Generic trait for object which can send any packets in form of a raw bytestream, with
/// no assumptions about the received protocol.
pub trait PacketHandler: Send {
    type Error;
    fn handle_packet(&self, sender_id: ComponentId, packet: &[u8]) -> Result<(), Self::Error>;
}

/// Extension trait of [PacketHandler] which allows downcasting by implementing [Downcast].
#[cfg(feature = "alloc")]
pub trait PacketSenderRawExt: PacketHandler + Downcast {
    // Remove this once trait upcasting coercion has been implemented.
    // Tracking issue: https://github.com/rust-lang/rust/issues/65991
    fn upcast(&self) -> &dyn PacketHandler<Error = Self::Error>;
    // Remove this once trait upcasting coercion has been implemented.
    // Tracking issue: https://github.com/rust-lang/rust/issues/65991
    fn upcast_mut(&mut self) -> &mut dyn PacketHandler<Error = Self::Error>;
}

/// Blanket implementation to automatically implement [PacketSenderRawExt] when the [alloc]
/// feature is enabled.
#[cfg(feature = "alloc")]
impl<T> PacketSenderRawExt for T
where
    T: PacketHandler + Send + 'static,
{
    // Remove this once trait upcasting coercion has been implemented.
    // Tracking issue: https://github.com/rust-lang/rust/issues/65991
    fn upcast(&self) -> &dyn PacketHandler<Error = Self::Error> {
        self
    }
    // Remove this once trait upcasting coercion has been implemented.
    // Tracking issue: https://github.com/rust-lang/rust/issues/65991
    fn upcast_mut(&mut self) -> &mut dyn PacketHandler<Error = Self::Error> {
        self
    }
}

#[cfg(feature = "alloc")]
impl_downcast!(PacketSenderRawExt assoc Error);

/// Generic trait for object which can send CCSDS space packets, for example ECSS PUS packets
/// or CCSDS File Delivery Protocol (CFDP) packets wrapped in space packets.
pub trait PacketSenderCcsds: Send {
    type Error;
    fn send_ccsds(
        &self,
        sender_id: ComponentId,
        header: &SpHeader,
        tc_raw: &[u8],
    ) -> Result<(), Self::Error>;
}

#[cfg(feature = "std")]
impl PacketSenderCcsds for mpsc::Sender<PacketAsVec> {
    type Error = GenericSendError;

    fn send_ccsds(
        &self,
        sender_id: ComponentId,
        _: &SpHeader,
        tc_raw: &[u8],
    ) -> Result<(), Self::Error> {
        self.send(PacketAsVec::new(sender_id, tc_raw.to_vec()))
            .map_err(|_| GenericSendError::RxDisconnected)
    }
}

#[cfg(feature = "std")]
impl PacketSenderCcsds for mpsc::SyncSender<PacketAsVec> {
    type Error = GenericSendError;

    fn send_ccsds(
        &self,
        sender_id: ComponentId,
        _: &SpHeader,
        packet_raw: &[u8],
    ) -> Result<(), Self::Error> {
        self.try_send(PacketAsVec::new(sender_id, packet_raw.to_vec()))
            .map_err(|e| match e {
                mpsc::TrySendError::Full(_) => GenericSendError::QueueFull(None),
                mpsc::TrySendError::Disconnected(_) => GenericSendError::RxDisconnected,
            })
    }
}

/// Generic trait for a packet receiver, with no restrictions on the type of packet.
/// Implementors write the telemetry into the provided buffer and return the size of the telemetry.
pub trait PacketSource: Send {
    type Error;
    fn retrieve_packet(&mut self, buffer: &mut [u8]) -> Result<usize, Self::Error>;
}

/// Extension trait of [PacketSource] which allows downcasting by implementing [Downcast].
#[cfg(feature = "alloc")]
pub trait PacketSourceExt: PacketSource + Downcast {
    // Remove this once trait upcasting coercion has been implemented.
    // Tracking issue: https://github.com/rust-lang/rust/issues/65991
    fn upcast(&self) -> &dyn PacketSource<Error = Self::Error>;
    // Remove this once trait upcasting coercion has been implemented.
    // Tracking issue: https://github.com/rust-lang/rust/issues/65991
    fn upcast_mut(&mut self) -> &mut dyn PacketSource<Error = Self::Error>;
}

/// Blanket implementation to automatically implement [PacketSourceExt] when the [alloc] feature
/// is enabled.
#[cfg(feature = "alloc")]
impl<T> PacketSourceExt for T
where
    T: PacketSource + 'static,
{
    // Remove this once trait upcasting coercion has been implemented.
    // Tracking issue: https://github.com/rust-lang/rust/issues/65991
    fn upcast(&self) -> &dyn PacketSource<Error = Self::Error> {
        self
    }
    // Remove this once trait upcasting coercion has been implemented.
    // Tracking issue: https://github.com/rust-lang/rust/issues/65991
    fn upcast_mut(&mut self) -> &mut dyn PacketSource<Error = Self::Error> {
        self
    }
}

/// Helper trait for any generic (static) store which allows storing raw or CCSDS packets.
pub trait CcsdsPacketPool: Debug {
    fn add_ccsds_tc(&mut self, _: &SpHeader, tc_raw: &[u8]) -> Result<PoolAddr, PoolError> {
        self.add_raw_tc(tc_raw)
    }

    fn add_raw_tc(&mut self, tc_raw: &[u8]) -> Result<PoolAddr, PoolError>;
}

/// Helper trait for any generic (static) store which allows storing ECSS PUS Telecommand packets.
pub trait PusTcPool {
    fn add_pus_tc(&mut self, pus_tc: &PusTcReader) -> Result<PoolAddr, PoolError>;
}

/// Helper trait for any generic (static) store which allows storing ECSS PUS Telemetry packets.
pub trait PusTmPool {
    fn add_pus_tm_from_reader(&mut self, pus_tm: &PusTmReader) -> Result<PoolAddr, PoolError>;
    fn add_pus_tm_from_creator(&mut self, pus_tm: &PusTmCreator) -> Result<PoolAddr, PoolError>;
}

/// Generic trait for any sender component able to send packets stored inside a pool structure.
pub trait PacketInPoolSender: Debug + Send {
    fn send_packet(
        &self,
        sender_id: ComponentId,
        store_addr: PoolAddr,
    ) -> Result<(), GenericSendError>;
}

#[cfg(feature = "alloc")]
pub mod alloc_mod {
    use alloc::vec::Vec;

    use super::*;

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
}
#[cfg(feature = "std")]
pub mod std_mod {

    use core::cell::RefCell;

    #[cfg(feature = "crossbeam")]
    use crossbeam_channel as cb;
    use spacepackets::ecss::WritablePusPacket;
    use thiserror::Error;

    use crate::legacy::pus::{EcssTmSender, EcssTmtcError, PacketSenderPusTc};
    use crate::pool::PoolProvider;

    use super::*;

    /// Newtype wrapper around the [SharedStaticMemoryPool] to enable extension helper traits on
    /// top of the regular shared memory pool API.
    #[derive(Debug, Clone)]
    pub struct SharedPacketPool(pub SharedStaticMemoryPool);

    impl SharedPacketPool {
        pub fn new(pool: &SharedStaticMemoryPool) -> Self {
            Self(pool.clone())
        }
    }

    impl PusTcPool for SharedPacketPool {
        fn add_pus_tc(&mut self, pus_tc: &PusTcReader) -> Result<PoolAddr, PoolError> {
            let mut pg = self.0.write().map_err(|_| PoolError::LockError)?;
            let addr = pg.free_element(pus_tc.len_packed(), |buf| {
                buf[0..pus_tc.len_packed()].copy_from_slice(pus_tc.raw_data());
            })?;
            Ok(addr)
        }
    }

    impl PusTmPool for SharedPacketPool {
        fn add_pus_tm_from_reader(&mut self, pus_tm: &PusTmReader) -> Result<PoolAddr, PoolError> {
            let mut pg = self.0.write().map_err(|_| PoolError::LockError)?;
            let addr = pg.free_element(pus_tm.len_packed(), |buf| {
                buf[0..pus_tm.len_packed()].copy_from_slice(pus_tm.raw_data());
            })?;
            Ok(addr)
        }

        fn add_pus_tm_from_creator(
            &mut self,
            pus_tm: &PusTmCreator,
        ) -> Result<PoolAddr, PoolError> {
            let mut pg = self.0.write().map_err(|_| PoolError::LockError)?;
            let mut result = Ok(0);
            let addr = pg.free_element(pus_tm.len_written(), |buf| {
                result = pus_tm.write_to_bytes(buf);
            })?;
            result?;
            Ok(addr)
        }
    }

    impl CcsdsPacketPool for SharedPacketPool {
        fn add_raw_tc(&mut self, tc_raw: &[u8]) -> Result<PoolAddr, PoolError> {
            let mut pg = self.0.write().map_err(|_| PoolError::LockError)?;
            let addr = pg.free_element(tc_raw.len(), |buf| {
                buf[0..tc_raw.len()].copy_from_slice(tc_raw);
            })?;
            Ok(addr)
        }
    }

    impl PacketHandler for mpsc::Sender<PacketAsVec> {
        type Error = GenericSendError;

        fn handle_packet(&self, sender_id: ComponentId, packet: &[u8]) -> Result<(), Self::Error> {
            self.send(PacketAsVec::new(sender_id, packet.to_vec()))
                .map_err(|_| GenericSendError::RxDisconnected)
        }
    }

    impl PacketHandler for mpsc::SyncSender<PacketAsVec> {
        type Error = GenericSendError;

        fn handle_packet(&self, sender_id: ComponentId, tc_raw: &[u8]) -> Result<(), Self::Error> {
            self.try_send(PacketAsVec::new(sender_id, tc_raw.to_vec()))
                .map_err(|e| match e {
                    mpsc::TrySendError::Full(_) => GenericSendError::QueueFull(None),
                    mpsc::TrySendError::Disconnected(_) => GenericSendError::RxDisconnected,
                })
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq, Error)]
    pub enum StoreAndSendError {
        #[error("Store error: {0}")]
        Store(#[from] PoolError),
        #[error("Genreric send error: {0}")]
        Send(#[from] GenericSendError),
    }

    pub use crate::pool::SharedStaticMemoryPool;

    impl PacketInPoolSender for mpsc::Sender<PacketInPool> {
        fn send_packet(
            &self,
            sender_id: ComponentId,
            store_addr: PoolAddr,
        ) -> Result<(), GenericSendError> {
            self.send(PacketInPool::new(sender_id, store_addr))
                .map_err(|_| GenericSendError::RxDisconnected)
        }
    }

    impl PacketInPoolSender for mpsc::SyncSender<PacketInPool> {
        fn send_packet(
            &self,
            sender_id: ComponentId,
            store_addr: PoolAddr,
        ) -> Result<(), GenericSendError> {
            self.try_send(PacketInPool::new(sender_id, store_addr))
                .map_err(|e| match e {
                    mpsc::TrySendError::Full(_) => GenericSendError::QueueFull(None),
                    mpsc::TrySendError::Disconnected(_) => GenericSendError::RxDisconnected,
                })
        }
    }

    #[cfg(feature = "crossbeam")]
    impl PacketInPoolSender for cb::Sender<PacketInPool> {
        fn send_packet(
            &self,
            sender_id: ComponentId,
            store_addr: PoolAddr,
        ) -> Result<(), GenericSendError> {
            self.try_send(PacketInPool::new(sender_id, store_addr))
                .map_err(|e| match e {
                    cb::TrySendError::Full(_) => GenericSendError::QueueFull(None),
                    cb::TrySendError::Disconnected(_) => GenericSendError::RxDisconnected,
                })
        }
    }

    /// This is the primary structure used to send packets stored in a dedicated memory pool
    /// structure.
    #[derive(Debug, Clone)]
    pub struct PacketSenderWithSharedPool<
        Sender: PacketInPoolSender = mpsc::SyncSender<PacketInPool>,
        PacketPool: CcsdsPacketPool = SharedPacketPool,
    > {
        pub sender: Sender,
        pub shared_pool: RefCell<PacketPool>,
    }

    impl<Sender: PacketInPoolSender> PacketSenderWithSharedPool<Sender, SharedPacketPool> {
        pub fn new_with_shared_packet_pool(
            packet_sender: Sender,
            shared_pool: &SharedStaticMemoryPool,
        ) -> Self {
            Self {
                sender: packet_sender,
                shared_pool: RefCell::new(SharedPacketPool::new(shared_pool)),
            }
        }
    }

    impl<Sender: PacketInPoolSender, PacketStore: CcsdsPacketPool>
        PacketSenderWithSharedPool<Sender, PacketStore>
    {
        pub fn new(packet_sender: Sender, shared_pool: PacketStore) -> Self {
            Self {
                sender: packet_sender,
                shared_pool: RefCell::new(shared_pool),
            }
        }
    }

    impl<Sender: PacketInPoolSender, PacketStore: CcsdsPacketPool + Clone>
        PacketSenderWithSharedPool<Sender, PacketStore>
    {
        pub fn shared_packet_store(&self) -> PacketStore {
            let pool = self.shared_pool.borrow();
            pool.clone()
        }
    }

    impl<Sender: PacketInPoolSender, PacketStore: CcsdsPacketPool + Send> PacketHandler
        for PacketSenderWithSharedPool<Sender, PacketStore>
    {
        type Error = StoreAndSendError;

        fn handle_packet(&self, sender_id: ComponentId, packet: &[u8]) -> Result<(), Self::Error> {
            let mut shared_pool = self.shared_pool.borrow_mut();
            let store_addr = shared_pool.add_raw_tc(packet)?;
            drop(shared_pool);
            self.sender
                .send_packet(sender_id, store_addr)
                .map_err(StoreAndSendError::Send)?;
            Ok(())
        }
    }

    impl<Sender: PacketInPoolSender, PacketStore: CcsdsPacketPool + PusTcPool + Send>
        PacketSenderPusTc for PacketSenderWithSharedPool<Sender, PacketStore>
    {
        type Error = StoreAndSendError;

        fn send_pus_tc(
            &self,
            sender_id: ComponentId,
            _: &SpHeader,
            pus_tc: &PusTcReader,
        ) -> Result<(), Self::Error> {
            let mut shared_pool = self.shared_pool.borrow_mut();
            let store_addr = shared_pool.add_raw_tc(pus_tc.raw_data())?;
            drop(shared_pool);
            self.sender
                .send_packet(sender_id, store_addr)
                .map_err(StoreAndSendError::Send)?;
            Ok(())
        }
    }

    impl<Sender: PacketInPoolSender, PacketStore: CcsdsPacketPool + Send> PacketSenderCcsds
        for PacketSenderWithSharedPool<Sender, PacketStore>
    {
        type Error = StoreAndSendError;

        fn send_ccsds(
            &self,
            sender_id: ComponentId,
            _sp_header: &SpHeader,
            tc_raw: &[u8],
        ) -> Result<(), Self::Error> {
            self.handle_packet(sender_id, tc_raw)
        }
    }

    impl<Sender: PacketInPoolSender, PacketStore: CcsdsPacketPool + PusTmPool + Send> EcssTmSender
        for PacketSenderWithSharedPool<Sender, PacketStore>
    {
        fn send_tm(
            &self,
            sender_id: crate::ComponentId,
            tm: crate::legacy::pus::PusTmVariant,
        ) -> Result<(), crate::legacy::pus::EcssTmtcError> {
            let send_addr = |store_addr: PoolAddr| {
                self.sender
                    .send_packet(sender_id, store_addr)
                    .map_err(EcssTmtcError::Send)
            };
            match tm {
                crate::legacy::pus::PusTmVariant::InStore(store_addr) => send_addr(store_addr),
                crate::legacy::pus::PusTmVariant::Direct(tm_creator) => {
                    let mut pool = self.shared_pool.borrow_mut();
                    let store_addr = pool.add_pus_tm_from_creator(&tm_creator)?;
                    send_addr(store_addr)
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use alloc::vec;

    use std::sync::RwLock;

    use crate::pool::{
        PoolProviderWithGuards, SharedStaticMemoryPool, StaticMemoryPool, StaticPoolConfig,
    };

    use super::*;
    use std::sync::mpsc;

    pub(crate) fn send_with_sender<SendError>(
        sender_id: ComponentId,
        packet_sender: &(impl PacketHandler<Error = SendError> + ?Sized),
        packet: &[u8],
    ) -> Result<(), SendError> {
        packet_sender.handle_packet(sender_id, packet)
    }

    #[test]
    fn test_basic_mpsc_channel_sender_bounded() {
        let (tx, rx) = mpsc::channel();
        let some_packet = vec![1, 2, 3, 4, 5];
        send_with_sender(1, &tx, &some_packet).expect("failed to send packet");
        let rx_packet = rx.try_recv().unwrap();
        assert_eq!(some_packet, rx_packet.packet);
        assert_eq!(1, rx_packet.sender_id);
    }

    #[test]
    fn test_basic_mpsc_channel_receiver_dropped() {
        let (tx, rx) = mpsc::channel();
        let some_packet = vec![1, 2, 3, 4, 5];
        drop(rx);
        let result = send_with_sender(2, &tx, &some_packet);
        assert!(result.is_err());
        matches!(result.unwrap_err(), GenericSendError::RxDisconnected);
    }

    #[test]
    fn test_basic_mpsc_sync_sender() {
        let (tx, rx) = mpsc::sync_channel(3);
        let some_packet = vec![1, 2, 3, 4, 5];
        send_with_sender(3, &tx, &some_packet).expect("failed to send packet");
        let rx_packet = rx.try_recv().unwrap();
        assert_eq!(some_packet, rx_packet.packet);
        assert_eq!(3, rx_packet.sender_id);
    }

    #[test]
    fn test_basic_mpsc_sync_sender_receiver_dropped() {
        let (tx, rx) = mpsc::sync_channel(3);
        let some_packet = vec![1, 2, 3, 4, 5];
        drop(rx);
        let result = send_with_sender(0, &tx, &some_packet);
        assert!(result.is_err());
        matches!(result.unwrap_err(), GenericSendError::RxDisconnected);
    }

    #[test]
    fn test_basic_mpsc_sync_sender_queue_full() {
        let (tx, rx) = mpsc::sync_channel(1);
        let some_packet = vec![1, 2, 3, 4, 5];
        send_with_sender(0, &tx, &some_packet).expect("failed to send packet");
        let result = send_with_sender(1, &tx, &some_packet);
        assert!(result.is_err());
        matches!(result.unwrap_err(), GenericSendError::QueueFull(None));
        let rx_packet = rx.try_recv().unwrap();
        assert_eq!(some_packet, rx_packet.packet);
    }

    #[test]
    fn test_basic_shared_store_sender_unbounded_sender() {
        let (tc_tx, tc_rx) = mpsc::channel();
        let pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(vec![(2, 8)], true);
        let shared_pool = SharedPacketPool::new(&SharedStaticMemoryPool::new(RwLock::new(
            StaticMemoryPool::new(pool_cfg),
        )));
        let some_packet = vec![1, 2, 3, 4, 5];
        let tc_sender = PacketSenderWithSharedPool::new(tc_tx, shared_pool.clone());
        send_with_sender(5, &tc_sender, &some_packet).expect("failed to send packet");
        let packet_in_pool = tc_rx.try_recv().unwrap();
        let mut pool = shared_pool.0.write().unwrap();
        let read_guard = pool.read_with_guard(packet_in_pool.store_addr);
        assert_eq!(read_guard.read_as_vec().unwrap(), some_packet);
        assert_eq!(packet_in_pool.sender_id, 5)
    }

    #[test]
    fn test_basic_shared_store_sender() {
        let (tc_tx, tc_rx) = mpsc::sync_channel(10);
        let pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(vec![(2, 8)], true);
        let shared_pool = SharedPacketPool::new(&SharedStaticMemoryPool::new(RwLock::new(
            StaticMemoryPool::new(pool_cfg),
        )));
        let some_packet = vec![1, 2, 3, 4, 5];
        let tc_sender = PacketSenderWithSharedPool::new(tc_tx, shared_pool.clone());
        send_with_sender(5, &tc_sender, &some_packet).expect("failed to send packet");
        let packet_in_pool = tc_rx.try_recv().unwrap();
        let mut pool = shared_pool.0.write().unwrap();
        let read_guard = pool.read_with_guard(packet_in_pool.store_addr);
        assert_eq!(read_guard.read_as_vec().unwrap(), some_packet);
        assert_eq!(packet_in_pool.sender_id, 5)
    }

    #[test]
    fn test_basic_shared_store_sender_rx_dropped() {
        let (tc_tx, tc_rx) = mpsc::sync_channel(10);
        let pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(vec![(2, 8)], true);
        let shared_pool = SharedPacketPool::new(&SharedStaticMemoryPool::new(RwLock::new(
            StaticMemoryPool::new(pool_cfg),
        )));
        let some_packet = vec![1, 2, 3, 4, 5];
        drop(tc_rx);
        let tc_sender = PacketSenderWithSharedPool::new(tc_tx, shared_pool.clone());
        let result = send_with_sender(2, &tc_sender, &some_packet);
        assert!(result.is_err());
        matches!(
            result.unwrap_err(),
            StoreAndSendError::Send(GenericSendError::RxDisconnected)
        );
    }

    #[test]
    fn test_basic_shared_store_sender_queue_full() {
        let (tc_tx, tc_rx) = mpsc::sync_channel(1);
        let pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(vec![(2, 8)], true);
        let shared_pool = SharedPacketPool::new(&SharedStaticMemoryPool::new(RwLock::new(
            StaticMemoryPool::new(pool_cfg),
        )));
        let some_packet = vec![1, 2, 3, 4, 5];
        let tc_sender = PacketSenderWithSharedPool::new(tc_tx, shared_pool.clone());
        send_with_sender(3, &tc_sender, &some_packet).expect("failed to send packet");
        let result = send_with_sender(3, &tc_sender, &some_packet);
        assert!(result.is_err());
        matches!(
            result.unwrap_err(),
            StoreAndSendError::Send(GenericSendError::RxDisconnected)
        );
        let packet_in_pool = tc_rx.try_recv().unwrap();
        let mut pool = shared_pool.0.write().unwrap();
        let read_guard = pool.read_with_guard(packet_in_pool.store_addr);
        assert_eq!(read_guard.read_as_vec().unwrap(), some_packet);
        assert_eq!(packet_in_pool.sender_id, 3);
    }

    #[test]
    fn test_basic_shared_store_store_error() {
        let (tc_tx, tc_rx) = mpsc::sync_channel(1);
        let pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(vec![(1, 8)], true);
        let shared_pool = SharedPacketPool::new(&SharedStaticMemoryPool::new(RwLock::new(
            StaticMemoryPool::new(pool_cfg),
        )));
        let some_packet = vec![1, 2, 3, 4, 5];
        let tc_sender = PacketSenderWithSharedPool::new(tc_tx, shared_pool.clone());
        send_with_sender(4, &tc_sender, &some_packet).expect("failed to send packet");
        let result = send_with_sender(4, &tc_sender, &some_packet);
        assert!(result.is_err());
        matches!(
            result.unwrap_err(),
            StoreAndSendError::Store(PoolError::StoreFull(..))
        );
        let packet_in_pool = tc_rx.try_recv().unwrap();
        let mut pool = shared_pool.0.write().unwrap();
        let read_guard = pool.read_with_guard(packet_in_pool.store_addr);
        assert_eq!(read_guard.read_as_vec().unwrap(), some_packet);
        assert_eq!(packet_in_pool.sender_id, 4);
    }
}
