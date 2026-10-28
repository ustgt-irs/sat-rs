//! # PUS Service 1 Verification Module
//!
//! This module allows packaging and sending PUS Service 1 packets. It is conforming to section
//! 8 of the PUS standard ECSS-E-ST-70-41C.
//!
//! The core object to report TC verification progress is the [VerificationReporter]. It exposes
//! an API which uses type-state programming to avoid calling the verification steps in
//! an invalid order.
//!
//! # Examples
//!
//! Basic single-threaded example where a full success sequence for a given ping telecommand is
//! executed. Note that the verification part could also be done in a separate thread.
//!
//! ```
//! use std::sync::{Arc, mpsc, RwLock};
//! use std::time::Duration;
//! use satrs::pool::{PoolProviderWithGuards, StaticMemoryPool, StaticPoolConfig};
//! use satrs::legacy::pus::verification::{
//!     VerificationReportingProvider, VerificationReporterConfig, VerificationReporter
//! };
//! use satrs::tmtc::{SharedStaticMemoryPool, PacketSenderWithSharedPool};
//! use satrs::request::UniqueApidTargetId;
//! use spacepackets::ecss::PusPacket;
//! use spacepackets::SpHeader;
//! use spacepackets::ecss::tc::{MessageTypeId, PusTcCreator, PusTcSecondaryHeader, CreatorConfig};
//! use spacepackets::ecss::tm::PusTmReader;
//! use arbitrary_int::{u11, u21};
//!
//! const EMPTY_STAMP: [u8; 7] = [0; 7];
//! const TEST_APID: u11 = u11::new(0x02);
//! const TEST_COMPONENT_ID: UniqueApidTargetId = UniqueApidTargetId::new(TEST_APID, u21::new(0x05));
//!
//! let pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(
//!     vec![(10, 32), (10, 64), (10, 128), (10, 1024)], false
//! );
//! let tm_pool = StaticMemoryPool::new(pool_cfg.clone());
//! let shared_tm_pool = SharedStaticMemoryPool::new(RwLock::new(tm_pool));
//! let (verif_tx, verif_rx) = mpsc::sync_channel(10);
//! let sender = PacketSenderWithSharedPool::new_with_shared_packet_pool(verif_tx, &shared_tm_pool);
//! let cfg = VerificationReporterConfig::new(TEST_APID, 1, 2, 8);
//! let mut  reporter = VerificationReporter::new(TEST_COMPONENT_ID.id(), &cfg);
//!
//! let tc_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(17, 1));
//! let pus_tc_0 = PusTcCreator::new_no_app_data(
//!     SpHeader::new_from_apid(TEST_APID),
//!     tc_header,
//!     CreatorConfig::default()
//! );
//! let init_token = reporter.start_verification(&pus_tc_0);
//!
//! // Complete success sequence for a telecommand
//! let accepted_token = reporter.acceptance_success(&sender, init_token, &EMPTY_STAMP).unwrap();
//! let started_token = reporter.start_success(&sender, accepted_token, &EMPTY_STAMP).unwrap();
//! reporter.completion_success(&sender, started_token, &EMPTY_STAMP).unwrap();
//!
//! // Verify it arrives correctly on receiver end
//! let mut tm_buf: [u8; 1024] = [0; 1024];
//! let mut packet_idx = 0;
//! while packet_idx < 3 {
//!     let tm_in_store = verif_rx.recv_timeout(Duration::from_millis(10)).unwrap();
//!     let tm_len;
//!     {
//!         let mut rg = shared_tm_pool.write().expect("Error locking shared pool");
//!         let store_guard = rg.read_with_guard(tm_in_store.store_addr);
//!         tm_len = store_guard.read(&mut tm_buf).expect("Error reading TM slice");
//!     }
//!     let pus_tm = PusTmReader::new(&tm_buf[0..tm_len], 7).expect("Error reading verification TM");
//!     if packet_idx == 0 {
//!         assert_eq!(pus_tm.message_subtype_id(), 1);
//!     } else if packet_idx == 1 {
//!         assert_eq!(pus_tm.message_subtype_id(), 3);
//!     } else if packet_idx == 2 {
//!         assert_eq!(pus_tm.message_subtype_id(), 7);
//!     }
//!     packet_idx += 1;
//! }
//! ```
//!
//! The [integration test](https://github.com/ustgt-irs/sat-rs/blob/main/satrs/tests/pus_verification.rs)
//! for the verification module contains examples how this module could be used in a more complex
//! context involving multiple threads
use super::{EcssTmSender, EcssTmtcError, source_buffer_large_enough};
use crate::params::{Params, WritableToBeBytes};
use arbitrary_int::{u3, u11, u14};
use core::fmt::{Debug, Display, Formatter};
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::mem::size_of;
#[cfg(feature = "alloc")]
use delegate::delegate;
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use spacepackets::SpHeader;
use spacepackets::ecss::tc::IsPusTelecommand;
use spacepackets::ecss::tm::{PusTmCreator, PusTmSecondaryHeader};
use spacepackets::ecss::{CreatorConfig, EcssEnumeration, MessageTypeId};
use spacepackets::{ByteConversionError, CcsdsPacket, PacketId, PacketSequenceControl};

pub use spacepackets::ecss::verification::*;
pub use spacepackets::seq_count::SequenceCounterSimple;

#[cfg(feature = "alloc")]
pub use alloc_mod::*;

use crate::ComponentId;
use crate::request::Apid;

/// This is a request identifier as specified in 5.4.11.2 c. of the PUS standard.
///
/// This field equivalent to the first two bytes of the CCSDS space packet header.
/// This version of the request ID is supplied in the verification reports and does not contain
/// the source ID.
#[derive(Debug, Eq, Copy, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct RequestId {
    version_number: u3,
    packet_id: PacketId,
    psc: PacketSequenceControl,
}

impl Display for RequestId {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:#08x}", self.raw())
    }
}

impl Hash for RequestId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw().hash(state);
    }
}

// Implement manually to satisfy derive_hash_xor_eq lint
impl PartialEq for RequestId {
    fn eq(&self, other: &Self) -> bool {
        self.version_number == other.version_number
            && self.packet_id == other.packet_id
            && self.psc == other.psc
    }
}

impl RequestId {
    pub const SIZE_AS_BYTES: usize = size_of::<u32>();

    /// This allows extracting the request ID from a given PUS telecommand.
    pub fn new(tc: &(impl CcsdsPacket + IsPusTelecommand)) -> Self {
        Self::new_from_ccsds_tc(tc)
    }

    /// Extract the request ID from a CCSDS TC packet.
    pub fn new_from_ccsds_tc(tc: &impl CcsdsPacket) -> Self {
        RequestId {
            version_number: tc.ccsds_version(),
            packet_id: tc.packet_id(),
            psc: tc.psc(),
        }
    }

    pub fn raw(&self) -> u32 {
        ((self.version_number.value() as u32) << 29)
            | ((self.packet_id.raw() as u32) << 16)
            | self.psc.raw() as u32
    }

    #[inline]
    pub const fn packet_id(&self) -> PacketId {
        self.packet_id
    }

    #[inline]
    pub const fn packet_seq_ctrl(&self) -> PacketSequenceControl {
        self.psc
    }

    pub fn to_bytes(&self, buf: &mut [u8]) {
        let raw = self.raw();
        buf.copy_from_slice(raw.to_be_bytes().as_slice());
    }

    pub fn from_bytes(buf: &[u8]) -> Option<Self> {
        if buf.len() < 4 {
            return None;
        }
        let raw = u32::from_be_bytes(buf[0..Self::SIZE_AS_BYTES].try_into().unwrap());
        Some(Self {
            version_number: u3::new(((raw >> 29) & 0b111) as u8),
            packet_id: PacketId::from(((raw >> 16) & 0xffff) as u16),
            psc: PacketSequenceControl::from((raw & 0xffff) as u16),
        })
    }
}

impl From<u32> for RequestId {
    fn from(value: u32) -> Self {
        Self {
            version_number: u3::new(((value >> 29) & 0b111) as u8),
            packet_id: PacketId::from(((value >> 16) & 0xffff) as u16),
            psc: PacketSequenceControl::from((value & 0xffff) as u16),
        }
    }
}

impl From<RequestId> for u32 {
    fn from(value: RequestId) -> Self {
        value.raw()
    }
}

/// If a verification operation fails, the passed token will be returned as well. This allows
/// re-trying the operation at a later point.
#[derive(Debug, Clone)]
pub struct VerificationOrSendErrorWithToken<T>(pub EcssTmtcError, pub VerificationToken<T>);

#[derive(Debug, Clone)]
pub struct VerificationErrorWithToken<T>(pub EcssTmtcError, pub VerificationToken<T>);

impl<T> From<VerificationErrorWithToken<T>> for VerificationOrSendErrorWithToken<T> {
    fn from(value: VerificationErrorWithToken<T>) -> Self {
        VerificationOrSendErrorWithToken(value.0, value.1)
    }
}

/// Support token to allow type-state programming. This prevents calling the verification
/// steps in an invalid order.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct VerificationToken<STATE> {
    state: PhantomData<STATE>,
    request_id: RequestId,
}

impl<STATE> VerificationToken<STATE> {
    pub fn new(req_id: RequestId) -> VerificationToken<TcStateNone> {
        VerificationToken {
            state: PhantomData,
            request_id: req_id,
        }
    }

    pub fn request_id(&self) -> RequestId {
        self.request_id
    }
}

impl VerificationToken<TcStateAccepted> {
    /// Create a verification token with the accepted state. This can be useful for test purposes.
    /// For general purposes, it is recommended to use the API exposed by verification handlers.
    pub fn new_accepted_state(req_id: RequestId) -> VerificationToken<TcStateAccepted> {
        VerificationToken {
            state: PhantomData,
            request_id: req_id,
        }
    }
}

impl VerificationToken<TcStateStarted> {
    /// Create a verification token with the started state. This can be useful for test purposes.
    /// For general purposes, it is recommended to use the API exposed by verification handlers.
    pub fn new_started_state(req_id: RequestId) -> VerificationToken<TcStateStarted> {
        VerificationToken {
            state: PhantomData,
            request_id: req_id,
        }
    }
}

pub trait WasAtLeastAccepted {}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TcStateNone;
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TcStateAccepted;
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TcStateStarted;
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TcStateCompleted;

