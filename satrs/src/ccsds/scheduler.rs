//! # CCSDS Telecommand Scheduler.
#![deny(missing_docs)]
use core::{hash::Hash, time::Duration};

#[cfg(feature = "alloc")]
pub use alloc_mod::*;
use spacepackets::{
    CcsdsPacketIdAndPsc,
    time::{TimestampError, UnixTime},
};

/// Generic CCSDS scheduling errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ScheduleError {
    /// The release time is within the time-margin added on top of the current time.
    /// The first parameter is the current time, the second one the time margin, and the third one
    /// the release time.
    #[error("release time in margin")]
    ReleaseTimeInTimeMargin {
        /// Current time.
        current_time: UnixTime,
        /// Configured time margin.
        time_margin: Duration,
        /// Release time.
        release_time: UnixTime,
    },
    /// Nested time-tagged commands are not allowed.
    #[error("nested scheduled tc")]
    NestedScheduledTc,
    /// TC data is empty.
    #[error("tc data empty")]
    TcDataEmpty,
    /// Scheduler is full, packet number limit reached.
    #[error("scheduler is full, packet number limit reached")]
    PacketLimitReached,
    /// Scheduler is full, numver of bytes limit reached.
    #[error("scheduler is full, number of bytes limit reached")]
    ByteLimitReached,
    /// Timestamp error.
    #[error("timestamp error: {0}")]
    TimestampError(#[from] TimestampError),
}

/// Packet ID used for identifying scheduled packets.
///
/// Right now, this ID can be determined from the packet without requiring external input
/// or custom data fields in the CCSDS space pacekt.
#[derive(Debug, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CcsdsSchedulePacketId {
    /// Base ID.
    pub base: CcsdsPacketIdAndPsc,
    /// Optional checksum of the packet.
    pub crc16: Option<u16>,
}

impl CcsdsSchedulePacketId {
    /// Create a new CCSDS scheduling packet ID.
    pub const fn new(base: CcsdsPacketIdAndPsc, checksum: Option<u16>) -> Self {
        Self {
            base,
            crc16: checksum,
        }
    }
}

impl Hash for CcsdsSchedulePacketId {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.base.hash(state);
        self.crc16.hash(state);
    }
}

/// Modules requiring [alloc] support.
#[cfg(feature = "alloc")]
pub mod alloc_mod {
    use core::time::Duration;
    #[cfg(feature = "std")]
    use std::time::SystemTimeError;

    use alloc::collections::btree_map;
    use spacepackets::{CcsdsPacketIdAndPsc, CcsdsPacketReader, time::UnixTime};

    use crate::ccsds::scheduler::CcsdsSchedulePacketId;

    /// The scheduler can be configured to have bounds for both the number of packets
    /// and the total number of bytes used by scheduled packets.
    ///
    /// This can be used to avoid memory exhaustion in systems with limited resources or under
    /// heavy workloads.
    #[derive(Default, Debug)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    #[cfg_attr(feature = "defmt", derive(defmt::Format))]
    pub struct Limits {
        /// Maximum number of scheduled packets.
        pub packets: Option<usize>,
        /// Maximum total number of bytes used by scheduled packets.
        pub bytes: Option<usize>,
    }

    impl Limits {
        /// Create new limits for the CCSDS scheduler.
        pub const fn new(packets: Option<usize>, bytes: Option<usize>) -> Self {
            Self { packets, bytes }
        }

        /// Check if no limits are set.
        pub fn has_no_limits(&self) -> bool {
            self.packets.is_none() && self.bytes.is_none()
        }
    }

    /// Fill count of the scheduler.
    #[derive(Default, Debug)]
    #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
    #[cfg_attr(feature = "defmt", derive(defmt::Format))]
    pub struct FillCount {
        /// Number of scheduled packets.
        pub packets: usize,
        /// Total number of bytes used by scheduled packets.
        pub bytes: usize,
    }

    /// Simple CCSDS scheduler implementation.
    ///
    /// Relies of [alloc] support but limits the number of scheduled packets.
    #[derive(Debug)]
    pub struct CcsdsScheduler {
        tc_map: alloc::collections::BTreeMap<
            UnixTime,
            alloc::vec::Vec<(CcsdsSchedulePacketId, alloc::vec::Vec<u8>)>,
        >,
        limits: Limits,
        pub(crate) current_time: UnixTime,
        time_margin: Duration,
    }

