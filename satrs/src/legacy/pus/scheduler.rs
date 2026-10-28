//! # PUS Service 11 Scheduling Module
//!
//! The core data structure of this module is the [PusScheduler]. This structure can be used
//! to perform the scheduling of telecommands like specified in the ECSS standard.
use arbitrary_int::{u11, u14};
use core::fmt::Debug;
use core::time::Duration;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use spacepackets::ecss::scheduling::TimeWindowType;
use spacepackets::ecss::tc::{GenericPusTcSecondaryHeader, IsPusTelecommand, PusTcReader};
use spacepackets::ecss::{PusError, PusPacket, WritablePusPacket};
use spacepackets::time::{CcsdsTimeProvider, TimeReader, TimeWriter, TimestampError, UnixTime};
use spacepackets::{ByteConversionError, CcsdsPacket};

use crate::pool::{PoolError, PoolProvider};
#[cfg(feature = "alloc")]
pub use alloc_mod::*;

/// This is the request ID as specified in ECSS-E-ST-70-41C 5.4.11.2 of the standard.
///
/// This version of the request ID is used to identify scheduled commands and also contains
/// the source ID found in the secondary header of PUS telecommands.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct RequestId {
    pub(crate) source_id: u16,
    pub(crate) apid: u11,
    pub(crate) seq_count: u14,
}

impl RequestId {
    #[inline]
    pub const fn source_id(&self) -> u16 {
        self.source_id
    }

    #[inline]
    pub const fn apid(&self) -> u11 {
        self.apid
    }

    #[inline]
    pub const fn seq_count(&self) -> u14 {
        self.seq_count
    }

    pub fn from_tc(
        tc: &(impl CcsdsPacket + GenericPusTcSecondaryHeader + IsPusTelecommand),
    ) -> Self {
        RequestId {
            source_id: tc.source_id(),
            apid: tc.apid(),
            seq_count: tc.seq_count(),
        }
    }

    pub const fn as_u64(&self) -> u64 {
        ((self.source_id as u64) << 32)
            | ((self.apid.value() as u64) << 16)
            | self.seq_count.value() as u64
    }
}

pub type AddrInStore = u64;

/// This is the format stored internally by the TC scheduler for each scheduled telecommand.
/// It consists of a generic address for that telecommand in the TC pool and a request ID.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct TcInfo {
    addr: AddrInStore,
    request_id: RequestId,
}

impl TcInfo {
    pub fn addr(&self) -> AddrInStore {
        self.addr
    }

    pub fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub fn new(addr: u64, request_id: RequestId) -> Self {
        TcInfo { addr, request_id }
    }
}

pub struct TimeWindow<TimeProvder> {
    time_window_type: TimeWindowType,
    start_time: Option<TimeProvder>,
    end_time: Option<TimeProvder>,
}

impl<TimeProvider> TimeWindow<TimeProvider> {
    pub fn new_select_all() -> Self {
        Self {
            time_window_type: TimeWindowType::SelectAll,
            start_time: None,
            end_time: None,
        }
    }

    pub fn time_window_type(&self) -> TimeWindowType {
        self.time_window_type
    }

    pub fn start_time(&self) -> Option<&TimeProvider> {
        self.start_time.as_ref()
    }

    pub fn end_time(&self) -> Option<&TimeProvider> {
        self.end_time.as_ref()
    }
}

impl<TimeProvider: CcsdsTimeProvider + Clone> TimeWindow<TimeProvider> {
    pub fn new_from_time_to_time(start_time: &TimeProvider, end_time: &TimeProvider) -> Self {
        Self {
            time_window_type: TimeWindowType::TimeTagToTimeTag,
            start_time: Some(start_time.clone()),
            end_time: Some(end_time.clone()),
        }
    }

    pub fn new_from_time(start_time: &TimeProvider) -> Self {
        Self {
            time_window_type: TimeWindowType::FromTimeTag,
            start_time: Some(start_time.clone()),
            end_time: None,
        }
    }