impl WasAtLeastAccepted for TcStateAccepted {}
impl WasAtLeastAccepted for TcStateStarted {}
impl WasAtLeastAccepted for TcStateCompleted {}

/// Token wrapper to model all possible verification tokens. These tokens are used to
/// enforce the correct order for the verification steps when doing verification reporting.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum TcStateToken {
    None(VerificationToken<TcStateNone>),
    Accepted(VerificationToken<TcStateAccepted>),
    Started(VerificationToken<TcStateStarted>),
    Completed(VerificationToken<TcStateCompleted>),
}

impl TcStateToken {
    pub fn request_id(&self) -> RequestId {
        match self {
            TcStateToken::None(token) => token.request_id(),
            TcStateToken::Accepted(token) => token.request_id(),
            TcStateToken::Started(token) => token.request_id(),
            TcStateToken::Completed(token) => token.request_id(),
        }
    }
}

impl From<VerificationToken<TcStateNone>> for TcStateToken {
    fn from(t: VerificationToken<TcStateNone>) -> Self {
        TcStateToken::None(t)
    }
}

impl TryFrom<TcStateToken> for VerificationToken<TcStateAccepted> {
    type Error = ();

    fn try_from(value: TcStateToken) -> Result<Self, Self::Error> {
        if let TcStateToken::Accepted(token) = value {
            Ok(token)
        } else {
            Err(())
        }
    }
}

impl TryFrom<TcStateToken> for VerificationToken<TcStateStarted> {
    type Error = ();

    fn try_from(value: TcStateToken) -> Result<Self, Self::Error> {
        if let TcStateToken::Started(token) = value {
            Ok(token)
        } else {
            Err(())
        }
    }
}

impl From<VerificationToken<TcStateAccepted>> for TcStateToken {
    fn from(t: VerificationToken<TcStateAccepted>) -> Self {
        TcStateToken::Accepted(t)
    }
}

impl From<VerificationToken<TcStateStarted>> for TcStateToken {
    fn from(t: VerificationToken<TcStateStarted>) -> Self {
        TcStateToken::Started(t)
    }
}

impl From<VerificationToken<TcStateCompleted>> for TcStateToken {
    fn from(t: VerificationToken<TcStateCompleted>) -> Self {
        TcStateToken::Completed(t)
    }
}

/// Composite helper struct to pass failure parameters to the [VerificationReporter]
pub struct FailParams<'stamp, 'fargs> {
    pub time_stamp: &'stamp [u8],
    pub failure_code: &'fargs dyn EcssEnumeration,
    pub failure_data: &'fargs [u8],
}

impl<'stamp, 'fargs> FailParams<'stamp, 'fargs> {
    pub fn new(
        time_stamp: &'stamp [u8],
        failure_code: &'fargs dyn EcssEnumeration,
        failure_data: &'fargs [u8],
    ) -> Self {
        Self {
            time_stamp,
            failure_code,
            failure_data,
        }
    }

    pub fn new_no_fail_data(
        time_stamp: &'stamp [u8],
        failure_code: &'fargs impl EcssEnumeration,
    ) -> Self {
        Self::new(time_stamp, failure_code, &[])
    }
}

/// Composite helper struct to pass step failure parameters to the [VerificationReporter]
pub struct FailParamsWithStep<'stamp, 'fargs> {
    pub common: FailParams<'stamp, 'fargs>,
    pub step: &'fargs dyn EcssEnumeration,
}

impl<'stamp, 'fargs> FailParamsWithStep<'stamp, 'fargs> {
    pub fn new(
        time_stamp: &'stamp [u8],
        step: &'fargs impl EcssEnumeration,
        failure_code: &'fargs dyn EcssEnumeration,
        failure_data: &'fargs [u8],
    ) -> Self {
        Self {
            common: FailParams::new(time_stamp, failure_code, failure_data),
            step,
        }
    }
}

/// This is a generic trait implemented by an object which can perform the ECSS PUS 1 verification
/// process according to PUS standard ECSS-E-ST-70-41C.
///
/// This trait allows using different message queue backends for the verification reporting process
/// or to swap the actual reporter with a test reporter for unit tests.
/// For general purposes, the [VerificationReporter] should be sufficient.
pub trait VerificationReportingProvider {
    /// It is generally assumed that the reporting provider is owned by some PUS service with
    /// a unique ID.
    fn owner_id(&self) -> ComponentId;

    fn set_apid(&mut self, apid: Apid);
    fn apid(&self) -> Apid;

    fn start_verification(
        &self,
        pus_tc: &(impl CcsdsPacket + IsPusTelecommand),
    ) -> VerificationToken<TcStateNone>;

    fn acceptance_success(
        &self,
        sender: &(impl EcssTmSender + ?Sized),
        token: VerificationToken<TcStateNone>,
        time_stamp: &[u8],
    ) -> Result<VerificationToken<TcStateAccepted>, EcssTmtcError>;

    fn acceptance_failure(
        &self,
        sender: &(impl EcssTmSender + ?Sized),
        token: VerificationToken<TcStateNone>,
        params: FailParams,
    ) -> Result<(), EcssTmtcError>;

    fn start_success(
        &self,
        sender: &(impl EcssTmSender + ?Sized),
        token: VerificationToken<TcStateAccepted>,
        time_stamp: &[u8],
    ) -> Result<VerificationToken<TcStateStarted>, EcssTmtcError>;

    fn start_failure(
        &self,
        sender: &(impl EcssTmSender + ?Sized),
        token: VerificationToken<TcStateAccepted>,
        params: FailParams,
    ) -> Result<(), EcssTmtcError>;

    fn step_success(
        &self,
        sender: &(impl EcssTmSender + ?Sized),
        token: &VerificationToken<TcStateStarted>,
        time_stamp: &[u8],
        step: impl EcssEnumeration,
    ) -> Result<(), EcssTmtcError>;

    fn step_failure(
        &self,
        sender: &(impl EcssTmSender + ?Sized),
        token: VerificationToken<TcStateStarted>,
        params: FailParamsWithStep,
    ) -> Result<(), EcssTmtcError>;

    fn completion_success<TcState: WasAtLeastAccepted + Copy>(
        &self,
        sender: &(impl EcssTmSender + ?Sized),
        token: VerificationToken<TcState>,
        time_stamp: &[u8],
    ) -> Result<(), EcssTmtcError>;

    fn completion_failure<TcState: WasAtLeastAccepted + Copy>(
        &self,
        sender: &(impl EcssTmSender + ?Sized),
        token: VerificationToken<TcState>,
        params: FailParams,
    ) -> Result<(), EcssTmtcError>;
}

/// Low level object which generates ECSS PUS 1 verification packets to verify the various steps
/// of telecommand handling as specified in the PUS standard.
///
/// This is the core component which can be used without [`alloc`] support. Please note that
/// the buffer passed to the API exposes by this struct will be used to serialize the source data.
/// This buffer may not be re-used to serialize the whole telemetry because that would overwrite
/// the source data itself.
#[derive(Debug, Clone)]
pub struct VerificationReportCreator {
    pub dest_id: u16,
    apid: u11,
}

impl VerificationReportCreator {
    pub fn new(apid: u11) -> Self {
        Self { apid, dest_id: 0 }
    }

    pub fn set_apid(&mut self, apid: u11) {
        self.apid = apid;
    }

    pub fn apid(&self) -> u11 {
        self.apid
    }

    pub fn dest_id(&self) -> u16 {
        self.dest_id
    }

    pub fn set_dest_id(&mut self, dest_id: u16) {
        self.dest_id = dest_id;
    }

    pub fn read_request_id(&self, pus_tc: &(impl CcsdsPacket + IsPusTelecommand)) -> RequestId {
        RequestId::new(pus_tc)
    }