    impl CcsdsScheduler {
        /// Create a new CCSDS scheduler.
        pub fn new(current_time: UnixTime, limits: Limits, time_margin: Duration) -> Self {
            Self {
                tc_map: alloc::collections::BTreeMap::new(),
                limits,
                current_time,
                time_margin,
            }
        }

        /// Like [Self::new], but sets the `init_current_time` parameter to the current system time.
        #[cfg(feature = "std")]
        pub fn new_with_current_init_time(
            limits: Limits,
            time_margin: Duration,
        ) -> Result<Self, SystemTimeError> {
            Ok(Self::new(UnixTime::now()?, limits, time_margin))
        }

        /// Current fill count: number of scheduled packets and total number of bytes.
        ///
        /// The first returned value is the number of scheduled packets, the second one is the
        /// byte count.
        pub fn current_fill_count(&self) -> FillCount {
            let mut fill_count = FillCount::default();
            for value in self.tc_map.values() {
                for (_, raw_scheduled_tc) in value {
                    fill_count.packets += 1;
                    fill_count.bytes += raw_scheduled_tc.len();
                }
            }
            fill_count
        }

        /// Current number of scheduled entries.
        pub fn num_of_entries(&self) -> usize {
            self.current_fill_count().packets
        }

        /// Update the current time.
        #[inline]
        pub fn update_time(&mut self, current_time: UnixTime) {
            self.current_time = current_time;
        }

        /// Current time.
        #[inline]
        pub fn current_time(&self) -> &UnixTime {
            &self.current_time
        }

        fn common_check(
            &mut self,
            release_time: UnixTime,
            packet_size: usize,
        ) -> Result<(), super::ScheduleError> {
            if !self.limits.has_no_limits() {
                let fill_count = self.current_fill_count();
                if let Some(max_bytes) = self.limits.bytes
                    && fill_count.bytes + packet_size > max_bytes
                {
                    return Err(super::ScheduleError::ByteLimitReached);
                }
                if let Some(max_packets) = self.limits.packets
                    && fill_count.packets + 1 > max_packets
                {
                    return Err(super::ScheduleError::PacketLimitReached);
                }
            }
            if release_time < self.current_time + self.time_margin {
                return Err(super::ScheduleError::ReleaseTimeInTimeMargin {
                    current_time: self.current_time,
                    time_margin: self.time_margin,
                    release_time,
                });
            }
            Ok(())
        }

        /// Insert a telecommand using an existing [CcsdsPacketReader].
        pub fn insert_telecommand_with_reader(
            &mut self,
            reader: &CcsdsPacketReader,
            release_time: UnixTime,
        ) -> Result<(), super::ScheduleError> {
            self.common_check(release_time, reader.packet_len())?;
            let base_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(reader);
            let checksum = reader.checksum();
            let packet_id_scheduling = CcsdsSchedulePacketId {
                base: base_id,
                crc16: checksum,
            };
            self.insert_telecommand(packet_id_scheduling, reader.raw_data(), release_time)?;

            Ok(())
        }

        /// Insert a raw telecommand, assuming the user has already extracted the
        /// [CcsdsSchedulePacketId]
        pub fn insert_telecommand(
            &mut self,
            packet_id_scheduling: CcsdsSchedulePacketId,
            raw_packet: &[u8],
            release_time: UnixTime,
        ) -> Result<(), super::ScheduleError> {
            self.common_check(release_time, raw_packet.len())?;
            match self.tc_map.entry(release_time) {
                btree_map::Entry::Vacant(e) => {
                    e.insert(alloc::vec![(packet_id_scheduling, raw_packet.to_vec())]);
                }
                btree_map::Entry::Occupied(mut v) => {
                    v.get_mut()
                        .push((packet_id_scheduling, raw_packet.to_vec()));
                }
            }
            Ok(())
        }

        /// Release all telecommands which should be released based on the current time.
        pub fn release_telecommands<R: FnMut(&CcsdsSchedulePacketId, &[u8])>(
            &mut self,
            mut releaser: R,
        ) {
            let tcs_to_release = self.telecommands_to_release();
            for tc_group in tcs_to_release {
                for (packet_id, raw_tc) in tc_group.1 {
                    releaser(packet_id, raw_tc);
                }
            }
            self.tc_map.retain(|k, _| k > &self.current_time);
        }