    pub fn new_to_time(end_time: &TimeProvider) -> Self {
        Self {
            time_window_type: TimeWindowType::ToTimeTag,
            start_time: None,
            end_time: Some(end_time.clone()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum ScheduleError {
    #[error("pus error: {0}")]
    PusError(#[from] PusError),
    /// The release time is within the time-margin added on top of the current time.
    /// The first parameter is the current time, the second one the time margin, and the third one
    /// the release time.
    #[error("release time in margin")]
    ReleaseTimeInTimeMargin {
        current_time: UnixTime,
        time_margin: Duration,
        release_time: UnixTime,
    },
    /// Nested time-tagged commands are not allowed.
    #[error("nested scheduled tc")]
    NestedScheduledTc,
    #[error("store error")]
    Pool(#[from] PoolError),
    #[error("tc data empty")]
    TcDataEmpty,
    #[error("timestamp error: {0}")]
    TimestampError(#[from] TimestampError),
    #[error("wrong subservice number {0}")]
    WrongSubservice(u8),
    #[error("wrong service number {0}")]
    WrongService(u8),
    #[error("byte conversion error: {0}")]
    ByteConversionError(#[from] ByteConversionError),
}

/// Generic trait for scheduler objects which are able to schedule ECSS PUS C packets.
pub trait PusScheduler {
    type TimeProvider: CcsdsTimeProvider + TimeReader;

    fn reset(&mut self, store: &mut (impl PoolProvider + ?Sized)) -> Result<(), PoolError>;

    fn is_enabled(&self) -> bool;

    fn enable(&mut self);

    /// A disabled scheduler should still delete commands where the execution time has been reached
    /// but should not release them to be executed.
    fn disable(&mut self);

    /// Insert a telecommand which was already unwrapped from the outer Service 11 packet and stored
    /// inside the telecommand packet pool.
    fn insert_unwrapped_and_stored_tc(
        &mut self,
        time_stamp: UnixTime,
        info: TcInfo,
    ) -> Result<(), ScheduleError>;

    /// Insert a telecommand based on the fully wrapped time-tagged telecommand. The timestamp
    /// provider needs to be supplied via a generic.
    fn insert_wrapped_tc<TimeProvider>(
        &mut self,
        pus_tc: &(impl IsPusTelecommand + PusPacket + GenericPusTcSecondaryHeader),
        pool: &mut (impl PoolProvider + ?Sized),
    ) -> Result<TcInfo, ScheduleError> {
        if PusPacket::service_type_id(pus_tc) != 11 {
            return Err(ScheduleError::WrongService(PusPacket::service_type_id(
                pus_tc,
            )));
        }
        if PusPacket::message_subtype_id(pus_tc) != 4 {
            return Err(ScheduleError::WrongSubservice(
                PusPacket::message_subtype_id(pus_tc),
            ));
        }
        if pus_tc.user_data().is_empty() {
            return Err(ScheduleError::TcDataEmpty);
        }
        let user_data = pus_tc.user_data();
        let stamp: Self::TimeProvider = TimeReader::from_bytes(user_data)?;
        let unix_stamp = stamp.unix_time();
        let stamp_len = stamp.len_as_bytes();
        self.insert_unwrapped_tc(unix_stamp, &user_data[stamp_len..], pool)
    }

    /// Insert a telecommand which was already unwrapped from the outer Service 11 packet but still
    /// needs to be stored inside the telecommand pool.
    fn insert_unwrapped_tc(
        &mut self,
        time_stamp: UnixTime,
        tc: &[u8],
        pool: &mut (impl PoolProvider + ?Sized),
    ) -> Result<TcInfo, ScheduleError> {
        let check_tc = PusTcReader::new(tc)?;
        if PusPacket::service_type_id(&check_tc) == 11
            && PusPacket::message_subtype_id(&check_tc) == 4
        {
            return Err(ScheduleError::NestedScheduledTc);
        }
        let req_id = RequestId::from_tc(&check_tc);

        match pool.add(tc) {
            Ok(addr) => {
                let info = TcInfo::new(addr, req_id);
                self.insert_unwrapped_and_stored_tc(time_stamp, info)?;
                Ok(info)
            }
            Err(err) => Err(err.into()),
        }
    }
}

/// Helper function to generate the application data for a PUS telecommand to insert an
/// activity into a time-based schedule according to ECSS-E-ST-70-41C 8.11.2.4
///
/// Please note that the N field is set to a [u16] unsigned bytefield with the value 1.
pub fn generate_insert_telecommand_app_data(
    buf: &mut [u8],
    release_time: &impl TimeWriter,
    request: &impl WritablePusPacket,
) -> Result<usize, ScheduleError> {
    let required_len = 2 + release_time.len_written() + request.len_written();
    if required_len > buf.len() {
        return Err(ByteConversionError::ToSliceTooSmall {
            found: buf.len(),
            expected: required_len,
        }
        .into());
    }
    let mut current_len = 0;
    let n = 1_u16;
    buf[current_len..current_len + 2].copy_from_slice(&n.to_be_bytes());
    current_len += 2;
    current_len += release_time
        .write_to_slice(&mut buf[current_len..current_len + release_time.len_written()])?;
    current_len +=
        request.write_to_slice(&mut buf[current_len..current_len + request.len_written()])?;
    Ok(current_len)
}

#[cfg(feature = "alloc")]
pub mod alloc_mod {
    use alloc::{
        collections::{
            BTreeMap,
            btree_map::{Entry, Range},
        },
        vec::Vec,
    };
    use spacepackets::time::cds::{self, DaysLen24Bits};

    use crate::pool::PoolAddr;

    use super::*;

    #[cfg(feature = "std")]
    use std::time::SystemTimeError;

    /// This function is similar to [generate_insert_telecommand_app_data] but returns the application
    /// data as a [alloc::vec::Vec].
    pub fn generate_insert_telecommand_app_data_as_vec(
        release_time: &impl TimeWriter,
        request: &impl WritablePusPacket,
    ) -> Result<alloc::vec::Vec<u8>, ScheduleError> {
        let mut vec = alloc::vec::Vec::new();
        vec.extend_from_slice(&1_u16.to_be_bytes());
        vec.append(&mut release_time.to_vec()?);
        vec.append(&mut request.to_vec()?);
        Ok(vec)
    }

    enum DeletionResult {
        WithoutStoreDeletion(Option<PoolAddr>),
        WithStoreDeletion(Result<bool, PoolError>),
    }

    /// This is the core data structure for scheduling PUS telecommands with [alloc] support.
    ///
    /// It is assumed that the actual telecommand data is stored in a separate TC pool offering
    /// a [crate::pool::PoolProvider] API. This data structure just tracks the store
    /// addresses and their release times and offers a convenient API to insert and release
    /// telecommands and perform other functionality specified by the ECSS standard in section 6.11.
    /// The time is tracked as a [spacepackets::time::UnixTime] but the only requirement to
    /// the timekeeping of the user is that it is convertible to that timestamp.
    ///
    /// The standard also specifies that the PUS scheduler can be enabled and disabled.
    /// A disabled scheduler should still delete commands where the execution time has been reached
    /// but should not release them to be executed.
    ///
    /// The implementation uses an ordered map internally with the release timestamp being the key.
    /// This allows efficient time based insertions and extractions which should be the primary use-case
    /// for a time-based command scheduler.
    /// There is no way to avoid duplicate [RequestId]s during insertion, which can occur even if the
    /// user always correctly increment for sequence counter due to overflows. To avoid this issue,
    /// it can make sense to split up telecommand groups by the APID to avoid overflows.
    ///
    /// Currently, sub-schedules and groups are not supported.
    #[derive(Debug)]
    pub struct PusSchedulerAlloc {
        // TODO: Use MonotonicTime from tai-time crate instead of UnixTime and cache leap seconds.
        // TODO: Introduce optional limit of commands stored in the TC map. If a limit is set,
        // there will be a check for each insertion whether the map is full, making the memory
        // usage of the scheduler more deterministic.
        tc_map: BTreeMap<UnixTime, Vec<TcInfo>>,
        pub(crate) current_time: UnixTime,
        time_margin: Duration,
        enabled: bool,
    }

    impl PusSchedulerAlloc {
        /// Create a new PUS scheduler.
        ///
        /// # Arguments
        ///
        /// * `init_current_time` - The time to initialize the scheduler with.
        /// * `time_margin` - This time margin is used when inserting new telecommands into the
        ///   schedule. If the release time of a new telecommand is earlier than the time margin
        ///   added to the current time, it will not be inserted into the schedule.
        /// * `tc_buf_size` - Buffer for temporary storage of telecommand packets. This buffer
        ///   should be large enough to accomodate the largest expected TC packets.
        pub fn new(init_current_time: UnixTime, time_margin: Duration) -> Self {
            PusSchedulerAlloc {
                tc_map: Default::default(),
                current_time: init_current_time,
                time_margin,
                enabled: true,
            }
        }

        /// Like [Self::new], but sets the `init_current_time` parameter to the current system time.
        #[cfg(feature = "std")]
        pub fn new_with_current_init_time(time_margin: Duration) -> Result<Self, SystemTimeError> {
            Ok(Self::new(UnixTime::now()?, time_margin))
        }

        pub fn num_scheduled_telecommands(&self) -> u64 {
            let mut num_entries = 0;
            for entries in &self.tc_map {
                num_entries += entries.1.len() as u64;
            }
            num_entries
        }

        #[inline]
        pub fn update_time(&mut self, current_time: UnixTime) {
            self.current_time = current_time;
        }

        #[inline]
        pub fn current_time(&self) -> &UnixTime {
            &self.current_time
        }

        /// Insert a telecommand which was already unwrapped from the outer Service 11 packet and stored
        /// inside the telecommand packet pool.
        pub fn insert_unwrapped_and_stored_tc(
            &mut self,
            time_stamp: UnixTime,
            info: TcInfo,
        ) -> Result<(), ScheduleError> {
            if time_stamp < self.current_time + self.time_margin {
                return Err(ScheduleError::ReleaseTimeInTimeMargin {
                    current_time: self.current_time,
                    time_margin: self.time_margin,
                    release_time: time_stamp,
                });
            }
            match self.tc_map.entry(time_stamp) {
                Entry::Vacant(e) => {
                    e.insert(alloc::vec![info]);
                }
                Entry::Occupied(mut v) => {
                    v.get_mut().push(info);
                }
            }
            Ok(())
        }

        /// Insert a telecommand which was already unwrapped from the outer Service 11 packet but still
        /// needs to be stored inside the telecommand pool.
        pub fn insert_unwrapped_tc(
            &mut self,
            time_stamp: UnixTime,
            tc: &[u8],
            pool: &mut (impl PoolProvider + ?Sized),
        ) -> Result<TcInfo, ScheduleError> {
            let check_tc = PusTcReader::new(tc)?;
            if PusPacket::service_type_id(&check_tc) == 11
                && PusPacket::message_subtype_id(&check_tc) == 4
            {
                return Err(ScheduleError::NestedScheduledTc);
            }
            let req_id = RequestId::from_tc(&check_tc);

            match pool.add(tc) {
                Ok(addr) => {
                    let info = TcInfo::new(addr, req_id);
                    self.insert_unwrapped_and_stored_tc(time_stamp, info)?;
                    Ok(info)
                }
                Err(err) => Err(err.into()),
            }
        }

        /// Insert a telecommand based on the fully wrapped time-tagged telecommand using a CDS
        /// short timestamp with 16-bit length of days field.
        pub fn insert_wrapped_tc_cds_short(
            &mut self,
            pus_tc: &(impl IsPusTelecommand + PusPacket + GenericPusTcSecondaryHeader),
            pool: &mut (impl PoolProvider + ?Sized),
        ) -> Result<TcInfo, ScheduleError> {
            self.insert_wrapped_tc::<cds::CdsTime>(pus_tc, pool)
        }

        /// Insert a telecommand based on the fully wrapped time-tagged telecommand using a CDS
        /// long timestamp with a 24-bit length of days field.
        pub fn insert_wrapped_tc_cds_long(
            &mut self,
            pus_tc: &(impl IsPusTelecommand + PusPacket + GenericPusTcSecondaryHeader),
            pool: &mut (impl PoolProvider + ?Sized),
        ) -> Result<TcInfo, ScheduleError> {
            self.insert_wrapped_tc::<cds::CdsTime<DaysLen24Bits>>(pus_tc, pool)
        }

        /// This function uses [Self::retrieve_by_time_filter] to extract all scheduled commands inside
        /// the time range and then deletes them from the provided store.
        ///
        /// Like specified in the documentation of [Self::retrieve_by_time_filter], the range extraction
        /// for deletion is always inclusive.
        ///
        /// This function returns the number of deleted commands on success. In case any deletion fails,
        /// the last deletion will be supplied in addition to the number of deleted commands.
        pub fn delete_by_time_filter<TimeProvider: CcsdsTimeProvider + Clone>(
            &mut self,
            time_window: TimeWindow<TimeProvider>,
            pool: &mut (impl PoolProvider + ?Sized),
        ) -> Result<u64, (u64, PoolError)> {
            let range = self.retrieve_by_time_filter(time_window);
            let mut del_packets = 0;
            let mut res_if_fails = None;
            let mut keys_to_delete = alloc::vec::Vec::new();
            for time_bucket in range {
                for tc in time_bucket.1 {
                    match pool.delete(tc.addr) {
                        Ok(_) => del_packets += 1,
                        Err(e) => res_if_fails = Some(e),
                    }
                }
                keys_to_delete.push(*time_bucket.0);
            }
            for key in keys_to_delete {
                self.tc_map.remove(&key);
            }
            if let Some(err) = res_if_fails {
                return Err((del_packets, err));
            }
            Ok(del_packets)
        }

        /// Deletes all the scheduled commands. This also deletes the packets from the passed TC pool.
        ///
        /// This function returns the number of deleted commands on success. In case any deletion fails,
        /// the last deletion will be supplied in addition to the number of deleted commands.
        pub fn delete_all(
            &mut self,
            pool: &mut (impl PoolProvider + ?Sized),
        ) -> Result<u64, (u64, PoolError)> {
            self.delete_by_time_filter(TimeWindow::<cds::CdsTime>::new_select_all(), pool)
        }

        /// Retrieve a range over all scheduled commands.
        pub fn retrieve_all(
            &mut self,
        ) -> alloc::collections::btree_map::Range<'_, UnixTime, alloc::vec::Vec<TcInfo>> {
            self.tc_map.range(..)
        }

        /// This retrieves scheduled telecommands which are inside the provided time window.
        ///
        /// It should be noted that the ranged extraction is always inclusive. For example, a range
        /// from 50 to 100 unix seconds would also include command scheduled at 100 unix seconds.
        pub fn retrieve_by_time_filter<TimeProvider: CcsdsTimeProvider>(
            &mut self,
            time_window: TimeWindow<TimeProvider>,
        ) -> Range<'_, UnixTime, alloc::vec::Vec<TcInfo>> {
            match time_window.time_window_type() {
                TimeWindowType::SelectAll => self.tc_map.range(..),
                TimeWindowType::TimeTagToTimeTag => {
                    // This should be guaranteed to be valid by library API, so unwrap is okay
                    let start_time = time_window.start_time().unwrap().unix_time();
                    let end_time = time_window.end_time().unwrap().unix_time();
                    self.tc_map.range(start_time..=end_time)
                }
                TimeWindowType::FromTimeTag => {
                    // This should be guaranteed to be valid by library API, so unwrap is okay
                    let start_time = time_window.start_time().unwrap().unix_time();
                    self.tc_map.range(start_time..)
                }
                TimeWindowType::ToTimeTag => {
                    // This should be guaranteed to be valid by library API, so unwrap is okay
                    let end_time = time_window.end_time().unwrap().unix_time();
                    self.tc_map.range(..=end_time)
                }
            }
        }

        /// Deletes a scheduled command with the given request  ID. Returns the store address if a
        /// scheduled command was found in the map and deleted, and None otherwise.
        ///
        /// Please note that this function will stop on the first telecommand with a request ID match.
        /// In case of duplicate IDs (which should generally not happen), this function needs to be
        /// called repeatedly.
        pub fn delete_by_request_id(&mut self, req_id: &RequestId) -> Option<PoolAddr> {
            if let DeletionResult::WithoutStoreDeletion(v) =
                self.delete_by_request_id_internal_without_store_deletion(req_id)
            {
                return v;
            }
            panic!("unexpected deletion result");
        }

        /// This behaves like [Self::delete_by_request_id] but deletes the packet from the pool as well.
        pub fn delete_by_request_id_and_from_pool(
            &mut self,
            req_id: &RequestId,
            pool: &mut (impl PoolProvider + ?Sized),
        ) -> Result<bool, PoolError> {
            if let DeletionResult::WithStoreDeletion(v) =
                self.delete_by_request_id_internal_with_store_deletion(req_id, pool)
            {
                return v;
            }
            panic!("unexpected deletion result");
        }

        fn delete_by_request_id_internal_without_store_deletion(
            &mut self,
            req_id: &RequestId,
        ) -> DeletionResult {
            let mut idx_found = None;
            for time_bucket in &mut self.tc_map {
                for (idx, tc_info) in time_bucket.1.iter().enumerate() {
                    if &tc_info.request_id == req_id {
                        idx_found = Some(idx);
                    }
                }
                if let Some(idx) = idx_found {
                    let addr = time_bucket.1.remove(idx).addr;
                    return DeletionResult::WithoutStoreDeletion(Some(addr));
                }
            }
            DeletionResult::WithoutStoreDeletion(None)
        }

        fn delete_by_request_id_internal_with_store_deletion(
            &mut self,
            req_id: &RequestId,
            pool: &mut (impl PoolProvider + ?Sized),
        ) -> DeletionResult {
            let mut idx_found = None;
            for time_bucket in &mut self.tc_map {
                for (idx, tc_info) in time_bucket.1.iter().enumerate() {
                    if &tc_info.request_id == req_id {
                        idx_found = Some(idx);
                    }
                }
                if let Some(idx) = idx_found {
                    let addr = time_bucket.1.remove(idx).addr;
                    return match pool.delete(addr) {
                        Ok(_) => DeletionResult::WithStoreDeletion(Ok(true)),
                        Err(e) => DeletionResult::WithStoreDeletion(Err(e)),
                    };
                }
            }
            DeletionResult::WithStoreDeletion(Ok(false))
        }

        #[cfg(feature = "std")]
        pub fn update_time_from_now(&mut self) -> Result<(), SystemTimeError> {
            self.current_time = UnixTime::now()?;
            Ok(())
        }

        /// Utility method which calls [Self::telecommands_to_release] and then calls a releaser
        /// closure for each telecommand which should be released. This function will also delete
        /// the telecommands from the holding store after calling the release closure if the user
        /// returns [true] from the release closure. A buffer must be provided to hold the
        /// telecommands for the release process.
        ///
        /// # Arguments
        ///
        /// * `releaser` - Closure where the first argument is whether the scheduler is enabled and
        ///   the second argument is the telecommand information also containing the store
        ///   address. This closure should return whether the command should be deleted. Please
        ///   note that returning false might lead to memory leaks if the TC is not cleared from
        ///   the store in some other way.
        /// * `tc_store` - The holding store of the telecommands.
        /// * `tc_buf` - Buffer to hold each telecommand being released.
        pub fn release_telecommands_with_buffer<R: FnMut(bool, &TcInfo, &[u8]) -> bool>(
            &mut self,
            releaser: R,
            tc_store: &mut (impl PoolProvider + ?Sized),
            tc_buf: &mut [u8],
        ) -> Result<u64, (u64, PoolError)> {
            self.release_telecommands_internal(releaser, tc_store, Some(tc_buf))
        }

        /// This functions is almost identical to [Self::release_telecommands_with_buffer] but does
        /// not require a user provided TC buffer because it will always use the
        /// [PoolProvider::read_as_vec] API to read the TC packets.
        ///
        /// However, this might also perform frequent allocations for all telecommands being
        /// released.
        pub fn release_telecommands<R: FnMut(bool, &TcInfo, &[u8]) -> bool>(
            &mut self,
            releaser: R,
            tc_store: &mut (impl PoolProvider + ?Sized),
        ) -> Result<u64, (u64, PoolError)> {
            self.release_telecommands_internal(releaser, tc_store, None)
        }

        fn release_telecommands_internal<R: FnMut(bool, &TcInfo, &[u8]) -> bool>(
            &mut self,
            mut releaser: R,
            tc_store: &mut (impl PoolProvider + ?Sized),
            mut tc_buf: Option<&mut [u8]>,
        ) -> Result<u64, (u64, PoolError)> {
            let tcs_to_release = self.telecommands_to_release();
            let mut released_tcs = 0;
            let mut store_error = Ok(());
            for tc in tcs_to_release {
                for info in tc.1 {
                    let should_delete = match tc_buf.as_mut() {
                        Some(buf) => {
                            tc_store
                                .read(&info.addr, buf)
                                .map_err(|e| (released_tcs, e))?;
                            releaser(self.enabled, info, buf)
                        }
                        None => {
                            let tc = tc_store
                                .read_as_vec(&info.addr)
                                .map_err(|e| (released_tcs, e))?;
                            releaser(self.enabled, info, &tc)
                        }
                    };
                    released_tcs += 1;
                    if should_delete {
                        let res = tc_store.delete(info.addr);
                        if res.is_err() {
                            store_error = res;
                        }
                    }
                }
            }
            self.tc_map.retain(|k, _| k > &self.current_time);
            store_error
                .map(|_| released_tcs)
                .map_err(|e| (released_tcs, e))
        }

        /// This utility method is similar to [Self::release_telecommands] but will not perform
        /// store deletions and thus does not require a mutable reference of the TC store.
        ///
        /// It will returns a [Vec] of [TcInfo]s to transfer the list of released
        /// telecommands to the user. The user should take care of deleting those telecommands
        /// from the holding store to prevent memory leaks.
        pub fn release_telecommands_no_deletion<R: FnMut(bool, &TcInfo, &[u8])>(
            &mut self,
            mut releaser: R,
            tc_store: &(impl PoolProvider + ?Sized),
            tc_buf: &mut [u8],
        ) -> Result<alloc::vec::Vec<TcInfo>, (alloc::vec::Vec<TcInfo>, PoolError)> {
            let tcs_to_release = self.telecommands_to_release();
            let mut released_tcs = alloc::vec::Vec::new();
            for tc in tcs_to_release {
                for info in tc.1 {
                    tc_store
                        .read(&info.addr, tc_buf)
                        .map_err(|e| (released_tcs.clone(), e))?;
                    releaser(self.is_enabled(), info, tc_buf);
                    released_tcs.push(*info);
                }
            }
            self.tc_map.retain(|k, _| k > &self.current_time);
            Ok(released_tcs)
        }

        /// Retrieve all telecommands which should be release based on the current time.
        pub fn telecommands_to_release(&self) -> Range<'_, UnixTime, Vec<TcInfo>> {
            self.tc_map.range(..=self.current_time)
        }
    }

    impl PusScheduler for PusSchedulerAlloc {
        type TimeProvider = cds::CdsTime;

        /// This will disable the scheduler and clear the schedule as specified in 6.11.4.4.
        /// Be careful with this command as it will delete all the commands in the schedule.
        ///
        /// The holding store for the telecommands needs to be passed so all the stored telecommands
        /// can be deleted to avoid a memory leak. If at last one deletion operation fails, the error
        /// will be returned but the method will still try to delete all the commands in the schedule.
        fn reset(&mut self, store: &mut (impl PoolProvider + ?Sized)) -> Result<(), PoolError> {
            self.enabled = false;
            let mut deletion_ok = Ok(());
            for tc_lists in &mut self.tc_map {
                for tc in tc_lists.1 {
                    let res = store.delete(tc.addr);
                    if res.is_err() {
                        deletion_ok = res;
                    }
                }
            }
            self.tc_map.clear();
            deletion_ok
        }

        fn is_enabled(&self) -> bool {
            self.enabled
        }

        fn enable(&mut self) {
            self.enabled = true;
        }

        /// A disabled scheduler should still delete commands where the execution time has been reached
        /// but should not release them to be executed.
        fn disable(&mut self) {
            self.enabled = false;
        }

        fn insert_unwrapped_and_stored_tc(
            &mut self,
            time_stamp: UnixTime,
            info: TcInfo,
        ) -> Result<(), ScheduleError> {
            if time_stamp < self.current_time + self.time_margin {
                return Err(ScheduleError::ReleaseTimeInTimeMargin {
                    current_time: self.current_time,
                    time_margin: self.time_margin,
                    release_time: time_stamp,
                });
            }
            match self.tc_map.entry(time_stamp) {
                Entry::Vacant(e) => {
                    e.insert(alloc::vec![info]);
                }
                Entry::Occupied(mut v) => {
                    v.get_mut().push(info);
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{
        PoolAddr, PoolError, PoolProvider, StaticMemoryPool, StaticPoolAddr, StaticPoolConfig,
    };
    use alloc::collections::btree_map::Range;
    use arbitrary_int::traits::Integer;
    use arbitrary_int::{u11, u14};
    use spacepackets::ecss::tc::{PusTcCreator, PusTcReader, PusTcSecondaryHeader};
    use spacepackets::ecss::{CreatorConfig, MessageTypeId};
    use spacepackets::time::{UnixTime, cds};
    use spacepackets::{PacketId, PacketSequenceControl, PacketType, SequenceFlags, SpHeader};
    use std::time::Duration;
    use std::vec::Vec;
    #[allow(unused_imports)]
    use std::{println, vec};

    fn pus_tc_base(timestamp: UnixTime, buf: &mut [u8]) -> (SpHeader, usize) {
        let cds_time =
            cds::CdsTime::from_unix_time_with_u16_days(&timestamp, cds::SubmillisPrecision::Absent)
                .unwrap();
        let len_time_stamp = cds_time.write_to_slice(buf).unwrap();
        let len_packet = base_ping_tc_simple_ctor(u14::new(0), &[])
            .write_to_slice(&mut buf[len_time_stamp..])
            .unwrap();
        (
            SpHeader::new_for_unseg_tc(u11::new(0x02), u14::new(0x34), len_packet as u16),
            len_packet + len_time_stamp,
        )
    }

    fn scheduled_tc(timestamp: UnixTime, buf: &mut [u8]) -> PusTcCreator<'_> {
        let (sph, len_app_data) = pus_tc_base(timestamp, buf);
        PusTcCreator::new_simple(
            sph,
            MessageTypeId::new(11, 4),
            &buf[..len_app_data],
            CreatorConfig::default(),
        )
    }

    fn wrong_tc_service(timestamp: UnixTime, buf: &mut [u8]) -> PusTcCreator<'_> {
        let (sph, len_app_data) = pus_tc_base(timestamp, buf);
        PusTcCreator::new_simple(
            sph,
            MessageTypeId::new(12, 4),
            &buf[..len_app_data],
            CreatorConfig::default(),
        )
    }

    fn wrong_tc_subservice(timestamp: UnixTime, buf: &mut [u8]) -> PusTcCreator<'_> {
        let (sph, len_app_data) = pus_tc_base(timestamp, buf);
        PusTcCreator::new_simple(
            sph,
            MessageTypeId::new(11, 5),
            &buf[..len_app_data],
            CreatorConfig::default(),
        )
    }

    fn double_wrapped_time_tagged_tc(timestamp: UnixTime, buf: &mut [u8]) -> PusTcCreator<'_> {
        let cds_time =
            cds::CdsTime::from_unix_time_with_u16_days(&timestamp, cds::SubmillisPrecision::Absent)
                .unwrap();
        let len_time_stamp = cds_time.write_to_slice(buf).unwrap();
        let sph = SpHeader::new_for_unseg_tc(u11::new(0x02), u14::new(0x34), 0);
        // app data should not matter, double wrapped time-tagged commands should be rejected right
        // away
        let inner_time_tagged_tc = PusTcCreator::new_simple(
            sph,
            MessageTypeId::new(11, 4),
            &[],
            CreatorConfig::default(),
        );
        let packet_len = inner_time_tagged_tc
            .write_to_slice(&mut buf[len_time_stamp..])
            .expect("writing inner time tagged tc failed");
        PusTcCreator::new_simple(
            sph,
            MessageTypeId::new(11, 4),
            &buf[..len_time_stamp + packet_len],
            CreatorConfig::default(),
        )
    }

    fn invalid_time_tagged_cmd() -> PusTcCreator<'static> {
        let sph = SpHeader::new_for_unseg_tc(u11::new(0x02), u14::new(0x34), 1);
        PusTcCreator::new_simple(
            sph,
            MessageTypeId::new(11, 4),
            &[],
            CreatorConfig::default(),
        )
    }

    fn base_ping_tc_simple_ctor(seq_count: u14, app_data: &'static [u8]) -> PusTcCreator<'static> {
        let sph = SpHeader::new_for_unseg_tc(u11::new(0x02), seq_count, 0);
        PusTcCreator::new_simple(
            sph,
            MessageTypeId::new(17, 1),
            app_data,
            CreatorConfig::default(),
        )
    }

    fn ping_tc_to_store(
        pool: &mut StaticMemoryPool,
        buf: &mut [u8],
        seq_count: u14,
        app_data: &'static [u8],
    ) -> TcInfo {
        let ping_tc = base_ping_tc_simple_ctor(seq_count, app_data);
        let ping_size = ping_tc.write_to_slice(buf).expect("writing ping TC failed");
        let first_addr = pool.add(&buf[0..ping_size]).unwrap();
        TcInfo::new(first_addr, RequestId::from_tc(&ping_tc))
    }

    #[test]
    fn test_enable_api() {
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        assert!(scheduler.is_enabled());
        scheduler.disable();
        assert!(!scheduler.is_enabled());
        scheduler.enable();
        assert!(scheduler.is_enabled());
    }

    #[test]
    fn test_reset() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));

        let mut buf: [u8; 32] = [0; 32];
        let tc_info_0 = ping_tc_to_store(&mut pool, &mut buf, u14::new(0), &[]);

        scheduler
            .insert_unwrapped_and_stored_tc(
                UnixTime::new_only_secs(100),
                TcInfo::new(tc_info_0.addr, tc_info_0.request_id),
            )
            .unwrap();

        let app_data = &[0, 1, 2];
        let tc_info_1 = ping_tc_to_store(&mut pool, &mut buf, u14::new(1), app_data);
        scheduler
            .insert_unwrapped_and_stored_tc(
                UnixTime::new_only_secs(200),
                TcInfo::new(tc_info_1.addr, tc_info_1.request_id),
            )
            .unwrap();

        let app_data = &[0, 1, 2];
        let tc_info_2 = ping_tc_to_store(&mut pool, &mut buf, u14::new(2), app_data);
        scheduler
            .insert_unwrapped_and_stored_tc(
                UnixTime::new_only_secs(300),
                TcInfo::new(tc_info_2.addr(), tc_info_2.request_id()),
            )
            .unwrap();

        assert_eq!(scheduler.num_scheduled_telecommands(), 3);
        assert!(scheduler.is_enabled());
        scheduler.reset(&mut pool).expect("deletion of TCs failed");
        assert!(!scheduler.is_enabled());
        assert_eq!(scheduler.num_scheduled_telecommands(), 0);
        assert!(!pool.has_element_at(&tc_info_0.addr()).unwrap());
        assert!(!pool.has_element_at(&tc_info_1.addr()).unwrap());
        assert!(!pool.has_element_at(&tc_info_2.addr()).unwrap());
    }

    #[test]
    fn insert_multi_with_same_time() {
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));

        scheduler
            .insert_unwrapped_and_stored_tc(
                UnixTime::new_only_secs(100),
                TcInfo::new(
                    PoolAddr::from(StaticPoolAddr {
                        pool_idx: 0,
                        packet_idx: 1,
                    }),
                    RequestId {
                        seq_count: u14::new(1),
                        apid: u11::ZERO,
                        source_id: 0,
                    },
                ),
            )
            .unwrap();

        scheduler
            .insert_unwrapped_and_stored_tc(
                UnixTime::new_only_secs(100),
                TcInfo::new(
                    PoolAddr::from(StaticPoolAddr {
                        pool_idx: 0,
                        packet_idx: 2,
                    }),
                    RequestId {
                        seq_count: u14::new(2),
                        apid: u11::new(1),
                        source_id: 5,
                    },
                ),
            )
            .unwrap();

        scheduler
            .insert_unwrapped_and_stored_tc(
                UnixTime::new_only_secs(300),
                TcInfo::new(
                    StaticPoolAddr {
                        pool_idx: 0,
                        packet_idx: 2,
                    }
                    .into(),
                    RequestId {
                        source_id: 10,
                        seq_count: u14::new(20),
                        apid: u11::new(23),
                    },
                ),
            )
            .unwrap();

        assert_eq!(scheduler.num_scheduled_telecommands(), 3);
    }