    fn success_verification_no_step<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        subservice: u8,
        request_id: &RequestId,
        seq_count: u14,
        msg_count: u16,
        time_stamp: &'time [u8],
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        let tm_creator = self.create_pus_verif_success_tm(
            src_data_buf,
            subservice,
            seq_count,
            msg_count,
            request_id,
            time_stamp,
            None::<&dyn EcssEnumeration>,
        )?;
        Ok(tm_creator)
    }

    // Internal helper function, too many arguments is acceptable for this case.
    #[allow(clippy::too_many_arguments)]
    fn failure_verification_no_step<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        subservice: u8,
        request_id: &RequestId,
        seq_count: u14,
        msg_count: u16,
        step: Option<&(impl EcssEnumeration + ?Sized)>,
        params: &FailParams<'time, '_>,
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        let tm_creator = self.create_pus_verif_fail_tm(
            src_data_buf,
            subservice,
            seq_count,
            msg_count,
            request_id,
            step,
            params,
        )?;
        Ok(tm_creator)
    }

    /// Package a PUS TM\[1, 1\] packet, see 8.1.2.1 of the PUS standard.
    pub fn acceptance_success<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        request_id: &RequestId,
        seq_count: u14,
        msg_count: u16,
        time_stamp: &'time [u8],
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        let tm_creator = self.success_verification_no_step(
            src_data_buf,
            MessageSubtypeId::TmAcceptanceSuccess.into(),
            request_id,
            seq_count,
            msg_count,
            time_stamp,
        )?;
        Ok(tm_creator)
    }

    /// Package a PUS TM\[1, 2\] packet, see 8.1.2.2 of the PUS standard.
    pub fn acceptance_failure<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        request_id: &RequestId,
        seq_count: u14,
        msg_count: u16,
        params: FailParams<'time, '_>,
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        self.failure_verification_no_step(
            src_data_buf,
            MessageSubtypeId::TmAcceptanceFailure.into(),
            request_id,
            seq_count,
            msg_count,
            None::<&dyn EcssEnumeration>,
            &params,
        )
    }

    /// Package and send a PUS TM\[1, 3\] packet, see 8.1.2.3 of the PUS standard.
    ///
    /// Requires a token previously acquired by calling [Self::acceptance_success].
    pub fn start_success<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        request_id: &RequestId,
        seq_count: u14,
        msg_count: u16,
        time_stamp: &'time [u8],
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        let tm_creator = self.success_verification_no_step(
            src_data_buf,
            MessageSubtypeId::TmStartSuccess.into(),
            request_id,
            seq_count,
            msg_count,
            time_stamp,
        )?;
        Ok(tm_creator)
    }

    /// Package and send a PUS TM\[1, 4\] packet, see 8.1.2.4 of the PUS standard.
    ///
    /// Requires a token previously acquired by calling [Self::acceptance_success]. It consumes
    /// the token because verification handling is done.
    pub fn start_failure<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        request_id: &RequestId,
        seq_count: u14,
        msg_count: u16,
        params: FailParams<'time, '_>,
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        self.failure_verification_no_step(
            src_data_buf,
            MessageSubtypeId::TmStartFailure.into(),
            request_id,
            seq_count,
            msg_count,
            None::<&dyn EcssEnumeration>,
            &params,
        )
    }

    /// Package and send a PUS TM\[1, 5\] packet, see 8.1.2.5 of the PUS standard.
    ///
    /// Requires a token previously acquired by calling [Self::start_success].
    pub fn step_success<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        request_id: &RequestId,
        seq_count: u14,
        msg_count: u16,
        time_stamp: &'time [u8],
        step: impl EcssEnumeration,
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        self.create_pus_verif_success_tm(
            src_data_buf,
            MessageSubtypeId::TmStepSuccess.into(),
            seq_count,
            msg_count,
            request_id,
            time_stamp,
            Some(&step),
        )
    }

    /// Package and send a PUS TM\[1, 6\] packet, see 8.1.2.6 of the PUS standard.
    ///
    /// Requires a token previously acquired by calling [Self::start_success]. It consumes the
    /// token because verification handling is done.
    pub fn step_failure<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        token: VerificationToken<TcStateStarted>,
        seq_count: u14,
        msg_count: u16,
        params: FailParamsWithStep<'time, '_>,
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        self.create_pus_verif_fail_tm(
            src_data_buf,
            MessageSubtypeId::TmStepFailure.into(),
            seq_count,
            msg_count,
            &token.request_id(),
            Some(params.step),
            &params.common,
        )
    }

    /// Package and send a PUS TM\[1, 7\] packet, see 8.1.2.7 of the PUS standard.
    ///
    /// Requires a token previously acquired by calling [Self::start_success]. It consumes the
    /// token because verification handling is done.
    pub fn completion_success<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        request_id: &RequestId,
        seq_counter: u14,
        msg_counter: u16,
        time_stamp: &'time [u8],
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        self.success_verification_no_step(
            src_data_buf,
            MessageSubtypeId::TmCompletionSuccess.into(),
            request_id,
            seq_counter,
            msg_counter,
            time_stamp,
        )
    }

    /// Package and send a PUS TM\[1, 8\] packet, see 8.1.2.8 of the PUS standard.
    ///
    /// Requires a token previously acquired by calling [Self::start_success]. It consumes the
    /// token because verification handling is done.
    pub fn completion_failure<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        request_id: &RequestId,
        seq_count: u14,
        msg_count: u16,
        params: FailParams<'time, '_>,
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        self.failure_verification_no_step(
            src_data_buf,
            MessageSubtypeId::TmCompletionFailure.into(),
            request_id,
            seq_count,
            msg_count,
            None::<&dyn EcssEnumeration>,
            &params,
        )
    }

    // Internal helper function, too many arguments is acceptable for this case.
    #[allow(clippy::too_many_arguments)]
    fn create_pus_verif_success_tm<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        subservice: u8,
        seq_count: u14,
        msg_counter: u16,
        req_id: &RequestId,
        time_stamp: &'time [u8],
        step: Option<&(impl EcssEnumeration + ?Sized)>,
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        let mut source_data_len = size_of::<u32>();
        if let Some(step) = step {
            source_data_len += step.size();
        }
        source_buffer_large_enough(src_data_buf.len(), source_data_len)?;
        let mut idx = 0;
        req_id.to_bytes(&mut src_data_buf[0..RequestId::SIZE_AS_BYTES]);
        idx += RequestId::SIZE_AS_BYTES;
        if let Some(step) = step {
            // Size check was done beforehand
            step.write_to_slice_be(&mut src_data_buf[idx..idx + step.size()])
                .unwrap();
        }
        let sp_header = SpHeader::new_for_unseg_tm(self.apid(), seq_count, 0);
        Ok(self.create_pus_verif_tm_base(
            src_data_buf,
            subservice,
            msg_counter,
            sp_header,
            time_stamp,
            source_data_len,
        ))
    }

    // Internal helper function, too many arguments is acceptable for this case.
    #[allow(clippy::too_many_arguments)]
    fn create_pus_verif_fail_tm<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        subservice: u8,
        seq_count: u14,
        msg_counter: u16,
        req_id: &RequestId,
        step: Option<&(impl EcssEnumeration + ?Sized)>,
        params: &FailParams<'time, '_>,
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        let mut idx = 0;
        let mut source_data_len = RequestId::SIZE_AS_BYTES + params.failure_code.size();
        if let Some(step) = step {
            source_data_len += step.size();
        }
        source_data_len += params.failure_data.len();
        source_buffer_large_enough(src_data_buf.len(), source_data_len)?;
        req_id.to_bytes(&mut src_data_buf[0..RequestId::SIZE_AS_BYTES]);
        idx += RequestId::SIZE_AS_BYTES;
        if let Some(step) = step {
            // Size check done beforehand
            step.write_to_slice_be(&mut src_data_buf[idx..idx + step.size()])
                .unwrap();
            idx += step.size();
        }
        params
            .failure_code
            .write_to_slice_be(&mut src_data_buf[idx..idx + params.failure_code.size()])?;
        idx += params.failure_code.size();
        src_data_buf[idx..idx + params.failure_data.len()].copy_from_slice(params.failure_data);
        let sp_header = SpHeader::new_for_unseg_tm(self.apid(), seq_count, 0);
        Ok(self.create_pus_verif_tm_base(
            src_data_buf,
            subservice,
            msg_counter,
            sp_header,
            params.time_stamp,
            source_data_len,
        ))
    }

    fn create_pus_verif_tm_base<'time, 'src_data>(
        &self,
        src_data_buf: &'src_data mut [u8],
        subservice: u8,
        msg_counter: u16,
        sp_header: SpHeader,
        time_stamp: &'time [u8],
        source_data_len: usize,
    ) -> PusTmCreator<'time, 'src_data> {
        let tm_sec_header = PusTmSecondaryHeader::new(
            MessageTypeId::new(1, subservice),
            msg_counter,
            self.dest_id,
            time_stamp,
        );
        PusTmCreator::new(
            sp_header,
            tm_sec_header,
            &src_data_buf[0..source_data_len],
            CreatorConfig::default(),
        )
    }
}

#[cfg(feature = "alloc")]
pub mod alloc_mod {
    use arbitrary_int::traits::Integer as _;
    use spacepackets::ecss::PusError;

    use super::*;
    use crate::legacy::pus::PusTmVariant;
    use core::cell::RefCell;

    #[derive(Clone)]
    pub struct VerificationReporterConfig {
        apid: u11,
        pub step_field_width: usize,
        pub fail_code_field_width: usize,
        pub max_fail_data_len: usize,
    }

    impl VerificationReporterConfig {
        pub fn new(
            apid: u11,
            step_field_width: usize,
            fail_code_field_width: usize,
            max_fail_data_len: usize,
        ) -> Self {
            Self {
                apid,
                step_field_width,
                fail_code_field_width,
                max_fail_data_len,
            }
        }
    }

    /// This trait allows hooking into the TM generation process of the [VerificationReporter].
    ///
    /// The [Self::modify_tm] function is called before the TM is sent. This allows users to change
    /// fields like the message count or sequence counter before the TM is sent.
    pub trait VerificationHook {
        fn modify_tm(&self, tm: &mut PusTmCreator);
    }

    /// [VerificationHook] which does nothing. This is the default hook variant for
    /// the [VerificationReporter], assuming that any necessary packet manipulation is performed by
    /// a centralized TM funnel or inlet.
    #[derive(Default, Copy, Clone)]
    pub struct DummyVerificationHook {}

    impl VerificationHook for DummyVerificationHook {
        fn modify_tm(&self, _tm: &mut PusTmCreator) {}
    }

    /// Primary verification reportewr object. It provides an API to send PUS 1 verification
    /// telemetry packets and verify the various steps of telecommand handling as specified in the
    /// PUS standard.
    ///
    /// It is assumed that the sequence counter and message counters are updated in a central
    /// TM funnel or TM inlet. This helper will always set those fields to 0. The APID and
    /// destination fields are assumed to be constant for a given repoter instance.
    #[derive(Clone)]
    pub struct VerificationReporter<
        VerificationHookInstance: VerificationHook = DummyVerificationHook,
    > {
        owner_id: ComponentId,
        source_data_buf: RefCell<alloc::vec::Vec<u8>>,
        pub report_creator: VerificationReportCreator,
        pub tm_hook: VerificationHookInstance,
    }

    impl VerificationReporter<DummyVerificationHook> {
        pub fn new(owner_id: ComponentId, cfg: &VerificationReporterConfig) -> Self {
            let reporter = VerificationReportCreator::new(cfg.apid);
            Self {
                owner_id,
                source_data_buf: RefCell::new(alloc::vec![
                    0;
                    RequestId::SIZE_AS_BYTES
                        + cfg.step_field_width
                        + cfg.fail_code_field_width
                        + cfg.max_fail_data_len
                ]),
                report_creator: reporter,
                tm_hook: DummyVerificationHook::default(),
            }
        }
    }

    impl<VerificationHookInstance: VerificationHook> VerificationReporter<VerificationHookInstance> {
        /// The provided [VerificationHook] can be used to modify a verification packet
        /// before it is sent.
        pub fn new_with_hook(
            owner_id: ComponentId,
            cfg: &VerificationReporterConfig,
            tm_hook: VerificationHookInstance,
        ) -> Self {
            let reporter = VerificationReportCreator::new(cfg.apid);
            Self {
                owner_id,
                source_data_buf: RefCell::new(alloc::vec![
                    0;
                    RequestId::SIZE_AS_BYTES
                        + cfg.step_field_width
                        + cfg.fail_code_field_width
                        + cfg.max_fail_data_len
                ]),
                report_creator: reporter,
                tm_hook,
            }
        }

        pub fn start_verification(
            &self,
            pus_tc: &(impl CcsdsPacket + IsPusTelecommand),
        ) -> VerificationToken<TcStateNone> {
            VerificationToken::<TcStateNone>::new(self.report_creator.read_request_id(pus_tc))
        }