        /// Retrieve all telecommands which should be released based on the current time.
        pub fn telecommands_to_release(
            &self,
        ) -> btree_map::Range<
            '_,
            UnixTime,
            alloc::vec::Vec<(CcsdsSchedulePacketId, alloc::vec::Vec<u8>)>,
        > {
            self.tc_map.range(..=self.current_time)
        }

        /// Delete scheduled telecommand by their packet ID.
        ///
        /// Returns whether any telecommand was deleted. This function might have to be called
        /// multiple times if multiple identical CCSDS packet IDs are possible.
        pub fn delete_by_id(&mut self, packet_id: &CcsdsSchedulePacketId) -> bool {
            let mut was_removed = false;
            self.tc_map.retain(|_, v| {
                let len_before = v.len();
                v.retain(|(stored_id, _)| stored_id != packet_id);
                let has_remaining = !v.is_empty();
                if v.len() < len_before {
                    was_removed = true;
                }
                has_remaining
            });
            was_removed
        }

        /// Delete all telecommands scheduled in a time window.
        ///
        /// The range includes the start time but excludes the end time. Returns whether any
        /// telecommands were deleted.
        pub fn delete_time_window(&mut self, start_time: UnixTime, end_time: UnixTime) -> bool {
            let len_before = self.tc_map.len();
            self.tc_map.retain(|k, _| k < &start_time || k >= &end_time);
            self.tc_map.len() < len_before
        }

        /// Delete all scheduled telecommands scheduled after or at a given time.
        ///
        /// Returns whether any telecommands were deleted.
        pub fn delete_starting_at(&mut self, start_time: UnixTime) -> bool {
            let len_before = self.tc_map.len();
            self.tc_map.retain(|k, _| k < &start_time);
            self.tc_map.len() < len_before
        }

        /// Delete all scheduled telecommands scheduled before but not equal to a given time.
        ///
        /// Returns whether any telecommands were deleted.
        pub fn delete_before(&mut self, end_time: UnixTime) -> bool {
            let len_before = self.tc_map.len();
            self.tc_map.retain(|k, _| k >= &end_time);
            self.tc_map.len() < len_before
        }

        /// Completely clear the scheduler.
        pub fn clear(&mut self) {
            self.tc_map.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use arbitrary_int::{traits::Integer, u11, u14};
    use spacepackets::{
        CcsdsPacketCreatorOwned, CcsdsPacketReader, ChecksumType, SpacePacketHeader,
    };

    use super::*;

    fn test_tc(app_data: &[u8], seq_count: u14) -> CcsdsPacketCreatorOwned {
        CcsdsPacketCreatorOwned::new(
            SpacePacketHeader::new_for_tc(
                u11::new(0x1),
                spacepackets::SequenceFlags::Unsegmented,
                seq_count,
                0,
            ),
            spacepackets::PacketType::Tc,
            app_data,
            Some(ChecksumType::WithCrc16),
        )
        .unwrap()
    }

    #[test]
    fn test_basic() {
        let unix_time = UnixTime::new(0, 0);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(Some(100), Some(1024)),
            Duration::from_millis(5000),
        );
        assert_eq!(scheduler.current_fill_count().packets, 0);
        assert_eq!(scheduler.current_fill_count().bytes, 0);
        assert_eq!(scheduler.num_of_entries(), 0);
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            0
        );
        assert_eq!(scheduler.current_time(), &unix_time);
        scheduler.release_telecommands(|_, _| {
            panic!("should not be called");
        });
    }