    #[test]
    fn test_time_update() {
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let time = UnixTime::new(1, 2_000_000);
        scheduler.update_time(time);
        assert_eq!(scheduler.current_time(), &time);
    }

    fn common_check(
        enabled: bool,
        store_addr: &PoolAddr,
        expected_store_addrs: Vec<PoolAddr>,
        counter: &mut usize,
    ) {
        assert!(enabled);
        assert!(expected_store_addrs.contains(store_addr));
        *counter += 1;
    }
    fn common_check_disabled(
        enabled: bool,
        store_addr: &PoolAddr,
        expected_store_addrs: Vec<PoolAddr>,
        counter: &mut usize,
    ) {
        assert!(!enabled);
        assert!(expected_store_addrs.contains(store_addr));
        *counter += 1;
    }

    #[test]
    fn test_request_id() {
        let src_id_to_set = 12;
        let apid_to_set = u11::new(0x22);
        let seq_count = u14::new(105);
        let sp_header = SpHeader::new_for_unseg_tc(apid_to_set, u14::new(105), 0);
        let mut sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(17, 1));
        sec_header.source_id = src_id_to_set;
        let ping_tc =
            PusTcCreator::new_no_app_data(sp_header, sec_header, CreatorConfig::default());
        let req_id = RequestId::from_tc(&ping_tc);
        assert_eq!(req_id.source_id(), src_id_to_set);
        assert_eq!(req_id.apid(), apid_to_set);
        assert_eq!(req_id.seq_count(), seq_count);
        assert_eq!(
            req_id.as_u64(),
            ((src_id_to_set as u64) << 32)
                | (apid_to_set.value() as u64) << 16
                | seq_count.value() as u64
        );
    }
    #[test]
    fn test_release_telecommands() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));

        let mut buf: [u8; 32] = [0; 32];
        let tc_info_0 = ping_tc_to_store(&mut pool, &mut buf, u14::ZERO, &[]);

        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(100), tc_info_0)
            .expect("insertion failed");

        let tc_info_1 = ping_tc_to_store(&mut pool, &mut buf, u14::new(1), &[]);
        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(200), tc_info_1)
            .expect("insertion failed");

        let mut i = 0;
        let mut test_closure_1 = |boolvar: bool, tc_info: &TcInfo, _tc: &[u8]| {
            common_check(boolvar, &tc_info.addr, vec![tc_info_0.addr()], &mut i);
            true
        };

        // test 1: too early, no tcs
        scheduler.update_time(UnixTime::new_only_secs(99));

        let mut tc_buf: [u8; 128] = [0; 128];
        scheduler
            .release_telecommands_with_buffer(&mut test_closure_1, &mut pool, &mut tc_buf)
            .expect("deletion failed");

        // test 2: exact time stamp of tc, releases 1 tc
        scheduler.update_time(UnixTime::new_only_secs(100));

        let mut released = scheduler
            .release_telecommands(&mut test_closure_1, &mut pool)
            .expect("deletion failed");
        assert_eq!(released, 1);
        // TC is deleted.
        assert!(!pool.has_element_at(&tc_info_0.addr()).unwrap());

        // test 3, late timestamp, release 1 overdue tc
        let mut test_closure_2 = |boolvar: bool, tc_info: &TcInfo, _tc: &[u8]| {
            common_check(boolvar, &tc_info.addr, vec![tc_info_1.addr()], &mut i);
            true
        };

        scheduler.update_time(UnixTime::new_only_secs(206));

        released = scheduler
            .release_telecommands_with_buffer(&mut test_closure_2, &mut pool, &mut tc_buf)
            .expect("deletion failed");
        assert_eq!(released, 1);
        // TC is deleted.
        assert!(!pool.has_element_at(&tc_info_1.addr()).unwrap());

        //test 4: no tcs left
        scheduler
            .release_telecommands(&mut test_closure_2, &mut pool)
            .expect("deletion failed");

        // check that 2 total tcs have been released
        assert_eq!(i, 2);
    }

    #[test]
    fn release_multi_with_same_time() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));

        let mut buf: [u8; 32] = [0; 32];
        let tc_info_0 = ping_tc_to_store(&mut pool, &mut buf, u14::ZERO, &[]);

        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(100), tc_info_0)
            .expect("insertion failed");

        let tc_info_1 = ping_tc_to_store(&mut pool, &mut buf, u14::new(1), &[]);
        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(100), tc_info_1)
            .expect("insertion failed");

        let mut i = 0;
        let mut test_closure = |boolvar: bool, store_addr: &TcInfo, _tc: &[u8]| {
            common_check(
                boolvar,
                &store_addr.addr,
                vec![tc_info_0.addr(), tc_info_1.addr()],
                &mut i,
            );
            true
        };

        // test 1: too early, no tcs
        scheduler.update_time(UnixTime::new_only_secs(99));
        let mut tc_buf: [u8; 128] = [0; 128];

        let mut released = scheduler
            .release_telecommands_with_buffer(&mut test_closure, &mut pool, &mut tc_buf)
            .expect("deletion failed");
        assert_eq!(released, 0);

        // test 2: exact time stamp of tc, releases 2 tc
        scheduler.update_time(UnixTime::new_only_secs(100));

        released = scheduler
            .release_telecommands(&mut test_closure, &mut pool)
            .expect("deletion failed");
        assert_eq!(released, 2);
        assert!(!pool.has_element_at(&tc_info_0.addr()).unwrap());
        assert!(!pool.has_element_at(&tc_info_1.addr()).unwrap());

        //test 3: no tcs left
        released = scheduler
            .release_telecommands(&mut test_closure, &mut pool)
            .expect("deletion failed");
        assert_eq!(released, 0);

        // check that 2 total tcs have been released
        assert_eq!(i, 2);
    }

    #[test]
    fn release_with_scheduler_disabled() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));

        scheduler.disable();

        let mut buf: [u8; 32] = [0; 32];
        let tc_info_0 = ping_tc_to_store(&mut pool, &mut buf, u14::ZERO, &[]);

        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(100), tc_info_0)
            .expect("insertion failed");

        let tc_info_1 = ping_tc_to_store(&mut pool, &mut buf, u14::new(1), &[]);
        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(200), tc_info_1)
            .expect("insertion failed");

        let mut i = 0;
        let mut test_closure_1 = |boolvar: bool, tc_info: &TcInfo, _tc: &[u8]| {
            common_check_disabled(boolvar, &tc_info.addr, vec![tc_info_0.addr()], &mut i);
            true
        };

        let mut tc_buf: [u8; 128] = [0; 128];

        // test 1: too early, no tcs
        scheduler.update_time(UnixTime::new_only_secs(99));

        scheduler
            .release_telecommands_with_buffer(&mut test_closure_1, &mut pool, &mut tc_buf)
            .expect("deletion failed");

        // test 2: exact time stamp of tc, releases 1 tc
        scheduler.update_time(UnixTime::new_only_secs(100));

        let mut released = scheduler
            .release_telecommands(&mut test_closure_1, &mut pool)
            .expect("deletion failed");
        assert_eq!(released, 1);
        assert!(!pool.has_element_at(&tc_info_0.addr()).unwrap());

        // test 3, late timestamp, release 1 overdue tc
        let mut test_closure_2 = |boolvar: bool, tc_info: &TcInfo, _tc: &[u8]| {
            common_check_disabled(boolvar, &tc_info.addr, vec![tc_info_1.addr()], &mut i);
            true
        };

        scheduler.update_time(UnixTime::new_only_secs(206));

        released = scheduler
            .release_telecommands(&mut test_closure_2, &mut pool)
            .expect("deletion failed");
        assert_eq!(released, 1);
        assert!(!pool.has_element_at(&tc_info_1.addr()).unwrap());

        //test 4: no tcs left
        scheduler
            .release_telecommands(&mut test_closure_2, &mut pool)
            .expect("deletion failed");

        // check that 2 total tcs have been released
        assert_eq!(i, 2);
    }

    #[test]
    fn insert_unwrapped_tc() {
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));

        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut buf: [u8; 32] = [0; 32];
        let tc_info_0 = ping_tc_to_store(&mut pool, &mut buf, u14::ZERO, &[]);

        let info = scheduler
            .insert_unwrapped_tc(
                UnixTime::new_only_secs(100),
                &buf[..pool.len_of_data(&tc_info_0.addr()).unwrap()],
                &mut pool,
            )
            .unwrap();

        assert!(pool.has_element_at(&tc_info_0.addr()).unwrap());

        let mut read_buf: [u8; 64] = [0; 64];
        pool.read(&tc_info_0.addr(), &mut read_buf).unwrap();
        let check_tc = PusTcReader::new(&read_buf).expect("incorrect Pus tc raw data");
        assert_eq!(check_tc, base_ping_tc_simple_ctor(u14::ZERO, &[]));

        assert_eq!(scheduler.num_scheduled_telecommands(), 1);

        scheduler.update_time(UnixTime::new_only_secs(101));

        let mut addr_vec = Vec::new();

        let mut i = 0;
        let mut test_closure = |boolvar: bool, tc_info: &TcInfo, _tc: &[u8]| {
            common_check(boolvar, &tc_info.addr, vec![info.addr], &mut i);
            // check that tc remains unchanged
            addr_vec.push(tc_info.addr);
            false
        };

        scheduler
            .release_telecommands(&mut test_closure, &mut pool)
            .unwrap();

        let read_len = pool.read(&addr_vec[0], &mut read_buf).unwrap();
        let check_tc = PusTcReader::new(&read_buf).expect("incorrect Pus tc raw data");
        assert_eq!(read_len, check_tc.packet_len());
        assert_eq!(check_tc, base_ping_tc_simple_ctor(u14::new(0), &[]));
    }

    #[test]
    fn insert_wrapped_tc() {
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));

        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));

        let mut buf: [u8; 32] = [0; 32];
        let tc = scheduled_tc(UnixTime::new_only_secs(100), &mut buf);

        let info = match scheduler.insert_wrapped_tc::<cds::CdsTime>(&tc, &mut pool) {
            Ok(addr) => addr,
            Err(e) => {
                panic!("unexpected error {e}");
            }
        };

        assert!(pool.has_element_at(&info.addr).unwrap());

        let read_len = pool.read(&info.addr, &mut buf).unwrap();
        let check_tc = PusTcReader::new(&buf).expect("incorrect Pus tc raw data");
        assert_eq!(read_len, check_tc.packet_len());
        assert_eq!(check_tc, base_ping_tc_simple_ctor(u14::ZERO, &[]));

        assert_eq!(scheduler.num_scheduled_telecommands(), 1);

        scheduler.update_time(UnixTime::new_only_secs(101));

        let mut addr_vec = Vec::new();

        let mut i = 0;
        let mut test_closure = |boolvar: bool, tc_info: &TcInfo, _tc: &[u8]| {
            common_check(boolvar, &tc_info.addr, vec![info.addr], &mut i);
            // check that tc remains unchanged
            addr_vec.push(tc_info.addr);
            false
        };

        let mut tc_buf: [u8; 64] = [0; 64];

        scheduler
            .release_telecommands_with_buffer(&mut test_closure, &mut pool, &mut tc_buf)
            .unwrap();

        let read_len = pool.read(&addr_vec[0], &mut buf).unwrap();
        let check_tc = PusTcReader::new(&buf).expect("incorrect PUS tc raw data");
        assert_eq!(read_len, check_tc.packet_len());
        assert_eq!(check_tc, base_ping_tc_simple_ctor(u14::new(0), &[]));
    }

    #[test]
    fn insert_wrong_service() {
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));

        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));

        let mut buf: [u8; 32] = [0; 32];
        let tc = wrong_tc_service(UnixTime::new_only_secs(100), &mut buf);

        let err = scheduler.insert_wrapped_tc::<cds::CdsTime>(&tc, &mut pool);
        assert!(err.is_err());
        let err = err.unwrap_err();
        match err {
            ScheduleError::WrongService(wrong_service) => {
                assert_eq!(wrong_service, 12);
            }
            _ => {
                panic!("unexpected error")
            }
        }
    }

    #[test]
    fn insert_wrong_subservice() {
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));

        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));

        let mut buf: [u8; 32] = [0; 32];
        let tc = wrong_tc_subservice(UnixTime::new_only_secs(100), &mut buf);

        let err = scheduler.insert_wrapped_tc::<cds::CdsTime>(&tc, &mut pool);
        assert!(err.is_err());
        let err = err.unwrap_err();
        match err {
            ScheduleError::WrongSubservice(wrong_subsrv) => {
                assert_eq!(wrong_subsrv, 5);
            }
            _ => {
                panic!("unexpected error")
            }
        }
    }

    #[test]
    fn insert_wrapped_tc_faulty_app_data() {
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let tc = invalid_time_tagged_cmd();
        let insert_res = scheduler.insert_wrapped_tc::<cds::CdsTime>(&tc, &mut pool);
        assert!(insert_res.is_err());
        let err = insert_res.unwrap_err();
        match err {
            ScheduleError::TcDataEmpty => {}
            _ => panic!("unexpected error {err}"),
        }
    }

    #[test]
    fn insert_doubly_wrapped_time_tagged_cmd() {
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut buf: [u8; 64] = [0; 64];
        let tc = double_wrapped_time_tagged_tc(UnixTime::new_only_secs(50), &mut buf);
        let insert_res = scheduler.insert_wrapped_tc::<cds::CdsTime>(&tc, &mut pool);
        assert!(insert_res.is_err());
        let err = insert_res.unwrap_err();
        match err {
            ScheduleError::NestedScheduledTc => {}
            _ => panic!("unexpected error {err}"),
        }
    }

    #[test]
    fn test_ctor_from_current() {
        let scheduler = PusSchedulerAlloc::new_with_current_init_time(Duration::from_secs(5))
            .expect("creation from current time failed");
        let current_time = scheduler.current_time;
        assert!(current_time.as_secs() > 0);
    }

    #[test]
    fn test_update_from_current() {
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        assert_eq!(scheduler.current_time.as_secs(), 0);
        scheduler
            .update_time_from_now()
            .expect("updating scheduler time from now failed");
        assert!(scheduler.current_time.as_secs() > 0);
    }

    #[test]
    fn release_time_within_time_margin() {
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));

        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));

        let mut buf: [u8; 32] = [0; 32];

        let tc = scheduled_tc(UnixTime::new_only_secs(4), &mut buf);
        let insert_res = scheduler.insert_wrapped_tc::<cds::CdsTime>(&tc, &mut pool);
        assert!(insert_res.is_err());
        let err = insert_res.unwrap_err();
        match err {
            ScheduleError::ReleaseTimeInTimeMargin {
                current_time,
                time_margin,
                release_time,
            } => {
                assert_eq!(current_time, UnixTime::new_only_secs(0));
                assert_eq!(time_margin, Duration::from_secs(5));
                assert_eq!(release_time, UnixTime::new_only_secs(4));
            }
            _ => panic!("unexepcted error {err}"),
        }
    }

    #[test]
    fn test_store_error_propagation_release() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let mut buf: [u8; 32] = [0; 32];
        let tc_info_0 = ping_tc_to_store(&mut pool, &mut buf, u14::ZERO, &[]);
        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(100), tc_info_0)
            .expect("insertion failed");

        let mut i = 0;
        let test_closure_1 = |boolvar: bool, tc_info: &TcInfo, _tc: &[u8]| {
            common_check_disabled(boolvar, &tc_info.addr, vec![tc_info_0.addr()], &mut i);
            true
        };

        // premature deletion
        pool.delete(tc_info_0.addr()).expect("deletion failed");
        // scheduler will only auto-delete if it is disabled.
        scheduler.disable();
        scheduler.update_time(UnixTime::new_only_secs(100));
        let release_res = scheduler.release_telecommands(test_closure_1, &mut pool);
        assert!(release_res.is_err());
        let err = release_res.unwrap_err();
        // TC could not even be read..
        assert_eq!(err.0, 0);
        match err.1 {
            PoolError::DataDoesNotExist(addr) => {
                assert_eq!(tc_info_0.addr(), addr);
            }
            _ => panic!("unexpected error {}", err.1),
        }
    }

    #[test]
    fn test_store_error_propagation_reset() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let mut buf: [u8; 32] = [0; 32];
        let tc_info_0 = ping_tc_to_store(&mut pool, &mut buf, u14::ZERO, &[]);
        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(100), tc_info_0)
            .expect("insertion failed");

        // premature deletion
        pool.delete(tc_info_0.addr()).expect("deletion failed");
        let reset_res = scheduler.reset(&mut pool);
        assert!(reset_res.is_err());
        let err = reset_res.unwrap_err();
        match err {
            PoolError::DataDoesNotExist(addr) => {
                assert_eq!(addr, tc_info_0.addr());
            }
            _ => panic!("unexpected error {err}"),
        }
    }

    #[test]
    fn test_delete_by_req_id_simple_retrieve_addr() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let mut buf: [u8; 32] = [0; 32];
        let tc_info_0 = ping_tc_to_store(&mut pool, &mut buf, u14::ZERO, &[]);
        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(100), tc_info_0)
            .expect("inserting tc failed");
        assert_eq!(scheduler.num_scheduled_telecommands(), 1);
        let addr = scheduler
            .delete_by_request_id(&tc_info_0.request_id())
            .unwrap();
        assert!(pool.has_element_at(&tc_info_0.addr()).unwrap());
        assert_eq!(tc_info_0.addr(), addr);
        assert_eq!(scheduler.num_scheduled_telecommands(), 0);
    }

    #[test]
    fn test_delete_by_req_id_simple_delete_all() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let mut buf: [u8; 32] = [0; 32];
        let tc_info_0 = ping_tc_to_store(&mut pool, &mut buf, u14::ZERO, &[]);
        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(100), tc_info_0)
            .expect("inserting tc failed");
        assert_eq!(scheduler.num_scheduled_telecommands(), 1);
        let del_res =
            scheduler.delete_by_request_id_and_from_pool(&tc_info_0.request_id(), &mut pool);
        assert!(del_res.is_ok());
        assert!(del_res.unwrap());
        assert!(!pool.has_element_at(&tc_info_0.addr()).unwrap());
        assert_eq!(scheduler.num_scheduled_telecommands(), 0);
    }

    #[test]
    fn test_delete_by_req_id_complex() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let mut buf: [u8; 32] = [0; 32];
        let tc_info_0 = ping_tc_to_store(&mut pool, &mut buf, u14::ZERO, &[]);
        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(100), tc_info_0)
            .expect("inserting tc failed");
        let tc_info_1 = ping_tc_to_store(&mut pool, &mut buf, u14::new(1), &[]);
        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(100), tc_info_1)
            .expect("inserting tc failed");
        let tc_info_2 = ping_tc_to_store(&mut pool, &mut buf, u14::new(2), &[]);
        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(100), tc_info_2)
            .expect("inserting tc failed");
        assert_eq!(scheduler.num_scheduled_telecommands(), 3);

        // Delete first packet
        let addr_0 = scheduler.delete_by_request_id(&tc_info_0.request_id());
        assert!(addr_0.is_some());
        assert_eq!(addr_0.unwrap(), tc_info_0.addr());
        assert!(pool.has_element_at(&tc_info_0.addr()).unwrap());
        assert_eq!(scheduler.num_scheduled_telecommands(), 2);

        // Delete next packet
        let del_res =
            scheduler.delete_by_request_id_and_from_pool(&tc_info_2.request_id(), &mut pool);
        assert!(del_res.is_ok());
        assert!(del_res.unwrap());
        assert!(!pool.has_element_at(&tc_info_2.addr()).unwrap());
        assert_eq!(scheduler.num_scheduled_telecommands(), 1);

        // Delete last packet
        let addr_1 =
            scheduler.delete_by_request_id_and_from_pool(&tc_info_1.request_id(), &mut pool);
        assert!(addr_1.is_ok());
        assert!(addr_1.unwrap());
        assert!(!pool.has_element_at(&tc_info_1.addr()).unwrap());
        assert_eq!(scheduler.num_scheduled_telecommands(), 0);
    }

    #[test]
    fn insert_full_store_test() {
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));

        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(1, 64)],
            false,
        ));

        let mut buf: [u8; 32] = [0; 32];
        // Store is full after this.
        pool.add(&[0, 1, 2]).unwrap();
        let tc = scheduled_tc(UnixTime::new_only_secs(100), &mut buf);

        let insert_res = scheduler.insert_wrapped_tc::<cds::CdsTime>(&tc, &mut pool);
        assert!(insert_res.is_err());
        let err = insert_res.unwrap_err();
        match err {
            ScheduleError::Pool(e) => match e {
                PoolError::StoreFull(_) => {}
                _ => panic!("unexpected store error {e}"),
            },
            _ => panic!("unexpected error {err}"),
        }
    }

    fn insert_command_with_release_time(
        pool: &mut StaticMemoryPool,
        scheduler: &mut PusSchedulerAlloc,
        seq_count: u14,
        release_secs: u64,
    ) -> TcInfo {
        let mut buf: [u8; 32] = [0; 32];
        let tc_info = ping_tc_to_store(pool, &mut buf, seq_count, &[]);

        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(release_secs as i64), tc_info)
            .expect("inserting tc failed");
        tc_info
    }

    #[test]
    fn test_time_window_retrieval_select_all() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let tc_info_0 = insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 50);
        let tc_info_1 = insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 100);
        assert_eq!(scheduler.num_scheduled_telecommands(), 2);
        let check_range = |range: Range<UnixTime, Vec<TcInfo>>| {
            let mut tcs_in_range = 0;
            for (idx, time_bucket) in range.enumerate() {
                tcs_in_range += 1;
                if idx == 0 {
                    assert_eq!(*time_bucket.0, UnixTime::new_only_secs(50));
                    assert_eq!(time_bucket.1.len(), 1);
                    assert_eq!(time_bucket.1[0].request_id, tc_info_0.request_id);
                } else if idx == 1 {
                    assert_eq!(*time_bucket.0, UnixTime::new_only_secs(100));
                    assert_eq!(time_bucket.1.len(), 1);
                    assert_eq!(time_bucket.1[0].request_id, tc_info_1.request_id);
                }
            }
            assert_eq!(tcs_in_range, 2);
        };
        let range = scheduler.retrieve_all();
        check_range(range);
        let range = scheduler.retrieve_by_time_filter(TimeWindow::<cds::CdsTime>::new_select_all());
        check_range(range);
    }

    #[test]
    fn test_time_window_retrieval_select_from_stamp() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let _ = insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 50);
        let tc_info_1 = insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 100);
        let tc_info_2 = insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 150);
        let start_stamp = cds::CdsTime::from_unix_time_with_u16_days(
            &UnixTime::new_only_secs(100),
            cds::SubmillisPrecision::Absent,
        )
        .expect("creating start stamp failed");
        let time_window = TimeWindow::new_from_time(&start_stamp);
        assert_eq!(scheduler.num_scheduled_telecommands(), 3);

        let range = scheduler.retrieve_by_time_filter(time_window);
        let mut tcs_in_range = 0;
        for (idx, time_bucket) in range.enumerate() {
            tcs_in_range += 1;
            if idx == 0 {
                assert_eq!(*time_bucket.0, UnixTime::new_only_secs(100));
                assert_eq!(time_bucket.1.len(), 1);
                assert_eq!(time_bucket.1[0].request_id, tc_info_1.request_id());
            } else if idx == 1 {
                assert_eq!(*time_bucket.0, UnixTime::new_only_secs(150));
                assert_eq!(time_bucket.1.len(), 1);
                assert_eq!(time_bucket.1[0].request_id, tc_info_2.request_id());
            }
        }
        assert_eq!(tcs_in_range, 2);
    }

    #[test]
    fn test_time_window_retrieval_select_to_time() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let tc_info_0 = insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 50);
        let tc_info_1 = insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 100);
        let _ = insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 150);
        assert_eq!(scheduler.num_scheduled_telecommands(), 3);

        let end_stamp = cds::CdsTime::from_unix_time_with_u16_days(
            &UnixTime::new_only_secs(100),
            cds::SubmillisPrecision::Absent,
        )
        .expect("creating start stamp failed");
        let time_window = TimeWindow::new_to_time(&end_stamp);
        let range = scheduler.retrieve_by_time_filter(time_window);
        let mut tcs_in_range = 0;
        for (idx, time_bucket) in range.enumerate() {
            tcs_in_range += 1;
            if idx == 0 {
                assert_eq!(*time_bucket.0, UnixTime::new_only_secs(50));
                assert_eq!(time_bucket.1.len(), 1);
                assert_eq!(time_bucket.1[0].request_id, tc_info_0.request_id());
            } else if idx == 1 {
                assert_eq!(*time_bucket.0, UnixTime::new_only_secs(100));
                assert_eq!(time_bucket.1.len(), 1);
                assert_eq!(time_bucket.1[0].request_id, tc_info_1.request_id());
            }
        }
        assert_eq!(tcs_in_range, 2);
    }

    #[test]
    fn test_time_window_retrieval_select_from_time_to_time() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let _ = insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 50);
        let tc_info_1 = insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 100);
        let tc_info_2 = insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 150);
        let _ = insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 200);
        assert_eq!(scheduler.num_scheduled_telecommands(), 4);

        let start_stamp = cds::CdsTime::from_unix_time_with_u16_days(
            &UnixTime::new_only_secs(100),
            cds::SubmillisPrecision::Absent,
        )
        .expect("creating start stamp failed");
        let end_stamp = cds::CdsTime::from_unix_time_with_u16_days(
            &UnixTime::new_only_secs(150),
            cds::SubmillisPrecision::Absent,
        )
        .expect("creating end stamp failed");
        let time_window = TimeWindow::new_from_time_to_time(&start_stamp, &end_stamp);
        let range = scheduler.retrieve_by_time_filter(time_window);
        let mut tcs_in_range = 0;
        for (idx, time_bucket) in range.enumerate() {
            tcs_in_range += 1;
            if idx == 0 {
                assert_eq!(*time_bucket.0, UnixTime::new_only_secs(100));
                assert_eq!(time_bucket.1.len(), 1);
                assert_eq!(time_bucket.1[0].request_id, tc_info_1.request_id());
            } else if idx == 1 {
                assert_eq!(*time_bucket.0, UnixTime::new_only_secs(150));
                assert_eq!(time_bucket.1.len(), 1);
                assert_eq!(time_bucket.1[0].request_id, tc_info_2.request_id());
            }
        }
        assert_eq!(tcs_in_range, 2);
    }

    #[test]
    fn test_deletion_all() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 50);
        insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 100);
        assert_eq!(scheduler.num_scheduled_telecommands(), 2);
        let del_res = scheduler.delete_all(&mut pool);
        assert!(del_res.is_ok());
        assert_eq!(del_res.unwrap(), 2);
        assert_eq!(scheduler.num_scheduled_telecommands(), 0);
        // Contrary to reset, this does not disable the scheduler.
        assert!(scheduler.is_enabled());

        insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 50);
        insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 100);
        assert_eq!(scheduler.num_scheduled_telecommands(), 2);
        let del_res = scheduler
            .delete_by_time_filter(TimeWindow::<cds::CdsTime>::new_select_all(), &mut pool);
        assert!(del_res.is_ok());
        assert_eq!(del_res.unwrap(), 2);
        assert_eq!(scheduler.num_scheduled_telecommands(), 0);
        // Contrary to reset, this does not disable the scheduler.
        assert!(scheduler.is_enabled());
    }

    #[test]
    fn test_deletion_from_start_time() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 50);
        let cmd_0_to_delete =
            insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 100);
        let cmd_1_to_delete =
            insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 150);
        assert_eq!(scheduler.num_scheduled_telecommands(), 3);
        let start_stamp = cds::CdsTime::from_unix_time_with_u16_days(
            &UnixTime::new_only_secs(100),
            cds::SubmillisPrecision::Absent,
        )
        .expect("creating start stamp failed");
        let time_window = TimeWindow::new_from_time(&start_stamp);
        let del_res = scheduler.delete_by_time_filter(time_window, &mut pool);
        assert!(del_res.is_ok());
        assert_eq!(del_res.unwrap(), 2);
        assert_eq!(scheduler.num_scheduled_telecommands(), 1);
        assert!(!pool.has_element_at(&cmd_0_to_delete.addr()).unwrap());
        assert!(!pool.has_element_at(&cmd_1_to_delete.addr()).unwrap());
    }

    #[test]
    fn test_deletion_to_end_time() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let cmd_0_to_delete =
            insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 50);
        let cmd_1_to_delete =
            insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 100);
        insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 150);
        assert_eq!(scheduler.num_scheduled_telecommands(), 3);

        let end_stamp = cds::CdsTime::from_unix_time_with_u16_days(
            &UnixTime::new_only_secs(100),
            cds::SubmillisPrecision::Absent,
        )
        .expect("creating start stamp failed");
        let time_window = TimeWindow::new_to_time(&end_stamp);
        let del_res = scheduler.delete_by_time_filter(time_window, &mut pool);
        assert!(del_res.is_ok());
        assert_eq!(del_res.unwrap(), 2);
        assert_eq!(scheduler.num_scheduled_telecommands(), 1);
        assert!(!pool.has_element_at(&cmd_0_to_delete.addr()).unwrap());
        assert!(!pool.has_element_at(&cmd_1_to_delete.addr()).unwrap());
    }

    #[test]
    fn test_deletion_from_start_time_to_end_time() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));
        let cmd_out_of_range_0 =
            insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 50);
        let cmd_0_to_delete =
            insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 100);
        let cmd_1_to_delete =
            insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 150);
        let cmd_out_of_range_1 =
            insert_command_with_release_time(&mut pool, &mut scheduler, u14::ZERO, 200);
        assert_eq!(scheduler.num_scheduled_telecommands(), 4);

        let start_stamp = cds::CdsTime::from_unix_time_with_u16_days(
            &UnixTime::new_only_secs(100),
            cds::SubmillisPrecision::Absent,
        )
        .expect("creating start stamp failed");
        let end_stamp = cds::CdsTime::from_unix_time_with_u16_days(
            &UnixTime::new_only_secs(150),
            cds::SubmillisPrecision::Absent,
        )
        .expect("creating end stamp failed");
        let time_window = TimeWindow::new_from_time_to_time(&start_stamp, &end_stamp);
        let del_res = scheduler.delete_by_time_filter(time_window, &mut pool);
        assert!(del_res.is_ok());
        assert_eq!(del_res.unwrap(), 2);
        assert_eq!(scheduler.num_scheduled_telecommands(), 2);
        assert!(pool.has_element_at(&cmd_out_of_range_0.addr()).unwrap());
        assert!(!pool.has_element_at(&cmd_0_to_delete.addr()).unwrap());
        assert!(!pool.has_element_at(&cmd_1_to_delete.addr()).unwrap());
        assert!(pool.has_element_at(&cmd_out_of_range_1.addr()).unwrap());
    }

    #[test]
    fn test_release_without_deletion() {
        let mut pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(10, 32), (5, 64)],
            false,
        ));
        let mut scheduler =
            PusSchedulerAlloc::new(UnixTime::new_only_secs(0), Duration::from_secs(5));

        let mut buf: [u8; 32] = [0; 32];
        let tc_info_0 = ping_tc_to_store(&mut pool, &mut buf, u14::ZERO, &[]);

        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(100), tc_info_0)
            .expect("insertion failed");

        let tc_info_1 = ping_tc_to_store(&mut pool, &mut buf, u14::new(1), &[]);
        scheduler
            .insert_unwrapped_and_stored_tc(UnixTime::new_only_secs(200), tc_info_1)
            .expect("insertion failed");

        let mut i = 0;
        let mut test_closure_1 = |boolvar: bool, tc_info: &TcInfo, _tc: &[u8]| {
            common_check(
                boolvar,
                &tc_info.addr,
                vec![tc_info_0.addr(), tc_info_1.addr()],
                &mut i,
            );
        };

        scheduler.update_time(UnixTime::new_only_secs(205));

        let mut tc_buf: [u8; 64] = [0; 64];
        let tc_info_vec = scheduler
            .release_telecommands_no_deletion(&mut test_closure_1, &pool, &mut tc_buf)
            .expect("deletion failed");
        assert_eq!(tc_info_vec[0], tc_info_0);
        assert_eq!(tc_info_vec[1], tc_info_1);
    }

    #[test]
    fn test_generic_insert_app_data_test() {
        let time_writer = cds::CdsTime::new_with_u16_days(1, 1);
        let sph = SpHeader::new(
            PacketId::new(PacketType::Tc, true, u11::new(0x002)),
            PacketSequenceControl::new(SequenceFlags::Unsegmented, u14::new(5)),
            0,
        );
        let sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(17, 1));
        let ping_tc = PusTcCreator::new_no_app_data(sph, sec_header, CreatorConfig::default());
        let mut buf: [u8; 64] = [0; 64];
        let result = generate_insert_telecommand_app_data(&mut buf, &time_writer, &ping_tc);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 2 + 7 + ping_tc.len_written());
        let n = u16::from_be_bytes(buf[0..2].try_into().unwrap());
        assert_eq!(n, 1);
        let time_reader = cds::CdsTime::from_bytes_with_u16_days(&buf[2..2 + 7]).unwrap();
        assert_eq!(time_reader, time_writer);
        let pus_tc_reader = PusTcReader::new(&buf[9..]).unwrap();
        assert_eq!(pus_tc_reader, ping_tc);
    }

    #[test]
    fn test_generic_insert_app_data_test_byte_conv_error() {
        let time_writer = cds::CdsTime::new_with_u16_days(1, 1);
        let sph = SpHeader::new(
            PacketId::new(PacketType::Tc, true, u11::new(0x002)),
            PacketSequenceControl::new(SequenceFlags::Unsegmented, u14::new(5)),
            0,
        );
        let sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(17, 1));
        let ping_tc = PusTcCreator::new_no_app_data(sph, sec_header, CreatorConfig::default());
        let mut buf: [u8; 16] = [0; 16];
        let result = generate_insert_telecommand_app_data(&mut buf, &time_writer, &ping_tc);
        assert!(result.is_err());
        let error = result.unwrap_err();
        if let ScheduleError::ByteConversionError(ByteConversionError::ToSliceTooSmall {
            found,
            expected,
        }) = error
        {
            assert_eq!(found, 16);
            assert_eq!(
                expected,
                2 + time_writer.len_written() + ping_tc.len_written()
            );
        } else {
            panic!("unexpected error {error}")
        }
    }

    #[test]
    fn test_generic_insert_app_data_test_as_vec() {
        let time_writer = cds::CdsTime::new_with_u16_days(1, 1);
        let sph = SpHeader::new(
            PacketId::new(PacketType::Tc, true, u11::new(0x002)),
            PacketSequenceControl::new(SequenceFlags::Unsegmented, u14::new(5)),
            0,
        );
        let sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(17, 1));
        let ping_tc = PusTcCreator::new_no_app_data(sph, sec_header, CreatorConfig::default());
        let mut buf: [u8; 64] = [0; 64];
        generate_insert_telecommand_app_data(&mut buf, &time_writer, &ping_tc).unwrap();
        let vec = generate_insert_telecommand_app_data_as_vec(&time_writer, &ping_tc)
            .expect("vec generation failed");
        assert_eq!(&buf[..vec.len()], vec);
    }
}