        pub fn start_verification_with_req_id(
            &self,
            request_id: RequestId,
        ) -> VerificationToken<TcStateNone> {
            VerificationToken::<TcStateNone>::new(request_id)
        }

        delegate!(
            to self.report_creator {
                pub fn set_apid(&mut self, apid: u11);
                pub fn apid(&self) -> u11;
                pub fn dest_id(&self) -> u16;
                pub fn set_dest_id(&mut self, dest_id: u16);
            }
        );

        pub fn allowed_source_data_len(&self) -> usize {
            self.source_data_buf.borrow().capacity()
        }
    }

    impl<VerificationHookInstance: VerificationHook> VerificationReportingProvider
        for VerificationReporter<VerificationHookInstance>
    {
        delegate!(
            to self.report_creator {
                fn set_apid(&mut self, apid: Apid);
                fn apid(&self) -> Apid;
            }
        );

        fn start_verification(
            &self,
            pus_tc: &(impl CcsdsPacket + IsPusTelecommand),
        ) -> VerificationToken<TcStateNone> {
            VerificationToken::<TcStateNone>::new(RequestId::new(pus_tc))
        }

        fn owner_id(&self) -> ComponentId {
            self.owner_id
        }

        /// Package and send a PUS TM\[1, 1\] packet, see 8.1.2.1 of the PUS standard
        fn acceptance_success(
            &self,
            sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcStateNone>,
            time_stamp: &[u8],
        ) -> Result<VerificationToken<TcStateAccepted>, EcssTmtcError> {
            let mut source_data_buf = self.source_data_buf.borrow_mut();
            let mut tm_creator = self
                .report_creator
                .acceptance_success(
                    source_data_buf.as_mut_slice(),
                    &token.request_id(),
                    u14::ZERO,
                    0,
                    time_stamp,
                )
                .map_err(PusError::ByteConversion)?;
            self.tm_hook.modify_tm(&mut tm_creator);
            sender.send_tm(self.owner_id(), PusTmVariant::Direct(tm_creator))?;
            Ok(VerificationToken::new_accepted_state(token.request_id()))
        }

        /// Package and send a PUS TM\[1, 2\] packet, see 8.1.2.2 of the PUS standard
        fn acceptance_failure(
            &self,
            sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcStateNone>,
            params: FailParams,
        ) -> Result<(), EcssTmtcError> {
            let mut buf = self.source_data_buf.borrow_mut();
            let mut tm_creator = self
                .report_creator
                .acceptance_failure(
                    buf.as_mut_slice(),
                    &token.request_id(),
                    u14::ZERO,
                    0,
                    params,
                )
                .map_err(PusError::ByteConversion)?;
            self.tm_hook.modify_tm(&mut tm_creator);
            sender.send_tm(self.owner_id(), PusTmVariant::Direct(tm_creator))?;
            Ok(())
        }

        /// Package and send a PUS TM\[1, 3\] packet, see 8.1.2.3 of the PUS standard.
        ///
        /// Requires a token previously acquired by calling [Self::acceptance_success].
        fn start_success(
            &self,
            sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcStateAccepted>,
            time_stamp: &[u8],
        ) -> Result<VerificationToken<TcStateStarted>, EcssTmtcError> {
            let mut buf = self.source_data_buf.borrow_mut();
            let mut tm_creator = self
                .report_creator
                .start_success(
                    buf.as_mut_slice(),
                    &token.request_id(),
                    u14::ZERO,
                    0,
                    time_stamp,
                )
                .map_err(PusError::ByteConversion)?;
            self.tm_hook.modify_tm(&mut tm_creator);
            sender.send_tm(self.owner_id(), PusTmVariant::Direct(tm_creator))?;
            Ok(VerificationToken::new_started_state(token.request_id()))
        }

        /// Package and send a PUS TM\[1, 4\] packet, see 8.1.2.4 of the PUS standard.
        ///
        /// Requires a token previously acquired by calling [Self::acceptance_success]. It consumes
        /// the token because verification handling is done.
        fn start_failure(
            &self,
            sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcStateAccepted>,
            params: FailParams,
        ) -> Result<(), EcssTmtcError> {
            let mut buf = self.source_data_buf.borrow_mut();
            let mut tm_creator = self
                .report_creator
                .start_failure(
                    buf.as_mut_slice(),
                    &token.request_id(),
                    u14::ZERO,
                    0,
                    params,
                )
                .map_err(PusError::ByteConversion)?;
            self.tm_hook.modify_tm(&mut tm_creator);
            sender.send_tm(self.owner_id(), PusTmVariant::Direct(tm_creator))?;
            Ok(())
        }

        /// Package and send a PUS TM\[1, 5\] packet, see 8.1.2.5 of the PUS standard.
        ///
        /// Requires a token previously acquired by calling [Self::start_success].
        fn step_success(
            &self,
            sender: &(impl EcssTmSender + ?Sized),
            token: &VerificationToken<TcStateStarted>,
            time_stamp: &[u8],
            step: impl EcssEnumeration,
        ) -> Result<(), EcssTmtcError> {
            let mut buf = self.source_data_buf.borrow_mut();
            let mut tm_creator = self
                .report_creator
                .step_success(
                    buf.as_mut_slice(),
                    &token.request_id(),
                    u14::ZERO,
                    0,
                    time_stamp,
                    step,
                )
                .map_err(PusError::ByteConversion)?;
            self.tm_hook.modify_tm(&mut tm_creator);
            sender.send_tm(self.owner_id(), PusTmVariant::Direct(tm_creator))?;
            Ok(())
        }

        /// Package and send a PUS TM\[1, 6\] packet, see 8.1.2.6 of the PUS standard.
        ///
        /// Requires a token previously acquired by calling [Self::start_success]. It consumes the
        /// token because verification handling is done.
        fn step_failure(
            &self,
            sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcStateStarted>,
            params: FailParamsWithStep,
        ) -> Result<(), EcssTmtcError> {
            let mut buf = self.source_data_buf.borrow_mut();
            let mut tm_creator = self
                .report_creator
                .step_failure(buf.as_mut_slice(), token, u14::ZERO, 0, params)
                .map_err(PusError::ByteConversion)?;
            self.tm_hook.modify_tm(&mut tm_creator);
            sender.send_tm(self.owner_id(), PusTmVariant::Direct(tm_creator))?;
            Ok(())
        }

        /// Package and send a PUS TM\[1, 7\] packet, see 8.1.2.7 of the PUS standard.
        ///
        /// Requires a token previously acquired by calling [Self::start_success]. It consumes the
        /// token because verification handling is done.
        fn completion_success<TcState: WasAtLeastAccepted + Copy>(
            &self,
            // sender_id: ComponentId,
            sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcState>,
            time_stamp: &[u8],
        ) -> Result<(), EcssTmtcError> {
            let mut buf = self.source_data_buf.borrow_mut();
            let mut tm_creator = self
                .report_creator
                .completion_success(
                    buf.as_mut_slice(),
                    &token.request_id(),
                    u14::ZERO,
                    0,
                    time_stamp,
                )
                .map_err(PusError::ByteConversion)?;
            self.tm_hook.modify_tm(&mut tm_creator);
            sender.send_tm(self.owner_id, PusTmVariant::Direct(tm_creator))?;
            Ok(())
        }

        /// Package and send a PUS TM\[1, 8\] packet, see 8.1.2.8 of the PUS standard.
        ///
        /// Requires a token previously acquired by calling [Self::start_success]. It consumes the
        /// token because verification handling is done.
        fn completion_failure<TcState: WasAtLeastAccepted + Copy>(
            &self,
            sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcState>,
            params: FailParams,
        ) -> Result<(), EcssTmtcError> {
            let mut buf = self.source_data_buf.borrow_mut();
            let mut tm_creator = self
                .report_creator
                .completion_failure(
                    buf.as_mut_slice(),
                    &token.request_id(),
                    u14::ZERO,
                    0,
                    params,
                )
                .map_err(PusError::ByteConversion)?;
            self.tm_hook.modify_tm(&mut tm_creator);
            sender.send_tm(self.owner_id(), PusTmVariant::Direct(tm_creator))?;
            Ok(())
        }
    }
}

pub struct FailParamHelper<'stamp, 'fargs, 'buf, 'params> {
    pub timestamp: &'stamp [u8],
    pub error_code: &'fargs dyn EcssEnumeration,
    pub small_data_buf: &'buf mut [u8],
    pub params: Option<&'params Params>,
}

/// This helper function simplifies generating completion failures where the error data has
/// the generic [Params] type.
///
/// A small data buffer needs to be supplied for the [Params::Heapless] type because all data
/// suplied as error data must be held in a slice. Passing a static buffer avoids dynamic memory
/// allocation for this case.
///
/// Please note that this specific function can not propagate the [Params::Store] variant.
/// This function also might not be able to propagate other error variants which are added in
/// the future. The returned boolean on success denotes whether the error parameters were
/// propagated properly.
pub fn handle_completion_failure_with_generic_params<TcState: WasAtLeastAccepted + Copy>(
    tm_sender: &(impl EcssTmSender + ?Sized),
    verif_token: VerificationToken<TcState>,
    verif_reporter: &impl VerificationReportingProvider,
    helper: FailParamHelper,
) -> Result<bool, EcssTmtcError> {
    let mut error_params_propagated = true;
    if helper.params.is_none() {
        verif_reporter.completion_failure(
            tm_sender,
            verif_token,
            FailParams::new(helper.timestamp, helper.error_code, &[]),
        )?;
        return Ok(true);
    }
    let error_params = helper.params.unwrap();
    match error_params {
        Params::Heapless(heapless_param) => {
            heapless_param
                .write_to_be_bytes(&mut helper.small_data_buf[..heapless_param.written_len()])?;
            verif_reporter.completion_failure(
                tm_sender,
                verif_token,
                FailParams::new(
                    helper.timestamp,
                    helper.error_code,
                    &helper.small_data_buf[..heapless_param.written_len()],
                ),
            )?;
        }
        #[cfg(feature = "alloc")]
        Params::Vec(vec) => {
            verif_reporter.completion_failure(
                tm_sender,
                verif_token,
                FailParams::new(helper.timestamp, helper.error_code, vec),
            )?;
        }
        #[cfg(feature = "alloc")]
        Params::String(str) => {
            verif_reporter.completion_failure(
                tm_sender,
                verif_token,
                FailParams::new(helper.timestamp, helper.error_code, str.as_bytes()),
            )?;
        }
        _ => {
            verif_reporter.completion_failure(
                tm_sender,
                verif_token,
                FailParams::new(helper.timestamp, helper.error_code, &[]),
            )?;
            error_params_propagated = false;
        }
    }
    Ok(error_params_propagated)
}