    #[test]
    fn test_mutable_closure() {
        let unix_time = UnixTime::new(0, 0);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(Some(100), Some(1024)),
            Duration::from_millis(5000),
        );
        let mut some_flag = false;
        // We should be able to manipulate the boolean inside the closure.
        scheduler.release_telecommands(|_, _| {
            some_flag = true;
        });
    }

    #[test]
    fn test_clear() {
        let unix_time = UnixTime::new(0, 0);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(Some(100), Some(1024)),
            Duration::from_millis(1000),
        );
        let test_tc = test_tc(&[1, 2, 3], u14::ZERO);
        let test_tc_raw = test_tc.to_vec();
        let reader = CcsdsPacketReader::new(&test_tc_raw, Some(ChecksumType::WithCrc16)).unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader, UnixTime::new(2, 0))
            .unwrap();
        assert_eq!(scheduler.current_fill_count().packets, 1);
        assert_eq!(scheduler.current_fill_count().bytes, test_tc_raw.len());
        assert_eq!(scheduler.num_of_entries(), 1);
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            0
        );
        scheduler.clear();
        assert_eq!(scheduler.current_fill_count().packets, 0);
        assert_eq!(scheduler.current_fill_count().bytes, 0);
        assert_eq!(scheduler.num_of_entries(), 0);
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            0
        );
    }

    #[test]
    fn insert_and_release_one() {
        let unix_time = UnixTime::new(0, 0);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(Some(100), Some(1024)),
            Duration::from_millis(1000),
        );
        let test_tc_0 = test_tc(&[1, 2, 3], u14::ZERO);
        let tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&test_tc_0);
        let test_tc_raw = test_tc_0.to_vec();
        let reader = CcsdsPacketReader::new(&test_tc_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let checksum = reader.checksum();
        scheduler
            .insert_telecommand_with_reader(&reader, UnixTime::new(2, 0))
            .unwrap();
        assert_eq!(scheduler.current_fill_count().packets, 1);
        assert_eq!(scheduler.current_fill_count().bytes, test_tc_raw.len());
        assert_eq!(scheduler.num_of_entries(), 1);
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            0
        );
        scheduler.release_telecommands(|_, _| {
            panic!("should not be called");
        });
        scheduler.update_time(UnixTime::new(3, 0));
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            1
        );
        scheduler.release_telecommands(|tc_id_scheduled, tc_raw| {
            assert_eq!(tc_id, tc_id_scheduled.base);
            assert_eq!(checksum, tc_id_scheduled.crc16);
            assert_eq!(tc_raw, test_tc_raw);
        });
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            0
        );
    }

    #[test]
    fn insert_and_release_multi_0() {
        let unix_time = UnixTime::new(0, 0);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(Some(100), Some(1024)),
            Duration::from_millis(1000),
        );
        let test_tc_0 = test_tc(&[42], u14::ZERO);
        let test_tc_1 = test_tc(&[1, 2, 3], u14::new(1));
        let tc_id_0 = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&test_tc_0);
        let tc_id_1 = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&test_tc_1);
        let test_tc_0_raw = test_tc_0.to_vec();
        let test_tc_1_raw = test_tc_1.to_vec();
        let reader_0 =
            CcsdsPacketReader::new(&test_tc_0_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let reader_1 =
            CcsdsPacketReader::new(&test_tc_1_raw, Some(ChecksumType::WithCrc16)).unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_0, UnixTime::new(2, 0))
            .unwrap();
        scheduler
            .insert_telecommand(
                CcsdsSchedulePacketId::new(tc_id_1, reader_1.checksum()),
                &test_tc_1_raw,
                UnixTime::new(5, 0),
            )
            .unwrap();
        assert_eq!(scheduler.current_fill_count().packets, 2);
        assert_eq!(
            scheduler.current_fill_count().bytes,
            test_tc_0_raw.len() + test_tc_1_raw.len()
        );
        assert_eq!(scheduler.num_of_entries(), 2);
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            0
        );
        scheduler.release_telecommands(|_, _| {
            panic!("should not be called");
        });

        // Release first TC.
        scheduler.update_time(UnixTime::new(3, 0));
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            1
        );
        scheduler.release_telecommands(|tc_id_scheduled, tc_raw| {
            assert_eq!(tc_id_0, tc_id_scheduled.base);
            assert_eq!(reader_0.checksum(), tc_id_scheduled.crc16);
            assert_eq!(tc_raw, test_tc_0_raw);
        });
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            0
        );
        assert_eq!(scheduler.current_fill_count().packets, 1);
        assert_eq!(scheduler.current_fill_count().bytes, test_tc_1_raw.len());
        assert_eq!(scheduler.num_of_entries(), 1);

        // Release second TC.
        scheduler.update_time(UnixTime::new(6, 0));
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            1
        );
        scheduler.release_telecommands(|tc_id_scheduled, tc_raw| {
            assert_eq!(tc_id_1, tc_id_scheduled.base);
            assert_eq!(reader_1.checksum(), tc_id_scheduled.crc16);
            assert_eq!(tc_raw, test_tc_1_raw);
        });
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            0
        );
        assert_eq!(scheduler.current_fill_count().packets, 0);
        assert_eq!(scheduler.current_fill_count().bytes, 0);
        assert_eq!(scheduler.num_of_entries(), 0);
    }

    #[test]
    fn insert_and_release_multi_1() {
        let unix_time = UnixTime::new(0, 0);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(Some(100), Some(1024)),
            Duration::from_millis(1000),
        );
        let test_tc_0 = test_tc(&[42], u14::ZERO);
        let test_tc_1 = test_tc(&[1, 2, 3], u14::new(1));
        let tc_id_0 = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&test_tc_0);
        let tc_id_1 = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&test_tc_1);
        let test_tc_0_raw = test_tc_0.to_vec();
        let test_tc_1_raw = test_tc_1.to_vec();
        let reader_0 =
            CcsdsPacketReader::new(&test_tc_0_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let reader_1 =
            CcsdsPacketReader::new(&test_tc_1_raw, Some(ChecksumType::WithCrc16)).unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_0, UnixTime::new(2, 0))
            .unwrap();
        scheduler
            .insert_telecommand(
                CcsdsSchedulePacketId::new(tc_id_1, reader_1.checksum()),
                &test_tc_1_raw,
                UnixTime::new(5, 0),
            )
            .unwrap();
        assert_eq!(scheduler.current_fill_count().packets, 2);
        assert_eq!(
            scheduler.current_fill_count().bytes,
            test_tc_0_raw.len() + test_tc_1_raw.len()
        );
        assert_eq!(scheduler.num_of_entries(), 2);
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            0
        );
        scheduler.release_telecommands(|_, _| {
            panic!("should not be called");
        });

        // Release first TC.
        scheduler.update_time(UnixTime::new(6, 0));
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            2
        );
        let mut index = 0;
        scheduler.release_telecommands(|tc_id_scheduled, tc_raw| {
            if index == 0 {
                assert_eq!(tc_id_0, tc_id_scheduled.base);
                assert_eq!(reader_0.checksum(), tc_id_scheduled.crc16);
                assert_eq!(tc_raw, test_tc_0_raw);
            } else {
                assert_eq!(tc_id_1, tc_id_scheduled.base);
                assert_eq!(reader_1.checksum(), tc_id_scheduled.crc16);
                assert_eq!(tc_raw, test_tc_1_raw);
            }
            index += 1;
        });
        assert_eq!(
            scheduler
                .telecommands_to_release()
                .collect::<alloc::vec::Vec<_>>()
                .len(),
            0
        );
        assert_eq!(scheduler.current_fill_count().packets, 0);
        assert_eq!(scheduler.current_fill_count().bytes, 0);
        assert_eq!(scheduler.num_of_entries(), 0);
    }

    #[test]
    fn test_packet_limit_reached() {
        let unix_time = UnixTime::new(0, 0);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(Some(3), None),
            Duration::from_millis(1000),
        );
        let test_tc_0 = test_tc(&[42], u14::ZERO);
        let test_tc_0_raw = test_tc_0.to_vec();
        let reader = CcsdsPacketReader::new(&test_tc_0_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&test_tc_0);
        scheduler
            .insert_telecommand_with_reader(&reader, UnixTime::new(2, 0))
            .unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader, UnixTime::new(2, 0))
            .unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader, UnixTime::new(2, 0))
            .unwrap();
        assert_eq!(scheduler.current_fill_count().packets, 3);
        assert_eq!(
            scheduler.insert_telecommand_with_reader(&reader, UnixTime::new(2, 0)),
            Err(ScheduleError::PacketLimitReached)
        );
        assert_eq!(
            scheduler.insert_telecommand(
                CcsdsSchedulePacketId::new(tc_id, reader.checksum()),
                &test_tc_0_raw,
                UnixTime::new(2, 0)
            ),
            Err(ScheduleError::PacketLimitReached)
        );
    }

    #[test]
    fn test_byte_limit_reached() {
        let unix_time = UnixTime::new(0, 0);
        let test_tc_0 = test_tc(&[42], u14::ZERO);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(None, Some(test_tc_0.len_written() * 3)),
            Duration::from_millis(1000),
        );
        let test_tc_0_raw = test_tc_0.to_vec();
        let reader = CcsdsPacketReader::new(&test_tc_0_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&test_tc_0);
        scheduler
            .insert_telecommand_with_reader(&reader, UnixTime::new(2, 0))
            .unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader, UnixTime::new(2, 0))
            .unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader, UnixTime::new(2, 0))
            .unwrap();
        assert_eq!(scheduler.current_fill_count().packets, 3);
        assert_eq!(
            scheduler.insert_telecommand_with_reader(&reader, UnixTime::new(2, 0)),
            Err(ScheduleError::ByteLimitReached)
        );
        assert_eq!(
            scheduler.insert_telecommand(
                CcsdsSchedulePacketId::new(tc_id, reader.checksum()),
                &test_tc_0_raw,
                UnixTime::new(2, 0)
            ),
            Err(ScheduleError::ByteLimitReached)
        );
    }

    #[test]
    fn test_deletion_by_id() {
        let unix_time = UnixTime::new(0, 0);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(Some(100), Some(1024)),
            Duration::from_millis(1000),
        );
        let test_tc = test_tc(&[1, 2, 3], u14::ZERO);
        let tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&test_tc);
        let test_tc_raw = test_tc.to_vec();
        let reader = CcsdsPacketReader::new(&test_tc_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let checksum = reader.checksum();
        let id = CcsdsSchedulePacketId::new(tc_id, checksum);
        scheduler
            .insert_telecommand_with_reader(&reader, UnixTime::new(2, 0))
            .unwrap();
        scheduler.delete_by_id(&id);
        assert_eq!(scheduler.current_fill_count().packets, 0);
        assert_eq!(scheduler.current_fill_count().bytes, 0);
    }

    #[test]
    fn test_deletion_by_window_0() {
        let unix_time = UnixTime::new(0, 0);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(Some(100), Some(1024)),
            Duration::from_millis(1000),
        );
        let test_tc_0 = test_tc(&[42], u14::ZERO);
        let test_tc_1 = test_tc(&[1, 2, 3], u14::new(1));
        let test_tc_2 = test_tc(&[1, 2, 3], u14::new(2));
        let test_tc_0_raw = test_tc_0.to_vec();
        let test_tc_1_raw = test_tc_1.to_vec();
        let test_tc_2_raw = test_tc_2.to_vec();
        let reader_0 =
            CcsdsPacketReader::new(&test_tc_0_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let reader_1 =
            CcsdsPacketReader::new(&test_tc_1_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let reader_2 =
            CcsdsPacketReader::new(&test_tc_2_raw, Some(ChecksumType::WithCrc16)).unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_0, UnixTime::new(2, 0))
            .unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_1, UnixTime::new(5, 0))
            .unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_2, UnixTime::new(7, 0))
            .unwrap();
        let deleted = scheduler.delete_time_window(UnixTime::new(3, 0), UnixTime::new(6, 0));
        assert!(deleted);
        assert_eq!(scheduler.current_fill_count().packets, 2);
        assert_eq!(
            scheduler.current_fill_count().bytes,
            test_tc_0_raw.len() + test_tc_2_raw.len()
        );
        scheduler.update_time(UnixTime::new(10, 0));
        let mut index = 0;
        scheduler.release_telecommands(|_id, packet| {
            if index == 0 {
                assert_eq!(packet, test_tc_0_raw);
            } else {
                assert_eq!(packet, test_tc_2_raw);
            }
            index += 1;
        });
        assert_eq!(scheduler.current_fill_count().packets, 0);
        assert_eq!(scheduler.current_fill_count().bytes, 0);
    }

    #[test]
    fn test_deletion_by_window_1() {
        let unix_time = UnixTime::new(0, 0);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(Some(100), Some(1024)),
            Duration::from_millis(1000),
        );
        let test_tc_0 = test_tc(&[42], u14::ZERO);
        let test_tc_1 = test_tc(&[1, 2, 3], u14::new(1));
        let test_tc_2 = test_tc(&[1, 2, 3], u14::new(2));
        let test_tc_0_raw = test_tc_0.to_vec();
        let test_tc_1_raw = test_tc_1.to_vec();
        let test_tc_2_raw = test_tc_2.to_vec();
        let reader_0 =
            CcsdsPacketReader::new(&test_tc_0_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let reader_1 =
            CcsdsPacketReader::new(&test_tc_1_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let reader_2 =
            CcsdsPacketReader::new(&test_tc_2_raw, Some(ChecksumType::WithCrc16)).unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_0, UnixTime::new(2, 0))
            .unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_1, UnixTime::new(5, 0))
            .unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_2, UnixTime::new(7, 0))
            .unwrap();
        // This only deletes the first 2 TCs.
        let deleted = scheduler.delete_time_window(UnixTime::new(2, 0), UnixTime::new(7, 0));
        assert!(deleted);
        assert_eq!(scheduler.current_fill_count().packets, 1);
        assert_eq!(scheduler.current_fill_count().bytes, test_tc_2_raw.len());
        scheduler.update_time(UnixTime::new(10, 0));
        scheduler.release_telecommands(|_id, packet| {
            assert_eq!(packet, test_tc_2_raw);
        });
    }

    #[test]
    fn test_deletion_from_start() {
        let unix_time = UnixTime::new(0, 0);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(Some(100), Some(1024)),
            Duration::from_millis(1000),
        );
        let test_tc_0 = test_tc(&[42], u14::ZERO);
        let test_tc_1 = test_tc(&[1, 2, 3], u14::new(1));
        let test_tc_2 = test_tc(&[1, 2, 3], u14::new(2));
        let test_tc_0_raw = test_tc_0.to_vec();
        let test_tc_1_raw = test_tc_1.to_vec();
        let test_tc_2_raw = test_tc_2.to_vec();
        let reader_0 =
            CcsdsPacketReader::new(&test_tc_0_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let reader_1 =
            CcsdsPacketReader::new(&test_tc_1_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let reader_2 =
            CcsdsPacketReader::new(&test_tc_2_raw, Some(ChecksumType::WithCrc16)).unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_0, UnixTime::new(2, 0))
            .unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_1, UnixTime::new(5, 0))
            .unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_2, UnixTime::new(7, 0))
            .unwrap();
        // This only deletes the first 2 TCs.
        let deleted = scheduler.delete_starting_at(UnixTime::new(5, 0));
        assert!(deleted);
        assert_eq!(scheduler.current_fill_count().packets, 1);
        assert_eq!(scheduler.current_fill_count().bytes, test_tc_0_raw.len());
        scheduler.update_time(UnixTime::new(10, 0));
        scheduler.release_telecommands(|_id, packet| {
            assert_eq!(packet, test_tc_0_raw);
        });
    }

    #[test]
    fn test_deletion_until_end() {
        let unix_time = UnixTime::new(0, 0);
        let mut scheduler = CcsdsScheduler::new(
            unix_time,
            Limits::new(Some(100), Some(1024)),
            Duration::from_millis(1000),
        );
        let test_tc_0 = test_tc(&[42], u14::ZERO);
        let test_tc_1 = test_tc(&[1, 2, 3], u14::new(1));
        let test_tc_2 = test_tc(&[1, 2, 3], u14::new(2));
        let test_tc_0_raw = test_tc_0.to_vec();
        let test_tc_1_raw = test_tc_1.to_vec();
        let test_tc_2_raw = test_tc_2.to_vec();
        let reader_0 =
            CcsdsPacketReader::new(&test_tc_0_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let reader_1 =
            CcsdsPacketReader::new(&test_tc_1_raw, Some(ChecksumType::WithCrc16)).unwrap();
        let reader_2 =
            CcsdsPacketReader::new(&test_tc_2_raw, Some(ChecksumType::WithCrc16)).unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_0, UnixTime::new(2, 0))
            .unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_1, UnixTime::new(5, 0))
            .unwrap();
        scheduler
            .insert_telecommand_with_reader(&reader_2, UnixTime::new(7, 0))
            .unwrap();
        // This only deletes the first 2 TCs.
        let deleted = scheduler.delete_before(UnixTime::new(7, 0));
        assert!(deleted);
        assert_eq!(scheduler.current_fill_count().packets, 1);
        assert_eq!(scheduler.current_fill_count().bytes, test_tc_2_raw.len());
        scheduler.update_time(UnixTime::new(10, 0));
        scheduler.release_telecommands(|_id, packet| {
            assert_eq!(packet, test_tc_2_raw);
        });
    }
}