/// This function is similar to [handle_completion_failure_with_generic_params] but handles the
/// step failure case.
pub fn handle_step_failure_with_generic_params(
    tm_sender: &(impl EcssTmSender + ?Sized),
    verif_token: VerificationToken<TcStateStarted>,
    verif_reporter: &impl VerificationReportingProvider,
    helper: FailParamHelper,
    step: &impl EcssEnumeration,
) -> Result<bool, EcssTmtcError> {
    if helper.params.is_none() {
        verif_reporter.step_failure(
            tm_sender,
            verif_token,
            FailParamsWithStep::new(helper.timestamp, step, helper.error_code, &[]),
        )?;
        return Ok(true);
    }
    let error_params = helper.params.unwrap();
    let mut error_params_propagated = true;
    match error_params {
        Params::Heapless(heapless_param) => {
            heapless_param
                .write_to_be_bytes(&mut helper.small_data_buf[..heapless_param.written_len()])?;
            verif_reporter.step_failure(
                tm_sender,
                verif_token,
                FailParamsWithStep::new(
                    helper.timestamp,
                    step,
                    helper.error_code,
                    &helper.small_data_buf[..heapless_param.written_len()],
                ),
            )?;
        }
        #[cfg(feature = "alloc")]
        Params::Vec(vec) => {
            verif_reporter.step_failure(
                tm_sender,
                verif_token,
                FailParamsWithStep::new(helper.timestamp, step, helper.error_code, vec),
            )?;
        }
        #[cfg(feature = "alloc")]
        Params::String(str) => {
            verif_reporter.step_failure(
                tm_sender,
                verif_token,
                FailParamsWithStep::new(helper.timestamp, step, helper.error_code, str.as_bytes()),
            )?;
        }
        _ => {
            verif_reporter.step_failure(
                tm_sender,
                verif_token,
                FailParamsWithStep::new(helper.timestamp, step, helper.error_code, &[]),
            )?;
            error_params_propagated = false;
        }
    }
    Ok(error_params_propagated)
}

#[cfg(any(feature = "test_util", test))]
pub mod test_util {
    use alloc::vec::Vec;
    use arbitrary_int::traits::Integer;
    use core::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;

    #[derive(Debug, PartialEq)]
    pub struct SuccessData {
        pub sender: ComponentId,
        pub time_stamp: Vec<u8>,
    }

    #[derive(Debug, PartialEq)]
    pub struct FailureData {
        pub sender: ComponentId,
        pub error_enum: u64,
        pub fail_data: Vec<u8>,
        pub time_stamp: Vec<u8>,
    }

    #[derive(Debug, PartialEq)]
    pub enum VerificationReportInfo {
        Added,
        AcceptanceSuccess(SuccessData),
        AcceptanceFailure(FailureData),
        StartedSuccess(SuccessData),
        StartedFailure(FailureData),
        StepSuccess { data: SuccessData, step: u16 },
        StepFailure(FailureData),
        CompletionSuccess(SuccessData),
        CompletionFailure(FailureData),
    }

    pub struct TestVerificationReporter {
        pub id: ComponentId,
        pub report_queue: RefCell<VecDeque<(RequestId, VerificationReportInfo)>>,
    }

    impl TestVerificationReporter {
        pub fn new(id: ComponentId) -> Self {
            Self {
                id,
                report_queue: Default::default(),
            }
        }
    }

    impl VerificationReportingProvider for TestVerificationReporter {
        fn start_verification(
            &self,
            pus_tc: &(impl CcsdsPacket + IsPusTelecommand),
        ) -> VerificationToken<TcStateNone> {
            let request_id = RequestId::new(pus_tc);
            self.report_queue
                .borrow_mut()
                .push_back((request_id, VerificationReportInfo::Added));
            VerificationToken::<TcStateNone>::new(RequestId::new(pus_tc))
        }

        fn set_apid(&mut self, _apid: Apid) {}

        fn apid(&self) -> Apid {
            Apid::ZERO
        }

        fn acceptance_success(
            &self,
            _sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcStateNone>,
            time_stamp: &[u8],
        ) -> Result<VerificationToken<TcStateAccepted>, EcssTmtcError> {
            self.report_queue.borrow_mut().push_back((
                token.request_id(),
                VerificationReportInfo::AcceptanceSuccess(SuccessData {
                    sender: self.owner_id(),
                    time_stamp: time_stamp.to_vec(),
                }),
            ));
            Ok(VerificationToken {
                state: PhantomData,
                request_id: token.request_id,
            })
        }

        fn acceptance_failure(
            &self,
            _sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcStateNone>,
            params: FailParams,
        ) -> Result<(), EcssTmtcError> {
            self.report_queue.borrow_mut().push_back((
                token.request_id(),
                VerificationReportInfo::AcceptanceFailure(FailureData {
                    sender: self.owner_id(),
                    error_enum: params.failure_code.value_raw(),
                    fail_data: params.failure_data.to_vec(),
                    time_stamp: params.time_stamp.to_vec(),
                }),
            ));
            Ok(())
        }

        fn start_success(
            &self,
            _sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcStateAccepted>,
            time_stamp: &[u8],
        ) -> Result<VerificationToken<TcStateStarted>, EcssTmtcError> {
            self.report_queue.borrow_mut().push_back((
                token.request_id(),
                VerificationReportInfo::StartedSuccess(SuccessData {
                    sender: self.owner_id(),
                    time_stamp: time_stamp.to_vec(),
                }),
            ));
            Ok(VerificationToken {
                state: PhantomData,
                request_id: token.request_id,
            })
        }

        fn start_failure(
            &self,
            _sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<super::TcStateAccepted>,
            params: FailParams,
        ) -> Result<(), EcssTmtcError> {
            self.report_queue.borrow_mut().push_back((
                token.request_id(),
                VerificationReportInfo::StartedFailure(FailureData {
                    sender: self.owner_id(),
                    error_enum: params.failure_code.value_raw(),
                    fail_data: params.failure_data.to_vec(),
                    time_stamp: params.time_stamp.to_vec(),
                }),
            ));
            Ok(())
        }

        fn step_success(
            &self,
            _sender: &(impl EcssTmSender + ?Sized),
            token: &VerificationToken<TcStateStarted>,
            time_stamp: &[u8],
            step: impl EcssEnumeration,
        ) -> Result<(), EcssTmtcError> {
            self.report_queue.borrow_mut().push_back((
                token.request_id(),
                VerificationReportInfo::StepSuccess {
                    data: SuccessData {
                        sender: self.owner_id(),
                        time_stamp: time_stamp.to_vec(),
                    },
                    step: step.value_raw() as u16,
                },
            ));
            Ok(())
        }

        fn step_failure(
            &self,
            _sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcStateStarted>,
            params: FailParamsWithStep,
        ) -> Result<(), EcssTmtcError> {
            self.report_queue.borrow_mut().push_back((
                token.request_id(),
                VerificationReportInfo::StepFailure(FailureData {
                    sender: self.owner_id(),
                    error_enum: params.common.failure_code.value_raw(),
                    fail_data: params.common.failure_data.to_vec(),
                    time_stamp: params.common.time_stamp.to_vec(),
                }),
            ));
            Ok(())
        }

        fn completion_success<TcState: super::WasAtLeastAccepted + Copy>(
            &self,
            _sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcState>,
            time_stamp: &[u8],
        ) -> Result<(), EcssTmtcError> {
            self.report_queue.borrow_mut().push_back((
                token.request_id(),
                VerificationReportInfo::CompletionSuccess(SuccessData {
                    sender: self.owner_id(),
                    time_stamp: time_stamp.to_vec(),
                }),
            ));
            Ok(())
        }

        fn completion_failure<TcState: WasAtLeastAccepted + Copy>(
            &self,
            _sender: &(impl EcssTmSender + ?Sized),
            token: VerificationToken<TcState>,
            params: FailParams,
        ) -> Result<(), EcssTmtcError> {
            self.report_queue.borrow_mut().push_back((
                token.request_id(),
                VerificationReportInfo::CompletionFailure(FailureData {
                    sender: self.owner_id(),
                    error_enum: params.failure_code.value_raw(),
                    fail_data: params.failure_data.to_vec(),
                    time_stamp: params.time_stamp.to_vec(),
                }),
            ));
            Ok(())
        }

        fn owner_id(&self) -> ComponentId {
            self.id
        }
    }

    impl TestVerificationReporter {
        pub fn check_next_was_added(&self, request_id: RequestId) {
            let (last_report_req_id, info) = self
                .report_queue
                .borrow_mut()
                .pop_front()
                .expect("report queue is empty");
            assert_eq!(request_id, last_report_req_id);
            assert_eq!(info, VerificationReportInfo::Added);
        }
        pub fn check_next_is_acceptance_success(&self, sender_id: ComponentId, req_id: RequestId) {
            let (last_report_req_id, info) = self
                .report_queue
                .borrow_mut()
                .pop_front()
                .expect("report queue is empty");
            assert_eq!(req_id, last_report_req_id);
            if let VerificationReportInfo::AcceptanceSuccess(data) = info {
                assert_eq!(data.sender, sender_id);
                return;
            }
            panic!("next message is not acceptance success message")
        }

        pub fn check_next_is_started_success(&self, sender_id: ComponentId, req_id: RequestId) {
            let (last_report_req_id, info) = self
                .report_queue
                .borrow_mut()
                .pop_front()
                .expect("report queue is empty");
            assert_eq!(req_id, last_report_req_id);
            if let VerificationReportInfo::StartedSuccess(data) = info {
                assert_eq!(data.sender, sender_id);
                return;
            }
            panic!("next message is not start success message")
        }

        pub fn check_next_is_step_success(
            &self,
            sender_id: ComponentId,
            request_id: RequestId,
            expected_step: u16,
        ) {
            let (last_report_req_id, info) = self
                .report_queue
                .borrow_mut()
                .pop_front()
                .expect("report queue is empty");
            assert_eq!(request_id, last_report_req_id);
            if let VerificationReportInfo::StepSuccess { data, step } = info {
                assert_eq!(data.sender, sender_id);
                assert_eq!(expected_step, step);
                return;
            }
            panic!("next message is not step success message: {info:?}")
        }

        pub fn check_next_is_step_failure(
            &self,
            sender_id: ComponentId,
            request_id: RequestId,
            error_code: u64,
        ) {
            let (last_report_req_id, info) = self
                .report_queue
                .borrow_mut()
                .pop_front()
                .expect("report queue is empty");
            assert_eq!(request_id, last_report_req_id);
            if let VerificationReportInfo::StepFailure(data) = info {
                assert_eq!(data.sender, sender_id);
                assert_eq!(data.error_enum, error_code);
                return;
            }
            panic!("next message is not step failure message")
        }

        pub fn check_next_is_completion_success(
            &self,
            sender_id: ComponentId,
            request_id: RequestId,
        ) {
            let (last_report_req_id, info) = self
                .report_queue
                .borrow_mut()
                .pop_front()
                .expect("report queue is empty");
            assert_eq!(request_id, last_report_req_id);
            if let VerificationReportInfo::CompletionSuccess(data) = info {
                assert_eq!(data.sender, sender_id);
                return;
            }
            panic!("next message is not completion success message: {info:?}")
        }

        pub fn check_next_is_completion_failure(
            &mut self,
            sender_id: ComponentId,
            request_id: RequestId,
            error_code: u64,
        ) {
            let (last_report_req_id, info) = self
                .report_queue
                .get_mut()
                .pop_front()
                .expect("report queue is empty");
            assert_eq!(request_id, last_report_req_id);
            if let VerificationReportInfo::CompletionFailure(data) = info {
                assert_eq!(data.sender, sender_id);
                assert_eq!(data.error_enum, error_code);
                return;
            }
            panic!("next message is not completion failure message: {info:?}")
        }

        pub fn assert_full_completion_success(
            &mut self,
            sender_id: ComponentId,
            request_id: RequestId,
            expected_steps: Option<u16>,
        ) {
            self.check_next_was_added(request_id);
            self.check_next_is_acceptance_success(sender_id, request_id);
            self.check_next_is_started_success(sender_id, request_id);
            if let Some(highest_num) = expected_steps {
                for i in 0..highest_num {
                    self.check_next_is_step_success(sender_id, request_id, i);
                }
            }
            self.check_next_is_completion_success(sender_id, request_id);
        }

        pub fn assert_completion_failure(
            &mut self,
            sender_id: ComponentId,
            request_id: RequestId,
            expected_steps: Option<u16>,
            error_code: u64,
        ) {
            self.check_next_was_added(request_id);
            self.check_next_is_acceptance_success(sender_id, request_id);
            self.check_next_is_started_success(sender_id, request_id);
            if let Some(highest_num) = expected_steps {
                for i in 0..highest_num {
                    self.check_next_is_step_success(sender_id, request_id, i);
                }
            }
            self.check_next_is_completion_failure(sender_id, request_id, error_code);
        }

        pub fn get_next_verification_message(&mut self) -> (RequestId, VerificationReportInfo) {
            self.report_queue
                .get_mut()
                .pop_front()
                .expect("report queue is empty")
        }
    }
}

#[cfg(test)]
pub mod tests {
    use crate::ComponentId;
    use crate::legacy::pus::test_util::{TEST_APID, TEST_COMPONENT_ID_0};
    use crate::legacy::pus::tests::CommonTmInfo;
    use crate::legacy::pus::verification::{
        EcssTmSender, EcssTmtcError, FailParams, FailParamsWithStep, RequestId, TcStateNone,
        VerificationReporter, VerificationReporterConfig, VerificationToken,
        handle_step_failure_with_generic_params,
    };
    use crate::legacy::pus::{ChannelWithId, PusTmVariant};
    use crate::params::Params;
    use crate::pool::{SharedStaticMemoryPool, StaticMemoryPool, StaticPoolConfig};
    use crate::request::MessageMetadata;
    use crate::spacepackets::seq_count::{SequenceCounter, SequenceCounterCcsdsSimple};
    use crate::tmtc::{PacketSenderWithSharedPool, SharedPacketPool};
    use alloc::format;
    use alloc::string::ToString;
    use arbitrary_int::traits::Integer;
    use arbitrary_int::{u11, u14};
    use spacepackets::ecss::tc::{PusTcCreator, PusTcReader, PusTcSecondaryHeader};
    use spacepackets::ecss::{
        CreatorConfig, EcssEnumU8, EcssEnumU16, EcssEnumU32, EcssEnumeration, MessageTypeId,
        PusError, PusPacket,
    };
    use spacepackets::{ByteConversionError, SpHeader};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::sync::{RwLock, mpsc};
    use std::vec;
    use std::vec::Vec;

    use super::{
        DummyVerificationHook, FailParamHelper, SequenceCounterSimple, TcStateAccepted,
        TcStateStarted, VerificationHook, VerificationReportingProvider, WasAtLeastAccepted,
        handle_completion_failure_with_generic_params,
    };

    fn is_send<T: Send>(_: &T) {}
    #[allow(dead_code)]
    fn is_sync<T: Sync>(_: &T) {}

    const EMPTY_STAMP: [u8; 7] = [0; 7];
    const DUMMY_STAMP: &[u8] = &[0, 1, 0, 1, 0, 1, 0];

    #[derive(Debug, Eq, PartialEq, Clone)]
    struct TmInfo {
        pub requestor: MessageMetadata,
        pub common: CommonTmInfo,
        pub additional_data: Option<Vec<u8>>,
    }

    #[derive(Default, Clone)]
    struct TestSender {
        pub service_queue: RefCell<VecDeque<TmInfo>>,
    }

    impl ChannelWithId for TestSender {
        fn id(&self) -> ComponentId {
            0
        }
        fn name(&self) -> &'static str {
            "test_sender"
        }
    }

    impl EcssTmSender for TestSender {
        fn send_tm(&self, sender_id: ComponentId, tm: PusTmVariant) -> Result<(), EcssTmtcError> {
            match tm {
                PusTmVariant::InStore(_) => {
                    panic!("TestSender: Can not deal with addresses");
                }
                PusTmVariant::Direct(tm) => {
                    assert_eq!(PusPacket::service_type_id(&tm), 1);
                    assert!(!tm.source_data().is_empty());
                    let mut time_stamp = [0; 7];
                    time_stamp.clone_from_slice(&tm.timestamp()[0..7]);
                    let src_data = tm.source_data();
                    assert!(src_data.len() >= 4);
                    let req_id =
                        RequestId::from_bytes(&src_data[0..RequestId::SIZE_AS_BYTES]).unwrap();
                    let mut vec = None;
                    if src_data.len() > 4 {
                        let mut new_vec = Vec::new();
                        new_vec.extend_from_slice(&src_data[RequestId::SIZE_AS_BYTES..]);
                        vec = Some(new_vec);
                    }
                    self.service_queue.borrow_mut().push_back(TmInfo {
                        requestor: MessageMetadata::new(req_id.into(), sender_id),
                        common: CommonTmInfo::new_from_tm(&tm),
                        additional_data: vec,
                    });
                    Ok(())
                }
            }
        }
    }

    #[derive(Default)]
    pub struct SequenceCounterHook {
        pub seq_counter: SequenceCounterCcsdsSimple,
        pub msg_counter: SequenceCounterSimple<u16>,
    }

    impl VerificationHook for SequenceCounterHook {
        fn modify_tm(&self, tm: &mut spacepackets::ecss::tm::PusTmCreator) {
            tm.set_seq_count(u14::new(self.seq_counter.get_and_increment()));
            tm.set_msg_counter(self.msg_counter.get_and_increment());
        }
    }

    struct VerificationReporterTestbench<
        VerificationHookInstance: VerificationHook = DummyVerificationHook,
    > {
        pub id: ComponentId,
        sender: TestSender,
        reporter: VerificationReporter<VerificationHookInstance>,
        pub request_id: RequestId,
        tc: Vec<u8>,
    }

    fn base_reporter(id: ComponentId, max_fail_data_len: usize) -> VerificationReporter {
        let cfg = VerificationReporterConfig::new(TEST_APID, 1, 2, max_fail_data_len);
        VerificationReporter::new(id, &cfg)
    }

    fn reporter_with_hook<VerificationHookInstance: VerificationHook>(
        id: ComponentId,
        hook: VerificationHookInstance,
    ) -> VerificationReporter<VerificationHookInstance> {
        let cfg = VerificationReporterConfig::new(TEST_APID, 1, 2, 8);
        VerificationReporter::new_with_hook(id, &cfg, hook)
    }

    impl<VerificiationHook: VerificationHook> VerificationReporterTestbench<VerificiationHook> {
        fn new_with_hook(id: ComponentId, tc: PusTcCreator, tm_hook: VerificiationHook) -> Self {
            let reporter = reporter_with_hook(id, tm_hook);
            Self {
                id,
                sender: TestSender::default(),
                reporter,
                request_id: RequestId::new(&tc),
                tc: tc.to_vec().unwrap(),
            }
        }

        pub fn start_verification(&self) -> VerificationToken<TcStateNone> {
            let tc_reader = PusTcReader::new(&self.tc).unwrap();
            self.reporter.start_verification(&tc_reader)
        }

        #[allow(dead_code)]
        fn set_dest_id(&mut self, dest_id: u16) {
            self.reporter.set_dest_id(dest_id);
        }

        fn acceptance_success(
            &self,
            token: VerificationToken<TcStateNone>,
            time_stamp: &[u8],
        ) -> Result<VerificationToken<TcStateAccepted>, EcssTmtcError> {
            self.reporter
                .acceptance_success(&self.sender, token, time_stamp)
        }

        fn acceptance_failure(
            &self,
            token: VerificationToken<TcStateNone>,
            params: FailParams,
        ) -> Result<(), EcssTmtcError> {
            self.reporter
                .acceptance_failure(&self.sender, token, params)
        }

        fn start_success(
            &self,
            token: VerificationToken<TcStateAccepted>,
            time_stamp: &[u8],
        ) -> Result<VerificationToken<TcStateStarted>, EcssTmtcError> {
            self.reporter.start_success(&self.sender, token, time_stamp)
        }

        fn start_failure(
            &self,
            token: VerificationToken<TcStateAccepted>,
            params: FailParams,
        ) -> Result<(), EcssTmtcError> {
            self.reporter.start_failure(&self.sender, token, params)
        }

        fn step_success(
            &self,
            token: &VerificationToken<TcStateStarted>,
            time_stamp: &[u8],
            step: impl EcssEnumeration,
        ) -> Result<(), EcssTmtcError> {
            self.reporter
                .step_success(&self.sender, token, time_stamp, step)
        }

        fn step_failure(
            &self,
            token: VerificationToken<TcStateStarted>,
            params: FailParamsWithStep,
        ) -> Result<(), EcssTmtcError> {
            self.reporter.step_failure(&self.sender, token, params)
        }

        fn completion_success<TcState: WasAtLeastAccepted + Copy>(
            &self,
            token: VerificationToken<TcState>,
            time_stamp: &[u8],
        ) -> Result<(), EcssTmtcError> {
            self.reporter
                .completion_success(&self.sender, token, time_stamp)
        }

        fn completion_failure<TcState: WasAtLeastAccepted + Copy>(
            &self,
            token: VerificationToken<TcState>,
            params: FailParams,
        ) -> Result<(), EcssTmtcError> {
            self.reporter
                .completion_failure(&self.sender, token, params)
        }

        fn check_acceptance_success(&self, timestamp: &[u8; 7]) {
            let cmp_info = TmInfo {
                requestor: MessageMetadata::new(self.request_id.into(), self.id),
                common: CommonTmInfo::new(
                    1,
                    TEST_APID,
                    u14::ZERO,
                    0,
                    self.reporter.dest_id(),
                    timestamp,
                ),
                additional_data: None,
            };
            let mut service_queue = self.sender.service_queue.borrow_mut();
            assert!(!service_queue.is_empty());
            let info = service_queue.pop_front().unwrap();
            assert_eq!(info, cmp_info);
        }

        fn check_start_success(&mut self, seq_count: u14, msg_counter: u16, timestamp: &[u8]) {
            let mut srv_queue = self.sender.service_queue.borrow_mut();
            let cmp_info = TmInfo {
                requestor: MessageMetadata::new(self.request_id.into(), self.id),
                common: CommonTmInfo::new(
                    3,
                    TEST_APID,
                    seq_count,
                    msg_counter,
                    self.reporter.dest_id(),
                    timestamp,
                ),
                additional_data: None,
            };
            let info = srv_queue.pop_front().unwrap();
            assert_eq!(info, cmp_info);
        }

        fn check_completion_success(&mut self, seq_count: u14, msg_counter: u16) {
            let cmp_info = TmInfo {
                requestor: MessageMetadata::new(self.request_id.into(), self.id),
                common: CommonTmInfo::new(
                    7,
                    TEST_APID,
                    seq_count,
                    msg_counter,
                    self.reporter.dest_id(),
                    &EMPTY_STAMP,
                ),
                additional_data: None,
            };
            let info = self.sender.service_queue.borrow_mut().pop_front().unwrap();
            assert_eq!(info, cmp_info);
        }
    }

    impl VerificationReporterTestbench<DummyVerificationHook> {
        fn new(id: ComponentId, tc: PusTcCreator, max_fail_data_len: usize) -> Self {
            let reporter = base_reporter(id, max_fail_data_len);
            Self {
                id,
                sender: TestSender::default(),
                reporter,
                request_id: RequestId::new(&tc),
                tc: tc.to_vec().unwrap(),
            }
        }

        fn check_acceptance_failure(&mut self, timestamp: &[u8; 7]) {
            let cmp_info = TmInfo {
                requestor: MessageMetadata::new(self.request_id.into(), self.id),
                common: CommonTmInfo::new(
                    2,
                    TEST_APID,
                    u14::ZERO,
                    0,
                    self.reporter.dest_id(),
                    timestamp,
                ),
                additional_data: Some([0, 2].to_vec()),
            };
            let service_queue = self.sender.service_queue.get_mut();
            assert_eq!(service_queue.len(), 1);
            let info = service_queue.pop_front().unwrap();
            assert_eq!(info, cmp_info);
        }

        fn check_start_failure(&mut self, fail_data_raw: [u8; 4]) {
            let mut srv_queue = self.sender.service_queue.borrow_mut();
            assert_eq!(srv_queue.len(), 2);
            let mut cmp_info = TmInfo {
                requestor: MessageMetadata::new(self.request_id.into(), self.id),
                common: CommonTmInfo::new_zero_seq_count(1, TEST_APID, 0, &EMPTY_STAMP),
                additional_data: None,
            };
            let mut info = srv_queue.pop_front().unwrap();
            assert_eq!(info, cmp_info);

            cmp_info = TmInfo {
                requestor: MessageMetadata::new(self.request_id.into(), self.id),
                common: CommonTmInfo::new_zero_seq_count(4, TEST_APID, 0, &EMPTY_STAMP),
                additional_data: Some([&[22], fail_data_raw.as_slice()].concat().to_vec()),
            };
            info = srv_queue.pop_front().unwrap();
            assert_eq!(info, cmp_info);
        }

        fn check_step_success(&mut self, step: u8, timestamp: &[u8; 7]) {
            let cmp_info = TmInfo {
                requestor: MessageMetadata::new(self.request_id.into(), self.id),
                common: CommonTmInfo::new_zero_seq_count(5, TEST_APID, 0, timestamp),
                additional_data: Some([step].to_vec()),
            };
            let mut srv_queue = self.sender.service_queue.borrow_mut();
            let info = srv_queue.pop_front().unwrap();
            assert_eq!(info, cmp_info);
        }

        fn check_step_failure(
            &mut self,
            step: &impl EcssEnumeration,
            error_code: &impl EcssEnumeration,
            fail_data: &[u8],
        ) {
            let mut additional_data = Vec::new();
            additional_data.extend(step.to_vec());
            additional_data.extend(error_code.to_vec());
            additional_data.extend(fail_data);
            let cmp_info = TmInfo {
                requestor: MessageMetadata::new(self.request_id.into(), self.id),
                common: CommonTmInfo::new_zero_seq_count(
                    6,
                    TEST_APID,
                    self.reporter.dest_id(),
                    &EMPTY_STAMP,
                ),
                additional_data: Some(additional_data),
            };
            let info = self.sender.service_queue.get_mut().pop_front().unwrap();
            assert_eq!(info, cmp_info);
        }

        fn check_completion_failure(
            &mut self,
            error_code: &impl EcssEnumeration,
            fail_data: &[u8],
        ) {
            let mut additional_data = Vec::new();
            additional_data.extend(error_code.to_vec());
            additional_data.extend(fail_data);
            let cmp_info = TmInfo {
                requestor: MessageMetadata::new(self.request_id.into(), self.id),
                common: CommonTmInfo::new_zero_seq_count(
                    8,
                    TEST_APID,
                    self.reporter.dest_id(),
                    &EMPTY_STAMP,
                ),
                additional_data: Some(additional_data),
            };
            let info = self.sender.service_queue.get_mut().pop_front().unwrap();
            assert_eq!(info, cmp_info);
        }
    }

    fn create_generic_ping() -> PusTcCreator<'static> {
        let sph = SpHeader::new_for_unseg_tc(TEST_APID, u14::new(0x34), 0);
        let tc_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(17, 1));
        PusTcCreator::new(sph, tc_header, &[], CreatorConfig::default())
    }

    #[test]
    fn test_mpsc_verif_send() {
        let pool = StaticMemoryPool::new(StaticPoolConfig::new_from_subpool_cfg_tuples(
            vec![(8, 8)],
            false,
        ));
        let shared_tm_store =
            SharedPacketPool::new(&SharedStaticMemoryPool::new(RwLock::new(pool)));
        let (tx, _) = mpsc::sync_channel(10);
        let mpsc_verif_sender = PacketSenderWithSharedPool::new(tx, shared_tm_store);
        is_send(&mpsc_verif_sender);
    }

    #[test]
    fn test_state() {
        let mut testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 16);
        assert_eq!(testbench.reporter.apid(), TEST_APID);
        testbench.reporter.set_apid(u11::new(TEST_APID.value() + 1));
        assert_eq!(testbench.reporter.apid().value(), TEST_APID.value() + 1);
    }

    #[test]
    fn test_basic_acceptance_success() {
        let testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 16);
        let token = testbench.start_verification();
        testbench
            .acceptance_success(token, &EMPTY_STAMP)
            .expect("sending acceptance success failed");
        testbench.check_acceptance_success(&EMPTY_STAMP);
    }

    #[test]
    fn test_basic_acceptance_failure() {
        let mut testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 16);
        let init_token = testbench.start_verification();
        let timestamp = [1, 2, 3, 4, 5, 6, 7];
        let fail_code = EcssEnumU16::new(2);
        let fail_params = FailParams::new_no_fail_data(timestamp.as_slice(), &fail_code);
        testbench
            .acceptance_failure(init_token, fail_params)
            .expect("sending acceptance failure failed");
        testbench.check_acceptance_failure(&timestamp);
    }

    #[test]
    fn test_basic_acceptance_failure_with_helper() {
        let mut testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 16);
        let init_token = testbench.start_verification();
        let timestamp = [1, 2, 3, 4, 5, 6, 7];
        let fail_code = EcssEnumU16::new(2);
        let fail_params = FailParams::new_no_fail_data(timestamp.as_slice(), &fail_code);
        testbench
            .acceptance_failure(init_token, fail_params)
            .expect("sending acceptance failure failed");
        testbench.check_acceptance_failure(&timestamp);
    }

    #[test]
    fn test_acceptance_fail_data_too_large() {
        let testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 8);
        let init_token = testbench.start_verification();
        let stamp_buf = [1, 2, 3, 4, 5, 6, 7];
        let fail_code = EcssEnumU16::new(2);
        let fail_data: [u8; 16] = [0; 16];
        // 4 req ID + 1 byte step + 2 byte error code + 8 byte fail data
        assert_eq!(testbench.reporter.allowed_source_data_len(), 15);
        let fail_params = FailParams::new(stamp_buf.as_slice(), &fail_code, fail_data.as_slice());
        let result = testbench.acceptance_failure(init_token, fail_params);
        assert!(result.is_err());
        let error = result.unwrap_err();
        match error {
            EcssTmtcError::Pus(PusError::ByteConversion(e)) => match e {
                ByteConversionError::ToSliceTooSmall { found, expected } => {
                    assert_eq!(
                        expected,
                        fail_data.len() + RequestId::SIZE_AS_BYTES + fail_code.size()
                    );
                    assert_eq!(found, testbench.reporter.allowed_source_data_len());
                }
                _ => {
                    panic!("{}", format!("Unexpected error {:?}", e))
                }
            },
            _ => {
                panic!("{}", format!("Unexpected error {:?}", error))
            }
        }
    }

    #[test]
    fn test_basic_acceptance_failure_with_fail_data() {
        let testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 16);
        let fail_code = EcssEnumU8::new(10);
        let fail_data = EcssEnumU32::new(12);
        let mut fail_data_raw = [0; 4];
        fail_data.write_to_slice_be(&mut fail_data_raw).unwrap();
        let fail_params = FailParams::new(&EMPTY_STAMP, &fail_code, fail_data_raw.as_slice());
        let init_token = testbench.start_verification();
        testbench
            .acceptance_failure(init_token, fail_params)
            .expect("sending acceptance failure failed");
        let cmp_info = TmInfo {
            requestor: MessageMetadata::new(testbench.request_id.into(), testbench.id),
            common: CommonTmInfo::new_zero_seq_count(2, TEST_APID, 0, &EMPTY_STAMP),
            additional_data: Some([10, 0, 0, 0, 12].to_vec()),
        };
        let mut service_queue = testbench.sender.service_queue.borrow_mut();
        assert_eq!(service_queue.len(), 1);
        let info = service_queue.pop_front().unwrap();
        assert_eq!(info, cmp_info);
    }

    #[test]
    fn test_start_failure() {
        let mut testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 16);
        let init_token = testbench.start_verification();
        let fail_code = EcssEnumU8::new(22);
        let fail_data: i32 = -12;
        let mut fail_data_raw = [0; 4];
        fail_data_raw.copy_from_slice(fail_data.to_be_bytes().as_slice());
        let fail_params = FailParams::new(&EMPTY_STAMP, &fail_code, fail_data_raw.as_slice());

        let accepted_token = testbench
            .acceptance_success(init_token, &EMPTY_STAMP)
            .expect("Sending acceptance success failed");
        testbench
            .start_failure(accepted_token, fail_params)
            .expect("Start failure failure");
        testbench.check_start_failure(fail_data_raw);
    }

    #[test]
    fn test_start_failure_with_helper() {
        let mut testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 16);
        let token = testbench.start_verification();
        let fail_code = EcssEnumU8::new(22);
        let fail_data: i32 = -12;
        let mut fail_data_raw = [0; 4];
        fail_data_raw.copy_from_slice(fail_data.to_be_bytes().as_slice());
        let fail_params = FailParams::new(&EMPTY_STAMP, &fail_code, fail_data_raw.as_slice());

        let accepted_token = testbench
            .acceptance_success(token, &EMPTY_STAMP)
            .expect("acceptance  failed");
        testbench
            .start_failure(accepted_token, fail_params)
            .expect("start failure failed");
        testbench.check_start_failure(fail_data_raw);
    }

    #[test]
    fn test_steps_success() {
        let mut testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 16);
        let token = testbench.start_verification();
        let accepted_token = testbench
            .acceptance_success(token, &EMPTY_STAMP)
            .expect("acceptance  failed");
        let started_token = testbench
            .start_success(accepted_token, &EMPTY_STAMP)
            .expect("acceptance  failed");
        testbench
            .step_success(&started_token, &EMPTY_STAMP, EcssEnumU8::new(0))
            .expect("step 0 failed");
        testbench
            .step_success(&started_token, &EMPTY_STAMP, EcssEnumU8::new(1))
            .expect("step 1 failed");
        assert_eq!(testbench.sender.service_queue.borrow().len(), 4);
        testbench.check_acceptance_success(&EMPTY_STAMP);
        testbench.check_start_success(u14::ZERO, 0, &EMPTY_STAMP);
        testbench.check_step_success(0, &EMPTY_STAMP);
        testbench.check_step_success(1, &EMPTY_STAMP);
    }

    #[test]
    fn test_step_failure() {
        let mut testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 16);
        let token = testbench.start_verification();
        let fail_code = EcssEnumU32::new(0x1020);
        let fail_data: f32 = -22.3232;
        let mut fail_data_raw = [0; 4];
        fail_data_raw.copy_from_slice(fail_data.to_be_bytes().as_slice());
        let fail_step = EcssEnumU8::new(1);
        let fail_params = FailParamsWithStep::new(
            &EMPTY_STAMP,
            &fail_step,
            &fail_code,
            fail_data_raw.as_slice(),
        );

        let accepted_token = testbench
            .acceptance_success(token, &EMPTY_STAMP)
            .expect("Sending acceptance success failed");
        let started_token = testbench
            .start_success(accepted_token, DUMMY_STAMP)
            .expect("Sending start success failed");
        testbench
            .step_success(&started_token, &EMPTY_STAMP, EcssEnumU8::new(0))
            .expect("Sending completion success failed");
        testbench
            .step_failure(started_token, fail_params)
            .expect("Step failure failed");
        testbench.check_acceptance_success(&EMPTY_STAMP);
        testbench.check_start_success(u14::ZERO, 0, DUMMY_STAMP);
        testbench.check_step_success(0, &EMPTY_STAMP);
        testbench.check_step_failure(&fail_step, &fail_code, &fail_data_raw);
    }

    #[test]
    fn test_completion_failure() {
        let mut testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 16);
        let token = testbench.start_verification();
        let fail_code = EcssEnumU32::new(0x1020);
        let fail_params = FailParams::new_no_fail_data(&EMPTY_STAMP, &fail_code);

        let accepted_token = testbench
            .acceptance_success(token, &EMPTY_STAMP)
            .expect("Sending acceptance success failed");
        let started_token = testbench
            .start_success(accepted_token, DUMMY_STAMP)
            .expect("Sending start success failed");
        testbench
            .completion_failure(started_token, fail_params)
            .expect("Completion failure");
        testbench.check_acceptance_success(&EMPTY_STAMP);
        testbench.check_start_success(u14::ZERO, 0, DUMMY_STAMP);

        testbench.check_completion_failure(&fail_code, &[]);
    }

    #[test]
    fn test_complete_success_sequence() {
        let mut testbench =
            VerificationReporterTestbench::new(TEST_COMPONENT_ID_0.id(), create_generic_ping(), 16);
        let token = testbench.start_verification();
        let accepted_token = testbench
            .acceptance_success(token, &EMPTY_STAMP)
            .expect("Sending acceptance success failed");
        let started_token = testbench
            .start_success(accepted_token, DUMMY_STAMP)
            .expect("Sending start success failed");
        testbench
            .completion_success(started_token, &EMPTY_STAMP)
            .expect("Sending completion success failed");
        testbench.check_acceptance_success(&EMPTY_STAMP);
        testbench.check_start_success(u14::ZERO, 0, DUMMY_STAMP);
        testbench.check_completion_success(u14::ZERO, 0);
    }

    #[test]
    fn test_packet_manipulation() {
        let mut testbench = VerificationReporterTestbench::new_with_hook(
            TEST_COMPONENT_ID_0.id(),
            create_generic_ping(),
            SequenceCounterHook::default(),
        );
        let token = testbench.start_verification();
        let accepted_token = testbench
            .acceptance_success(token, &EMPTY_STAMP)
            .expect("Sending acceptance success failed");
        let started_token = testbench
            .start_success(accepted_token, &[0, 1, 0, 1, 0, 1, 0])
            .expect("Sending start success failed");
        testbench
            .completion_success(started_token, &EMPTY_STAMP)
            .expect("Sending completion success failed");
        testbench.check_acceptance_success(&EMPTY_STAMP);
        testbench.check_start_success(u14::new(1), 1, DUMMY_STAMP);
        testbench.check_completion_success(u14::new(2), 2);
    }

    #[test]
    fn test_completion_failure_helper_string_param() {
        let mut testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 32);
        let token = testbench.start_verification();
        let accepted_token = testbench
            .acceptance_success(token, &EMPTY_STAMP)
            .expect("Sending acceptance success failed");
        let mut small_data_buf: [u8; 16] = [0; 16];
        let fail_code = EcssEnumU8::new(1);
        let fail_data = "error 404 oh no".to_string();
        let fail_params = Params::String(fail_data.clone());
        let result = handle_completion_failure_with_generic_params(
            &testbench.sender,
            accepted_token,
            &testbench.reporter,
            FailParamHelper {
                timestamp: &EMPTY_STAMP,
                error_code: &fail_code,
                small_data_buf: &mut small_data_buf,
                params: Some(&fail_params),
            },
        );
        assert!(result.unwrap());
        testbench.check_acceptance_success(&EMPTY_STAMP);
        testbench.check_completion_failure(&fail_code, fail_data.as_bytes());
    }

    #[test]
    fn test_step_failure_helper_string_param() {
        let mut testbench = VerificationReporterTestbench::new(0, create_generic_ping(), 32);
        let token = testbench.start_verification();
        let accepted_token = testbench
            .acceptance_success(token, &EMPTY_STAMP)
            .expect("Sending acceptance success failed");
        let started_token = testbench
            .start_success(accepted_token, &EMPTY_STAMP)
            .expect("Sending start success failed");
        let mut small_data_buf: [u8; 16] = [0; 16];
        let step = EcssEnumU8::new(2);
        let fail_code = EcssEnumU8::new(1);
        let fail_data = "AAAAAAAAAAAHHHHHH".to_string();
        let fail_params = Params::String(fail_data.clone());
        let result = handle_step_failure_with_generic_params(
            &testbench.sender,
            started_token,
            &testbench.reporter,
            FailParamHelper {
                timestamp: &EMPTY_STAMP,
                error_code: &fail_code,
                small_data_buf: &mut small_data_buf,
                params: Some(&fail_params),
            },
            &step,
        );
        assert!(result.unwrap());
        testbench.check_acceptance_success(&EMPTY_STAMP);
        testbench.check_start_success(u14::ZERO, 0, &EMPTY_STAMP);
        testbench.check_step_failure(&step, &fail_code, fail_data.as_bytes());
    }

    #[test]
    fn test_completion_failure_helper_vec_param() {
        // TODO: Test this.
    }

    #[test]
    fn test_completion_failure_helper_raw_param() {
        // TODO: Test this.
    }

    #[test]
    fn test_completion_failure_helper_store_param_ignored() {
        // TODO: Test this.
    }
}
