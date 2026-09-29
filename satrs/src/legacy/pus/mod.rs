//! # PUS support modules
//!
//! This module contains structures to make working with the PUS C standard easier.
//! The example-std application contains various usage examples of these components.
pub mod event;

use self::verification::{TcStateAccepted, TcStateToken, VerificationToken};
use crate::ComponentId;
use crate::pool::{PoolAddr, PoolError};
use crate::queue::{GenericReceiveError, GenericSendError};
use crate::request::{GenericMessage, MessageMetadata, RequestId};
#[cfg(feature = "alloc")]
use crate::tmtc::PacketAsVec;
use crate::tmtc::PacketInPool;
use core::fmt::{Display, Formatter};
use core::time::Duration;
#[cfg(feature = "alloc")]
use downcast_rs::{Downcast, impl_downcast};
#[cfg(feature = "alloc")]
use dyn_clone::DynClone;
#[cfg(feature = "std")]
use std::error::Error;

use spacepackets::ecss::PusError;
use spacepackets::ecss::tc::{PusTcCreator, PusTcReader};
use spacepackets::ecss::tm::PusTmCreator;
use spacepackets::{ByteConversionError, SpHeader};

pub mod action;
pub mod mode;
pub mod scheduler;
#[cfg(feature = "std")]
pub mod scheduler_srv;
#[cfg(feature = "std")]
pub mod test;
pub mod verification;

#[cfg(feature = "alloc")]
pub use alloc_mod::*;

#[cfg(feature = "std")]
pub use std_mod::*;

use self::verification::VerificationReportingProvider;

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PusTmVariant<'time, 'src_data> {
    InStore(PoolAddr),
    Direct(PusTmCreator<'time, 'src_data>),
}

impl From<PoolAddr> for PusTmVariant<'_, '_> {
    fn from(value: PoolAddr) -> Self {
        Self::InStore(value)
    }
}

impl<'time, 'src_data> From<PusTmCreator<'time, 'src_data>> for PusTmVariant<'time, 'src_data> {
    fn from(value: PusTmCreator<'time, 'src_data>) -> Self {
        Self::Direct(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EcssTmtcError {
    Store(PoolError),
    ByteConversion(ByteConversionError),
    Pus(PusError),
    CantSendAddr(PoolAddr),
    CantSendDirectTm,
    Send(GenericSendError),
    Receive(GenericReceiveError),
}

impl Display for EcssTmtcError {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        match self {
            EcssTmtcError::Store(store) => {
                write!(f, "ecss tmtc error: {store}")
            }
            EcssTmtcError::ByteConversion(e) => {
                write!(f, "ecss tmtc error: {e}")
            }
            EcssTmtcError::Pus(e) => {
                write!(f, "ecss tmtc error: {e}")
            }
            EcssTmtcError::CantSendAddr(addr) => {
                write!(f, "can not send address {addr}")
            }
            EcssTmtcError::CantSendDirectTm => {
                write!(f, "can not send TM directly")
            }
            EcssTmtcError::Send(e) => {
                write!(f, "ecss tmtc error: {e}")
            }
            EcssTmtcError::Receive(e) => {
                write!(f, "ecss tmtc error {e}")
            }
        }
    }
}

impl From<PoolError> for EcssTmtcError {
    fn from(value: PoolError) -> Self {
        Self::Store(value)
    }
}

impl From<PusError> for EcssTmtcError {
    fn from(value: PusError) -> Self {
        Self::Pus(value)
    }
}

impl From<GenericSendError> for EcssTmtcError {
    fn from(value: GenericSendError) -> Self {
        Self::Send(value)
    }
}

impl From<ByteConversionError> for EcssTmtcError {
    fn from(value: ByteConversionError) -> Self {
        Self::ByteConversion(value)
    }
}

impl From<GenericReceiveError> for EcssTmtcError {
    fn from(value: GenericReceiveError) -> Self {
        Self::Receive(value)
    }
}

#[cfg(feature = "std")]
impl Error for EcssTmtcError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            EcssTmtcError::Store(e) => Some(e),
            EcssTmtcError::ByteConversion(e) => Some(e),
            EcssTmtcError::Pus(e) => Some(e),
            EcssTmtcError::Send(e) => Some(e),
            EcssTmtcError::Receive(e) => Some(e),
            _ => None,
        }
    }
}
pub trait ChannelWithId: Send {
    /// Each sender can have an ID associated with it
    fn id(&self) -> ComponentId;
    fn name(&self) -> &'static str {
        "unset"
    }
}

/// Generic trait for a user supplied sender object.
///
/// This sender object is responsible for sending PUS telemetry to a TM sink.
pub trait EcssTmSender: Send {
    fn send_tm(&self, sender_id: ComponentId, tm: PusTmVariant) -> Result<(), EcssTmtcError>;
}

/// Generic trait for a user supplied sender object.
///
/// This sender object is responsible for sending PUS telecommands to a TC recipient. Each
/// telecommand can optionally have a token which contains its verification state.
pub trait EcssTcSender {
    fn send_tc(&self, tc: PusTcCreator, token: Option<TcStateToken>) -> Result<(), EcssTmtcError>;
}

/// Dummy object which can be useful for tests.
#[derive(Default)]
pub struct EcssTmDummySender {}

impl EcssTmSender for EcssTmDummySender {
    fn send_tm(&self, _source_id: ComponentId, _tm: PusTmVariant) -> Result<(), EcssTmtcError> {
        Ok(())
    }
}

/// A PUS telecommand packet can be stored in memory and sent using different methods. Right now,
/// storage inside a pool structure like [crate::pool::StaticMemoryPool], and storage inside a
/// `Vec<u8>` are supported.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TcInMemory {
    Pool(PacketInPool),
    #[cfg(feature = "alloc")]
    Vec(PacketAsVec),
}

impl From<PacketInPool> for TcInMemory {
    fn from(value: PacketInPool) -> Self {
        Self::Pool(value)
    }
}

#[cfg(feature = "alloc")]
impl From<PacketAsVec> for TcInMemory {
    fn from(value: PacketAsVec) -> Self {
        Self::Vec(value)
    }
}

/// Generic structure for an ECSS PUS Telecommand and its correspoding verification token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EcssTcAndToken {
    pub tc_in_memory: TcInMemory,
    pub token: Option<TcStateToken>,
}

impl EcssTcAndToken {
    pub fn new(tc_in_memory: impl Into<TcInMemory>, token: impl Into<TcStateToken>) -> Self {
        Self {
            tc_in_memory: tc_in_memory.into(),
            token: Some(token.into()),
        }
    }
}

/// Generic abstraction for a telecommand being sent around after is has been accepted.
pub struct AcceptedEcssTcAndToken {
    pub tc_in_memory: TcInMemory,
    pub token: VerificationToken<TcStateAccepted>,
}

impl From<AcceptedEcssTcAndToken> for EcssTcAndToken {
    fn from(value: AcceptedEcssTcAndToken) -> Self {
        EcssTcAndToken {
            tc_in_memory: value.tc_in_memory,
            token: Some(value.token.into()),
        }
    }
}

impl TryFrom<EcssTcAndToken> for AcceptedEcssTcAndToken {
    type Error = ();

    fn try_from(value: EcssTcAndToken) -> Result<Self, Self::Error> {
        if let Some(TcStateToken::Accepted(token)) = value.token {
            return Ok(AcceptedEcssTcAndToken {
                tc_in_memory: value.tc_in_memory,
                token,
            });
        }
        Err(())
    }
}

#[derive(Debug, Clone)]
pub enum TryRecvTmtcError {
    Tmtc(EcssTmtcError),
    Empty,
}

impl From<EcssTmtcError> for TryRecvTmtcError {
    fn from(value: EcssTmtcError) -> Self {
        Self::Tmtc(value)
    }
}

impl From<PusError> for TryRecvTmtcError {
    fn from(value: PusError) -> Self {
        Self::Tmtc(value.into())
    }
}

impl From<PoolError> for TryRecvTmtcError {
    fn from(value: PoolError) -> Self {
        Self::Tmtc(value.into())
    }
}

/// Generic trait for a user supplied receiver object.
pub trait EcssTcReceiver {
    fn recv_tc(&self) -> Result<EcssTcAndToken, TryRecvTmtcError>;
}

/// Generic trait for objects which can send ECSS PUS telecommands.
pub trait PacketSenderPusTc: Send {
    type Error;
    fn send_pus_tc(
        &self,
        sender_id: ComponentId,
        header: &SpHeader,
        pus_tc: &PusTcReader,
    ) -> Result<(), Self::Error>;
}

pub trait ActiveRequestStore<V>: Sized {
    fn insert(&mut self, request_id: &RequestId, request_info: V);
    fn get(&self, request_id: RequestId) -> Option<&V>;
    fn get_mut(&mut self, request_id: RequestId) -> Option<&mut V>;
    fn remove(&mut self, request_id: RequestId) -> bool;

    /// Call a user-supplied closure for each active request.
    fn for_each<F: FnMut(&RequestId, &V)>(&self, f: F);

    /// Call a user-supplied closure for each active request. Mutable variant.
    fn for_each_mut<F: FnMut(&RequestId, &mut V)>(&mut self, f: F);
}

pub trait ActiveRequest {
    fn target_id(&self) -> ComponentId;
    fn token(&self) -> TcStateToken;
    fn set_token(&mut self, token: TcStateToken);
    fn has_timed_out(&self) -> bool;
    fn timeout(&self) -> Duration;
}

/// This trait is an abstraction for the routing of PUS request to a dedicated
/// recipient using the generic [ComponentId].
pub trait PusRequestRouter<Request> {
    type Error;

    fn route(
        &self,
        requestor_info: MessageMetadata,
        target_id: ComponentId,
        request: Request,
    ) -> Result<(), Self::Error>;
}

pub trait PusReplyHandler<ActiveRequestInfo: ActiveRequest, ReplyType> {
    type Error;

    /// This function handles a reply for a given PUS request and returns whether that request
    /// is finished. A finished PUS request will be removed from the active request map.
    fn handle_reply(
        &mut self,
        reply: &GenericMessage<ReplyType>,
        active_request: &ActiveRequestInfo,
        tm_sender: &impl EcssTmSender,
        verification_handler: &impl VerificationReportingProvider,
        time_stamp: &[u8],
    ) -> Result<bool, Self::Error>;

    fn handle_unrequested_reply(
        &mut self,
        reply: &GenericMessage<ReplyType>,
        tm_sender: &impl EcssTmSender,
    ) -> Result<(), Self::Error>;

    /// Handle the timeout of an active request.
    fn handle_request_timeout(
        &mut self,
        active_request: &ActiveRequestInfo,
        tm_sender: &impl EcssTmSender,
        verification_handler: &impl VerificationReportingProvider,
        time_stamp: &[u8],
    ) -> Result<(), Self::Error>;
}

#[cfg(feature = "alloc")]
pub mod alloc_mod {
    use hashbrown::HashMap;

    use super::*;

    /// Extension trait for [EcssTmSender].
    ///
    /// It provides additional functionality, for example by implementing the [Downcast] trait
    /// and the [DynClone] trait.
    ///
    /// [Downcast] is implemented to allow passing the sender as a boxed trait object and still
    /// retrieve the concrete type at a later point.
    ///
    /// [DynClone] allows cloning the trait object as long as the boxed object implements
    /// [Clone].
    #[cfg(feature = "alloc")]
    pub trait EcssTmSenderExt: EcssTmSender + Downcast + DynClone {
        // Remove this once trait upcasting coercion has been implemented.
        // Tracking issue: https://github.com/rust-lang/rust/issues/65991
        fn upcast(&self) -> &dyn EcssTmSender;
        // Remove this once trait upcasting coercion has been implemented.
        // Tracking issue: https://github.com/rust-lang/rust/issues/65991
        fn upcast_mut(&mut self) -> &mut dyn EcssTmSender;
    }

    /// Blanket implementation for all types which implement [EcssTmSender] and are clonable.
    impl<T> EcssTmSenderExt for T
    where
        T: EcssTmSender + Clone + 'static,
    {
        // Remove this once trait upcasting coercion has been implemented.
        // Tracking issue: https://github.com/rust-lang/rust/issues/65991
        fn upcast(&self) -> &dyn EcssTmSender {
            self
        }
        // Remove this once trait upcasting coercion has been implemented.
        // Tracking issue: https://github.com/rust-lang/rust/issues/65991
        fn upcast_mut(&mut self) -> &mut dyn EcssTmSender {
            self
        }
    }

    dyn_clone::clone_trait_object!(EcssTmSenderExt);
    impl_downcast!(EcssTmSenderExt);

    /// Extension trait for [EcssTcSender].
    ///
    /// It provides additional functionality, for example by implementing the [Downcast] trait
    /// and the [DynClone] trait.
    ///
    /// [Downcast] is implemented to allow passing the sender as a boxed trait object and still
    /// retrieve the concrete type at a later point.
    ///
    /// [DynClone] allows cloning the trait object as long as the boxed object implements
    /// [Clone].
    #[cfg(feature = "alloc")]
    pub trait EcssTcSenderExt: EcssTcSender + Downcast + DynClone {}

    /// Blanket implementation for all types which implement [EcssTcSender] and are clonable.
    impl<T> EcssTcSenderExt for T where T: EcssTcSender + Clone + 'static {}

    dyn_clone::clone_trait_object!(EcssTcSenderExt);
    impl_downcast!(EcssTcSenderExt);

    /// Extension trait for [EcssTcReceiver].
    ///
    /// It provides additional functionality, for example by implementing the [Downcast] trait
    /// and the [DynClone] trait.
    ///
    /// [Downcast] is implemented to allow passing the sender as a boxed trait object and still
    /// retrieve the concrete type at a later point.
    ///
    /// [DynClone] allows cloning the trait object as long as the boxed object implements
    /// [Clone].
    #[cfg(feature = "alloc")]
    pub trait EcssTcReceiverExt: EcssTcReceiver + Downcast {}

    /// Blanket implementation for all types which implement [EcssTcReceiver] and are clonable.
    impl<T> EcssTcReceiverExt for T where T: EcssTcReceiver + 'static {}

    impl_downcast!(EcssTcReceiverExt);

    /// This trait is an abstraction for the conversion of a PUS telecommand into a generic request
    /// type.
    ///
    /// Having a dedicated trait for this allows maximum flexiblity and tailoring of the standard.
    /// The only requirement is that a valid active request information instance and a request
    /// are returned by the core conversion function. The active request type needs to fulfill
    /// the [ActiveRequest] trait bound.
    ///
    /// The user should take care of performing the error handling as well. Some of the following
    /// aspects might be relevant:
    ///
    /// - Checking the validity of the APID, service ID, subservice ID.
    /// - Checking the validity of the user data.
    ///
    /// A [VerificationReportingProvider] instance is passed to the user to also allow handling
    /// of the verification process as part of the PUS standard requirements.
    pub trait PusTcToRequestConverter<ActiveRequestInfo: ActiveRequest, Request> {
        type Error;
        fn convert(
            &mut self,
            token: VerificationToken<TcStateAccepted>,
            tc: &PusTcReader,
            tm_sender: &(impl EcssTmSender + ?Sized),
            verif_reporter: &impl VerificationReportingProvider,
            time_stamp: &[u8],
        ) -> Result<(ActiveRequestInfo, Request), Self::Error>;
    }

    #[derive(Clone, Debug)]
    pub struct DefaultActiveRequestMap<V>(pub HashMap<RequestId, V>);

    impl<V> Default for DefaultActiveRequestMap<V> {
        fn default() -> Self {
            Self(HashMap::new())
        }
    }

    impl<V> ActiveRequestStore<V> for DefaultActiveRequestMap<V> {
        fn insert(&mut self, request_id: &RequestId, request: V) {
            self.0.insert(*request_id, request);
        }

        fn get(&self, request_id: RequestId) -> Option<&V> {
            self.0.get(&request_id)
        }

        fn get_mut(&mut self, request_id: RequestId) -> Option<&mut V> {
            self.0.get_mut(&request_id)
        }

        fn remove(&mut self, request_id: RequestId) -> bool {
            self.0.remove(&request_id).is_some()
        }

        fn for_each<F: FnMut(&RequestId, &V)>(&self, mut f: F) {
            for (req_id, active_req) in &self.0 {
                f(req_id, active_req);
            }
        }

        fn for_each_mut<F: FnMut(&RequestId, &mut V)>(&mut self, mut f: F) {
            for (req_id, active_req) in &mut self.0 {
                f(req_id, active_req);
            }
        }
    }

    /*
    /// Generic reply handler structure which can be used to handle replies for a specific PUS
    /// service.
    ///
    /// This is done by keeping track of active requests using an internal map structure. An API
    /// to register new active requests is exposed as well.
    /// The reply handler performs boilerplate tasks like performing the verification handling and
    /// timeout handling.
    ///
    /// This object is not useful by itself but serves as a common building block for high-level
    /// PUS reply handlers. Concrete PUS handlers should constrain the [ActiveRequestProvider] and
    /// the `ReplyType` generics to specific types tailored towards PUS services in addition to
    /// providing an API which can process received replies and convert them into verification
    /// completions or other operation like user hook calls. The framework also provides some
    /// concrete PUS handlers for common PUS services like the mode, action and housekeeping
    /// service.
    ///
    /// This object does not automatically update its internal time information used to check for
    /// timeouts. The user should call the [Self::update_time] and [Self::update_time_from_now]
    /// methods to do this.
    pub struct PusServiceReplyHandler<
        ActiveRequestMap: ActiveRequestMapProvider<ActiveRequestType>,
        ReplyHook: ReplyHandlerHook<ActiveRequestType, ReplyType>,
        ActiveRequestType: ActiveRequestProvider,
        ReplyType,
    > {
        pub active_request_map: ActiveRequestMap,
        pub tm_buf: alloc::vec::Vec<u8>,
        pub current_time: UnixTimestamp,
        pub user_hook: ReplyHook,
        phantom: PhantomData<(ActiveRequestType, ReplyType)>,
    }

    impl<
            ActiveRequestMap: ActiveRequestMapProvider<ActiveRequestType>,
            ReplyHook: ReplyHandlerHook<ActiveRequestType, ReplyType>,
            ActiveRequestType: ActiveRequestProvider,
            ReplyType,
        >
        PusServiceReplyHandler<
            ActiveRequestMap,
            ReplyHook,
            ActiveRequestType,
            ReplyType,
        >
    {
        #[cfg(feature = "std")]
        pub fn new_from_now(
            active_request_map: ActiveRequestMap,
            fail_data_buf_size: usize,
            user_hook: ReplyHook,
        ) -> Result<Self, std::time::SystemTimeError> {
            let current_time = UnixTimestamp::from_now()?;
            Ok(Self::new(
                active_request_map,
                fail_data_buf_size,
                user_hook,
                tm_sender,
                current_time,
            ))
        }

        pub fn new(
            active_request_map: ActiveRequestMap,
            fail_data_buf_size: usize,
            user_hook: ReplyHook,
            tm_sender: TmSender,
            init_time: UnixTimestamp,
        ) -> Self {
            Self {
                active_request_map,
                tm_buf: alloc::vec![0; fail_data_buf_size],
                current_time: init_time,
                user_hook,
                tm_sender,
                phantom: PhantomData,
            }
        }

        pub fn add_routed_request(
            &mut self,
            request_id: verification::RequestId,
            active_request_type: ActiveRequestType,
        ) {
            self.active_request_map
                .insert(&request_id.into(), active_request_type);
        }

        pub fn request_active(&self, request_id: RequestId) -> bool {
            self.active_request_map.get(request_id).is_some()
        }

        /// Check for timeouts across all active requests.
        ///
        /// It will call [Self::handle_timeout] for all active requests which have timed out.
        pub fn check_for_timeouts(&mut self, time_stamp: &[u8]) -> Result<(), EcssTmtcError> {
            let mut timed_out_commands = alloc::vec::Vec::new();
            self.active_request_map.for_each(|request_id, active_req| {
                let diff = self.current_time - active_req.start_time();
                if diff.duration_absolute > active_req.timeout() {
                    self.handle_timeout(active_req, time_stamp);
                }
                timed_out_commands.push(*request_id);
            });
            for timed_out_command in timed_out_commands {
                self.active_request_map.remove(timed_out_command);
            }
            Ok(())
        }

        /// Handle the timeout for a given active request.
        ///
        /// This implementation will report a verification completion failure with a user-provided
        /// error code. It supplies the configured request timeout in milliseconds as a [u64]
        /// serialized in big-endian format as the failure data.
        pub fn handle_timeout(&self, active_request: &ActiveRequestType, time_stamp: &[u8]) {
            let timeout = active_request.timeout().as_millis() as u64;
            let timeout_raw = timeout.to_be_bytes();
            self.verification_reporter
                .completion_failure(
                    active_request.token(),
                    FailParams::new(
                        time_stamp,
                        &self.user_hook.timeout_error_code(),
                        &timeout_raw,
                    ),
                )
                .unwrap();
            self.user_hook.timeout_callback(active_request);
        }

        /// Update the current time used for timeout checks based on the current OS time.
        #[cfg(feature = "std")]
        pub fn update_time_from_now(&mut self) -> Result<(), std::time::SystemTimeError> {
            self.current_time = UnixTimestamp::from_now()?;
            Ok(())
        }

        /// Update the current time used for timeout checks.
        pub fn update_time(&mut self, time: UnixTimestamp) {
            self.current_time = time;
        }
    }
    */
}

#[cfg(feature = "std")]
pub mod std_mod {
    use super::verification::{TcStateAccepted, VerificationToken};
    use super::*;
    use crate::ComponentId;
    use crate::HandlingStatus;
    use crate::pool::{
        PoolAddr, PoolError, PoolProvider, PoolProviderWithGuards, SharedStaticMemoryPool,
    };
    use crate::tmtc::{PacketAsVec, PacketSenderWithSharedPool};
    use alloc::vec::Vec;
    use core::time::Duration;
    use spacepackets::ByteConversionError;
    use spacepackets::ecss::WritablePusPacket;
    use spacepackets::ecss::tc::PusTcReader;
    use spacepackets::time::StdTimestampError;
    use std::string::String;
    use std::sync::mpsc;
    use std::sync::mpsc::TryRecvError;
    use thiserror::Error;

    #[cfg(feature = "crossbeam")]
    pub use cb_mod::*;

    use super::verification::{TcStateToken, VerificationReportingProvider};
    use super::{AcceptedEcssTcAndToken, ActiveRequest, TcInMemory};
    use crate::tmtc::PacketInPool;

    impl From<mpsc::SendError<PoolAddr>> for EcssTmtcError {
        fn from(_: mpsc::SendError<PoolAddr>) -> Self {
            Self::Send(GenericSendError::RxDisconnected)
        }
    }

    impl EcssTmSender for mpsc::Sender<PacketInPool> {
        fn send_tm(&self, source_id: ComponentId, tm: PusTmVariant) -> Result<(), EcssTmtcError> {
            match tm {
                PusTmVariant::InStore(store_addr) => self
                    .send(PacketInPool {
                        sender_id: source_id,
                        store_addr,
                    })
                    .map_err(|_| GenericSendError::RxDisconnected)?,
                PusTmVariant::Direct(_) => return Err(EcssTmtcError::CantSendDirectTm),
            };
            Ok(())
        }
    }

    impl EcssTmSender for mpsc::SyncSender<PacketInPool> {
        fn send_tm(&self, source_id: ComponentId, tm: PusTmVariant) -> Result<(), EcssTmtcError> {
            match tm {
                PusTmVariant::InStore(store_addr) => self
                    .try_send(PacketInPool {
                        sender_id: source_id,
                        store_addr,
                    })
                    .map_err(|e| EcssTmtcError::Send(e.into()))?,
                PusTmVariant::Direct(_) => return Err(EcssTmtcError::CantSendDirectTm),
            };
            Ok(())
        }
    }

    pub type MpscTmAsVecSender = mpsc::Sender<PacketAsVec>;

    impl EcssTmSender for MpscTmAsVecSender {
        fn send_tm(&self, source_id: ComponentId, tm: PusTmVariant) -> Result<(), EcssTmtcError> {
            match tm {
                PusTmVariant::InStore(addr) => return Err(EcssTmtcError::CantSendAddr(addr)),
                PusTmVariant::Direct(tm) => self
                    .send(PacketAsVec {
                        sender_id: source_id,
                        packet: tm.to_vec()?,
                    })
                    .map_err(|e| EcssTmtcError::Send(e.into()))?,
            };
            Ok(())
        }
    }

    pub type MpscTmAsVecSenderBounded = mpsc::SyncSender<PacketAsVec>;

    impl EcssTmSender for MpscTmAsVecSenderBounded {
        fn send_tm(&self, source_id: ComponentId, tm: PusTmVariant) -> Result<(), EcssTmtcError> {
            match tm {
                PusTmVariant::InStore(addr) => return Err(EcssTmtcError::CantSendAddr(addr)),
                PusTmVariant::Direct(tm) => self
                    .send(PacketAsVec {
                        sender_id: source_id,
                        packet: tm.to_vec()?,
                    })
                    .map_err(|e| EcssTmtcError::Send(e.into()))?,
            };
            Ok(())
        }
    }

    pub type MpscTcReceiver = mpsc::Receiver<EcssTcAndToken>;

    impl EcssTcReceiver for MpscTcReceiver {
        fn recv_tc(&self) -> Result<EcssTcAndToken, TryRecvTmtcError> {
            self.try_recv().map_err(|e| match e {
                TryRecvError::Empty => TryRecvTmtcError::Empty,
                TryRecvError::Disconnected => TryRecvTmtcError::Tmtc(EcssTmtcError::from(
                    GenericReceiveError::TxDisconnected(None),
                )),
            })
        }
    }

    #[cfg(feature = "crossbeam")]
    pub mod cb_mod {
        use super::*;
        use crossbeam_channel as cb;

        impl From<cb::SendError<PoolAddr>> for EcssTmtcError {
            fn from(_: cb::SendError<PoolAddr>) -> Self {
                Self::Send(GenericSendError::RxDisconnected)
            }
        }

        impl From<cb::TrySendError<PoolAddr>> for EcssTmtcError {
            fn from(value: cb::TrySendError<PoolAddr>) -> Self {
                match value {
                    cb::TrySendError::Full(_) => Self::Send(GenericSendError::QueueFull(None)),
                    cb::TrySendError::Disconnected(_) => {
                        Self::Send(GenericSendError::RxDisconnected)
                    }
                }
            }
        }

        impl EcssTmSender for cb::Sender<PacketInPool> {
            fn send_tm(
                &self,
                sender_id: ComponentId,
                tm: PusTmVariant,
            ) -> Result<(), EcssTmtcError> {
                match tm {
                    PusTmVariant::InStore(addr) => self
                        .try_send(PacketInPool::new(sender_id, addr))
                        .map_err(|e| EcssTmtcError::Send(e.into()))?,
                    PusTmVariant::Direct(_) => return Err(EcssTmtcError::CantSendDirectTm),
                };
                Ok(())
            }
        }
        impl EcssTmSender for cb::Sender<PacketAsVec> {
            fn send_tm(
                &self,
                sender_id: ComponentId,
                tm: PusTmVariant,
            ) -> Result<(), EcssTmtcError> {
                match tm {
                    PusTmVariant::InStore(addr) => return Err(EcssTmtcError::CantSendAddr(addr)),
                    PusTmVariant::Direct(tm) => self
                        .send(PacketAsVec::new(sender_id, tm.to_vec()?))
                        .map_err(|e| EcssTmtcError::Send(e.into()))?,
                };
                Ok(())
            }
        }

        pub type CrossbeamTcReceiver = cb::Receiver<EcssTcAndToken>;
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ActivePusRequestStd {
        target_id: ComponentId,
        token: TcStateToken,
        start_time: std::time::Instant,
        timeout: Duration,
    }

    impl ActivePusRequestStd {
        pub fn new(
            target_id: ComponentId,
            token: impl Into<TcStateToken>,
            timeout: Duration,
        ) -> Self {
            Self {
                target_id,
                token: token.into(),
                start_time: std::time::Instant::now(),
                timeout,
            }
        }
    }

    impl ActiveRequest for ActivePusRequestStd {
        fn target_id(&self) -> ComponentId {
            self.target_id
        }

        fn token(&self) -> TcStateToken {
            self.token
        }

        fn timeout(&self) -> Duration {
            self.timeout
        }
        fn set_token(&mut self, token: TcStateToken) {
            self.token = token;
        }

        fn has_timed_out(&self) -> bool {
            std::time::Instant::now() - self.start_time > self.timeout
        }
    }

    // TODO: All these types could probably be no_std if we implemented error handling ourselves..
    // but thiserror is really nice, so keep it like this for simplicity for now. Maybe thiserror
    // will be no_std soon, see https://github.com/rust-lang/rust/issues/103765 .

    #[derive(Debug, Clone, Error)]
    pub enum PusTcFromMemError {
        #[error("generic PUS error: {0}")]
        EcssTmtc(#[from] EcssTmtcError),
        #[error("invalid format of TC in memory: {0:?}")]
        InvalidFormat(TcInMemory),
    }

    #[derive(Debug, Clone, Error)]
    pub enum GenericRoutingError {
        // #[error("not enough application data, expected at least {expected}, found {found}")]
        // NotEnoughAppData { expected: usize, found: usize },
        #[error("Unknown target ID {0}")]
        UnknownTargetId(ComponentId),
        #[error("Sending action request failed: {0}")]
        Send(GenericSendError),
    }

    /// This error can be used for generic conversions from PUS Telecommands to request types.
    ///
    /// Please note that this error can also be used if no request is generated and the PUS
    /// service, subservice and application data is used directly to perform some request.
    #[derive(Debug, Clone, Error)]
    pub enum GenericConversionError {
        #[error("wrong service number {0} for packet handler")]
        WrongService(u8),
        #[error("invalid subservice {0}")]
        InvalidSubservice(u8),
        #[error("not enough application data, expected at least {expected}, found {found}")]
        NotEnoughAppData { expected: usize, found: usize },
        #[error("invalid application data")]
        InvalidAppData(String),
    }

    /// Wrapper type which tries to encapsulate all possible errors when handling PUS packets.
    #[derive(Debug, Clone, Error)]
    pub enum PusPacketHandlingError {
        #[error("error polling PUS TC packet: {0}")]
        TcPolling(#[from] EcssTmtcError),
        #[error("error generating PUS reader from memory: {0}")]
        TcFromMem(#[from] PusTcFromMemError),
        #[error("generic request conversion error: {0}")]
        RequestConversion(#[from] GenericConversionError),
        #[error("request routing error: {0}")]
        RequestRouting(#[from] GenericRoutingError),
        #[error("invalid verification token")]
        InvalidVerificationToken,
        #[error("other error {0}")]
        Other(String),
    }

    #[derive(Debug, Clone, Error)]
    pub enum PartialPusHandlingError {
        #[error("generic timestamp generation error")]
        Time(#[from] StdTimestampError),
        #[error("error sending telemetry: {0}")]
        TmSend(EcssTmtcError),
        #[error("error sending verification message")]
        Verification(EcssTmtcError),
        #[error("invalid verification token")]
        NoVerificationToken,
    }

    /// Generic result type for handlers which can process PUS packets.
    #[derive(Debug, Clone)]
    pub enum DirectPusPacketHandlerResult {
        Handled(HandlingStatus),
        SubserviceNotImplemented(u8, VerificationToken<TcStateAccepted>),
        CustomSubservice(u8, VerificationToken<TcStateAccepted>),
    }

    impl From<HandlingStatus> for DirectPusPacketHandlerResult {
        fn from(value: HandlingStatus) -> Self {
            Self::Handled(value)
        }
    }

    /// This trait provides an abstraction for caching a raw ECSS telecommand and then
    /// providing the [PusTcReader] abstraction to read the cache raw telecommand.
    pub trait CacheAndReadRawEcssTc {
        fn cache(&mut self, possible_packet: &TcInMemory) -> Result<(), PusTcFromMemError>;

        fn tc_slice_raw(&self) -> &[u8];

        fn sender_id(&self) -> Option<ComponentId>;

        fn cache_and_convert(
            &mut self,
            possible_packet: &TcInMemory,
        ) -> Result<PusTcReader<'_>, PusTcFromMemError> {
            self.cache(possible_packet)?;
            Ok(PusTcReader::new(self.tc_slice_raw()).map_err(EcssTmtcError::Pus)?)
        }

        fn convert(&self) -> Result<PusTcReader<'_>, PusTcFromMemError> {
            Ok(PusTcReader::new(self.tc_slice_raw()).map_err(EcssTmtcError::Pus)?)
        }
    }

    /// Converter structure for PUS telecommands which are stored inside a `Vec<u8>` structure.
    /// Please note that this structure is not able to convert TCs which are stored inside a
    /// [SharedStaticMemoryPool].
    #[derive(Default, Clone)]
    pub struct EcssTcVecCacher {
        sender_id: Option<ComponentId>,
        pub pus_tc_raw: Option<Vec<u8>>,
    }

    impl CacheAndReadRawEcssTc for EcssTcVecCacher {
        fn cache(&mut self, tc_in_memory: &TcInMemory) -> Result<(), PusTcFromMemError> {
            self.pus_tc_raw = None;
            match tc_in_memory {
                super::TcInMemory::Pool(_packet_in_pool) => {
                    return Err(PusTcFromMemError::InvalidFormat(tc_in_memory.clone()));
                }
                super::TcInMemory::Vec(packet_with_sender) => {
                    self.pus_tc_raw = Some(packet_with_sender.packet.clone());
                    self.sender_id = Some(packet_with_sender.sender_id);
                }
            };
            Ok(())
        }

        fn sender_id(&self) -> Option<ComponentId> {
            self.sender_id
        }

        fn tc_slice_raw(&self) -> &[u8] {
            if self.pus_tc_raw.is_none() {
                return &[];
            }
            self.pus_tc_raw.as_ref().unwrap()
        }
    }

    /// Converter structure for PUS telecommands which are stored inside
    /// [SharedStaticMemoryPool] structure. This is useful if run-time allocation for these
    /// packets should be avoided. Please note that this structure is not able to convert TCs which
    /// are stored as a `Vec<u8>`.
    #[derive(Clone)]
    pub struct EcssTcInSharedPoolCacher {
        sender_id: Option<ComponentId>,
        shared_tc_pool: SharedStaticMemoryPool,
        pus_buf: Vec<u8>,
    }

    impl EcssTcInSharedPoolCacher {
        pub fn new(shared_tc_store: SharedStaticMemoryPool, max_expected_tc_size: usize) -> Self {
            Self {
                sender_id: None,
                shared_tc_pool: shared_tc_store,
                pus_buf: alloc::vec![0; max_expected_tc_size],
            }
        }

        pub fn copy_tc_to_buf(&mut self, addr: PoolAddr) -> Result<(), PusTcFromMemError> {
            // Keep locked section as short as possible.
            let mut tc_pool = self.shared_tc_pool.write().map_err(|_| {
                PusTcFromMemError::EcssTmtc(EcssTmtcError::Store(PoolError::LockError))
            })?;
            let tc_size = tc_pool.len_of_data(&addr).map_err(EcssTmtcError::Store)?;
            if tc_size > self.pus_buf.len() {
                return Err(
                    EcssTmtcError::ByteConversion(ByteConversionError::ToSliceTooSmall {
                        found: self.pus_buf.len(),
                        expected: tc_size,
                    })
                    .into(),
                );
            }
            let tc_guard = tc_pool.read_with_guard(addr);
            // TODO: Proper error handling.
            tc_guard.read(&mut self.pus_buf[0..tc_size]).unwrap();
            Ok(())
        }
    }

    impl CacheAndReadRawEcssTc for EcssTcInSharedPoolCacher {
        fn cache(&mut self, tc_in_memory: &TcInMemory) -> Result<(), PusTcFromMemError> {
            match tc_in_memory {
                super::TcInMemory::Pool(packet_in_pool) => {
                    self.copy_tc_to_buf(packet_in_pool.store_addr)?;
                    self.sender_id = Some(packet_in_pool.sender_id);
                }
                super::TcInMemory::Vec(_) => {
                    return Err(PusTcFromMemError::InvalidFormat(tc_in_memory.clone()));
                }
            };
            Ok(())
        }

        fn tc_slice_raw(&self) -> &[u8] {
            self.pus_buf.as_ref()
        }

        fn sender_id(&self) -> Option<ComponentId> {
            self.sender_id
        }
    }

    // TODO: alloc feature flag?
    #[derive(Clone)]
    pub enum EcssTcCacher {
        Static(EcssTcInSharedPoolCacher),
        Heap(EcssTcVecCacher),
    }

    impl EcssTcCacher {
        pub fn new_static(static_store_converter: EcssTcInSharedPoolCacher) -> Self {
            Self::Static(static_store_converter)
        }

        pub fn new_heap(heap_converter: EcssTcVecCacher) -> Self {
            Self::Heap(heap_converter)
        }
    }

    impl CacheAndReadRawEcssTc for EcssTcCacher {
        fn cache(&mut self, tc_in_memory: &TcInMemory) -> Result<(), PusTcFromMemError> {
            match self {
                Self::Static(converter) => converter.cache(tc_in_memory),
                Self::Heap(converter) => converter.cache(tc_in_memory),
            }
        }
        fn tc_slice_raw(&self) -> &[u8] {
            match self {
                Self::Static(converter) => converter.tc_slice_raw(),
                Self::Heap(converter) => converter.tc_slice_raw(),
            }
        }
        fn sender_id(&self) -> Option<ComponentId> {
            match self {
                Self::Static(converter) => converter.sender_id(),
                Self::Heap(converter) => converter.sender_id(),
            }
        }
    }

    pub struct PusServiceBase<
        TcReceiver: EcssTcReceiver,
        TmSender: EcssTmSender,
        VerificationReporter: VerificationReportingProvider,
    > {
        pub id: ComponentId,
        pub tc_receiver: TcReceiver,
        pub tm_sender: TmSender,
        pub verif_reporter: VerificationReporter,
    }

    /// This is a high-level PUS packet handler helper.
    ///
    /// It performs some of the boilerplate acitivities involved when handling PUS telecommands and
    /// it can be used to implement the handling of PUS telecommands for certain PUS telecommands
    /// groups (for example individual services).
    ///
    /// This base class can handle PUS telecommands backed by different memory storage machanisms
    /// by using the [CacheAndReadRawEcssTc] abstraction. This object provides some convenience
    /// methods to make the generic parts of TC handling easier.
    pub struct PusServiceHelper<
        TcReceiver: EcssTcReceiver,
        TmSender: EcssTmSender,
        TcInMemConverter: CacheAndReadRawEcssTc,
        VerificationReporter: VerificationReportingProvider,
    > {
        pub common: PusServiceBase<TcReceiver, TmSender, VerificationReporter>,
        pub tc_in_mem_converter: TcInMemConverter,
    }

    impl<
        TcReceiver: EcssTcReceiver,
        TmSender: EcssTmSender,
        TcInMemConverter: CacheAndReadRawEcssTc,
        VerificationReporter: VerificationReportingProvider,
    > PusServiceHelper<TcReceiver, TmSender, TcInMemConverter, VerificationReporter>
    {
        pub fn new(
            id: ComponentId,
            tc_receiver: TcReceiver,
            tm_sender: TmSender,
            verification_handler: VerificationReporter,
            tc_in_mem_converter: TcInMemConverter,
        ) -> Self {
            Self {
                common: PusServiceBase {
                    id,
                    tc_receiver,
                    tm_sender,
                    verif_reporter: verification_handler,
                },
                tc_in_mem_converter,
            }
        }

        pub fn id(&self) -> ComponentId {
            self.common.id
        }

        pub fn tm_sender(&self) -> &TmSender {
            &self.common.tm_sender
        }

        /// This function can be used to poll the internal [EcssTcReceiver] object for the next
        /// telecommand packet. It will return `Ok(None)` if there are not packets available.
        /// In any other case, it will perform the acceptance of the ECSS TC packet using the
        /// internal [VerificationReportingProvider] object. It will then return the telecommand
        /// and the according accepted token.
        pub fn retrieve_and_accept_next_packet(
            &mut self,
        ) -> Result<Option<AcceptedEcssTcAndToken>, PusPacketHandlingError> {
            match self.common.tc_receiver.recv_tc() {
                Ok(EcssTcAndToken {
                    tc_in_memory,
                    token,
                }) => {
                    if token.is_none() {
                        return Err(PusPacketHandlingError::InvalidVerificationToken);
                    }
                    let token = token.unwrap();
                    let accepted_token = VerificationToken::<TcStateAccepted>::try_from(token)
                        .map_err(|_| PusPacketHandlingError::InvalidVerificationToken)?;
                    Ok(Some(AcceptedEcssTcAndToken {
                        tc_in_memory,
                        token: accepted_token,
                    }))
                }
                Err(e) => match e {
                    TryRecvTmtcError::Tmtc(e) => Err(PusPacketHandlingError::TcPolling(e)),
                    TryRecvTmtcError::Empty => Ok(None),
                },
            }
        }

        pub fn verif_reporter(&self) -> &VerificationReporter {
            &self.common.verif_reporter
        }
        pub fn verif_reporter_mut(&mut self) -> &mut VerificationReporter {
            &mut self.common.verif_reporter
        }

        pub fn tc_in_mem_converter(&self) -> &TcInMemConverter {
            &self.tc_in_mem_converter
        }

        pub fn tc_in_mem_converter_mut(&mut self) -> &mut TcInMemConverter {
            &mut self.tc_in_mem_converter
        }
    }

    pub type PusServiceHelperDynWithMpsc<TcInMemConverter, VerificationReporter> =
        PusServiceHelper<MpscTcReceiver, MpscTmAsVecSender, TcInMemConverter, VerificationReporter>;
    pub type PusServiceHelperDynWithBoundedMpsc<TcInMemConverter, VerificationReporter> =
        PusServiceHelper<
            MpscTcReceiver,
            MpscTmAsVecSenderBounded,
            TcInMemConverter,
            VerificationReporter,
        >;
    pub type PusServiceHelperStaticWithMpsc<TcInMemConverter, VerificationReporter> =
        PusServiceHelper<
            MpscTcReceiver,
            PacketSenderWithSharedPool,
            TcInMemConverter,
            VerificationReporter,
        >;
    pub type PusServiceHelperStaticWithBoundedMpsc<TcInMemConverter, VerificationReporter> =
        PusServiceHelper<
            MpscTcReceiver,
            PacketSenderWithSharedPool,
            TcInMemConverter,
            VerificationReporter,
        >;
}

pub(crate) fn source_buffer_large_enough(
    cap: usize,
    len: usize,
) -> Result<(), ByteConversionError> {
    if len > cap {
        return Err(ByteConversionError::ToSliceTooSmall {
            found: cap,
            expected: len,
        });
    }
    Ok(())
}

#[cfg(any(feature = "test_util", test))]
pub mod test_util {
    use arbitrary_int::{u11, u21};
    use spacepackets::ecss::{tc::PusTcCreator, tm::PusTmReader};

    use crate::request::UniqueApidTargetId;

    use super::{
        DirectPusPacketHandlerResult, PusPacketHandlingError,
        verification::{self, TcStateAccepted, VerificationToken},
    };

    pub const TEST_APID: u11 = u11::new(0x101);
    pub const TEST_UNIQUE_ID_0: u21 = u21::new(0x05);
    pub const TEST_UNIQUE_ID_1: u21 = u21::new(0x06);

    pub const TEST_COMPONENT_ID_0: UniqueApidTargetId =
        UniqueApidTargetId::new(TEST_APID, TEST_UNIQUE_ID_0);
    pub const TEST_COMPONENT_ID_1: UniqueApidTargetId =
        UniqueApidTargetId::new(TEST_APID, TEST_UNIQUE_ID_1);

    pub trait PusTestHarness {
        fn start_verification(&mut self, tc: &PusTcCreator) -> VerificationToken<TcStateAccepted>;
        fn send_tc(&self, token: &VerificationToken<TcStateAccepted>, tc: &PusTcCreator);
        fn read_next_tm(&mut self) -> PusTmReader<'_>;
        fn check_no_tm_available(&self) -> bool;
        fn check_next_verification_tm(
            &self,
            subservice: u8,
            expected_request_id: verification::RequestId,
        );
    }

    pub trait SimplePusPacketHandler {
        fn handle_one_tc(&mut self)
        -> Result<DirectPusPacketHandlerResult, PusPacketHandlingError>;
    }
}

#[cfg(test)]
pub mod tests {
    use core::cell::RefCell;
    use std::sync::mpsc::TryRecvError;
    use std::sync::{RwLock, mpsc};

    use alloc::collections::VecDeque;
    use alloc::vec::Vec;
    use arbitrary_int::{u11, u14};
    use satrs_shared::res_code::ResultU16;
    use spacepackets::CcsdsPacket;
    use spacepackets::ecss::tc::{PusTcCreator, PusTcReader};
    use spacepackets::ecss::tm::{GenericPusTmSecondaryHeader, PusTmCreator, PusTmReader};
    use spacepackets::ecss::{PusPacket, WritablePusPacket};
    use test_util::{TEST_APID, TEST_COMPONENT_ID_0};

    use super::verification::{RequestId, VerificationReporter};
    use crate::ComponentId;
    use crate::pool::{PoolProvider, SharedStaticMemoryPool, StaticMemoryPool, StaticPoolConfig};
    use crate::tmtc::{PacketAsVec, PacketInPool, PacketSenderWithSharedPool, SharedPacketPool};

    use super::verification::test_util::TestVerificationReporter;
    use super::verification::{
        TcStateAccepted, VerificationReporterConfig, VerificationReportingProvider,
        VerificationToken,
    };
    use super::*;

    #[derive(Debug, Eq, PartialEq, Clone)]
    pub(crate) struct CommonTmInfo {
        pub subservice: u8,
        pub apid: u11,
        pub seq_count: u14,
        pub msg_counter: u16,
        pub dest_id: u16,
        pub timestamp: Vec<u8>,
    }

    impl CommonTmInfo {
        pub fn new(
            subservice: u8,
            apid: u11,
            seq_count: u14,
            msg_counter: u16,
            dest_id: u16,
            timestamp: &[u8],
        ) -> Self {
            Self {
                subservice,
                apid,
                seq_count,
                msg_counter,
                dest_id,
                timestamp: timestamp.to_vec(),
            }
        }
        pub fn new_zero_seq_count(
            subservice: u8,
            apid: u11,
            dest_id: u16,
            timestamp: &[u8],
        ) -> Self {
            Self::new(subservice, apid, u14::new(0), 0, dest_id, timestamp)
        }

        pub fn new_from_tm(tm: &PusTmCreator) -> Self {
            let mut timestamp = [0; 7];
            timestamp.clone_from_slice(&tm.timestamp()[0..7]);
            Self {
                subservice: PusPacket::message_subtype_id(tm),
                apid: tm.apid(),
                seq_count: tm.seq_count(),
                msg_counter: tm.msg_type_counter(),
                dest_id: tm.dest_id(),
                timestamp: timestamp.to_vec(),
            }
        }
    }

    /// Common fields for a PUS service test harness.
    pub struct PusServiceHandlerWithSharedStoreCommon {
        pus_buf: RefCell<[u8; 2048]>,
        tm_buf: [u8; 2048],
        tc_pool: SharedStaticMemoryPool,
        tm_pool: SharedPacketPool,
        tc_sender: mpsc::SyncSender<EcssTcAndToken>,
        tm_receiver: mpsc::Receiver<PacketInPool>,
    }

    pub type PusServiceHelperStatic = PusServiceHelper<
        MpscTcReceiver,
        PacketSenderWithSharedPool,
        EcssTcInSharedPoolCacher,
        VerificationReporter,
    >;

    impl PusServiceHandlerWithSharedStoreCommon {
        /// This function generates the structure in addition to the PUS service handler
        /// [PusServiceHandler] which might be required for a specific PUS service handler.
        ///
        /// The PUS service handler is instantiated with a [EcssTcInStoreConverter].
        pub fn new(id: ComponentId) -> (Self, PusServiceHelperStatic) {
            let pool_cfg = StaticPoolConfig::new_from_subpool_cfg_tuples(
                alloc::vec![(16, 16), (8, 32), (4, 64)],
                false,
            );
            let tc_pool = StaticMemoryPool::new(pool_cfg.clone());
            let tm_pool = StaticMemoryPool::new(pool_cfg);
            let shared_tc_pool = SharedStaticMemoryPool::new(RwLock::new(tc_pool));
            let shared_tm_pool = SharedStaticMemoryPool::new(RwLock::new(tm_pool));
            let shared_tm_pool_wrapper = SharedPacketPool::new(&shared_tm_pool);
            let (test_srv_tc_tx, test_srv_tc_rx) = mpsc::sync_channel(10);
            let (tm_tx, tm_rx) = mpsc::sync_channel(10);

            let verif_cfg = VerificationReporterConfig::new(TEST_APID, 1, 2, 8);
            let verification_handler =
                VerificationReporter::new(TEST_COMPONENT_ID_0.id(), &verif_cfg);
            let test_srv_tm_sender =
                PacketSenderWithSharedPool::new(tm_tx, shared_tm_pool_wrapper.clone());
            let in_store_converter = EcssTcInSharedPoolCacher::new(shared_tc_pool.clone(), 2048);
            (
                Self {
                    pus_buf: RefCell::new([0; 2048]),
                    tm_buf: [0; 2048],
                    tc_pool: shared_tc_pool,
                    tm_pool: shared_tm_pool_wrapper,
                    tc_sender: test_srv_tc_tx,
                    tm_receiver: tm_rx,
                },
                PusServiceHelper::new(
                    id,
                    test_srv_tc_rx,
                    test_srv_tm_sender,
                    verification_handler,
                    in_store_converter,
                ),
            )
        }
        pub fn send_tc(
            &self,
            sender_id: ComponentId,
            token: &VerificationToken<TcStateAccepted>,
            tc: &PusTcCreator,
        ) {
            let mut mut_buf = self.pus_buf.borrow_mut();
            let tc_size = tc.write_to_bytes(mut_buf.as_mut_slice()).unwrap();
            let mut tc_pool = self.tc_pool.write().unwrap();
            let addr = tc_pool.add(&mut_buf[..tc_size]).unwrap();
            drop(tc_pool);
            // Send accepted TC to test service handler.
            self.tc_sender
                .send(EcssTcAndToken::new(
                    PacketInPool::new(sender_id, addr),
                    *token,
                ))
                .expect("sending tc failed");
        }

        pub fn read_next_tm(&mut self) -> PusTmReader<'_> {
            let next_msg = self.tm_receiver.try_recv();
            assert!(next_msg.is_ok());
            let tm_in_pool = next_msg.unwrap();
            let tm_pool = self.tm_pool.0.read().unwrap();
            let tm_raw = tm_pool.read_as_vec(&tm_in_pool.store_addr).unwrap();
            self.tm_buf[0..tm_raw.len()].copy_from_slice(&tm_raw);
            PusTmReader::new(&self.tm_buf, 7).unwrap()
        }

        pub fn check_no_tm_available(&self) -> bool {
            let next_msg = self.tm_receiver.try_recv();
            if let TryRecvError::Empty = next_msg.unwrap_err() {
                return true;
            }
            false
        }

        pub fn check_next_verification_tm(&self, subservice: u8, expected_request_id: RequestId) {
            let next_msg = self.tm_receiver.try_recv();
            assert!(next_msg.is_ok());
            let tm_in_pool = next_msg.unwrap();
            let tm_pool = self.tm_pool.0.read().unwrap();
            let tm_raw = tm_pool.read_as_vec(&tm_in_pool.store_addr).unwrap();
            let tm = PusTmReader::new(&tm_raw, 7).unwrap();
            assert_eq!(PusPacket::service_type_id(&tm), 1);
            assert_eq!(PusPacket::message_subtype_id(&tm), subservice);
            assert_eq!(tm.apid(), TEST_APID);
            let req_id =
                RequestId::from_bytes(tm.user_data()).expect("generating request ID failed");
            assert_eq!(req_id, expected_request_id);
        }
    }

    pub struct PusServiceHandlerWithVecCommon {
        current_tm: Option<Vec<u8>>,
        tc_sender: mpsc::Sender<EcssTcAndToken>,
        tm_receiver: mpsc::Receiver<PacketAsVec>,
    }
    pub type PusServiceHelperDynamic =
        PusServiceHelper<MpscTcReceiver, MpscTmAsVecSender, EcssTcVecCacher, VerificationReporter>;

    impl PusServiceHandlerWithVecCommon {
        pub fn new_with_standard_verif_reporter(
            id: ComponentId,
        ) -> (Self, PusServiceHelperDynamic) {
            let (test_srv_tc_tx, test_srv_tc_rx) = mpsc::channel();
            let (tm_tx, tm_rx) = mpsc::channel();

            let verif_cfg = VerificationReporterConfig::new(TEST_APID, 1, 2, 8);
            let verification_handler =
                VerificationReporter::new(TEST_COMPONENT_ID_0.id(), &verif_cfg);
            let in_store_converter = EcssTcVecCacher::default();
            (
                Self {
                    current_tm: None,
                    tc_sender: test_srv_tc_tx,
                    tm_receiver: tm_rx,
                },
                PusServiceHelper::new(
                    id,
                    test_srv_tc_rx,
                    tm_tx,
                    verification_handler,
                    in_store_converter,
                ),
            )
        }
    }

    impl PusServiceHandlerWithVecCommon {
        pub fn new_with_test_verif_sender(
            id: ComponentId,
        ) -> (
            Self,
            PusServiceHelper<
                MpscTcReceiver,
                MpscTmAsVecSender,
                EcssTcVecCacher,
                TestVerificationReporter,
            >,
        ) {
            let (test_srv_tc_tx, test_srv_tc_rx) = mpsc::channel();
            let (tm_tx, tm_rx) = mpsc::channel();

            let in_store_converter = EcssTcVecCacher::default();
            let verification_handler = TestVerificationReporter::new(id);
            (
                Self {
                    current_tm: None,
                    tc_sender: test_srv_tc_tx,
                    tm_receiver: tm_rx,
                    //verification_handler: verification_handler.clone(),
                },
                PusServiceHelper::new(
                    id,
                    test_srv_tc_rx,
                    tm_tx,
                    verification_handler,
                    in_store_converter,
                ),
            )
        }
    }

    impl PusServiceHandlerWithVecCommon {
        pub fn send_tc(
            &self,
            sender_id: ComponentId,
            token: &VerificationToken<TcStateAccepted>,
            tc: &PusTcCreator,
        ) {
            // Send accepted TC to test service handler.
            self.tc_sender
                .send(EcssTcAndToken::new(
                    TcInMemory::Vec(PacketAsVec::new(
                        sender_id,
                        tc.to_vec().expect("pus tc conversion to vec failed"),
                    )),
                    *token,
                ))
                .expect("sending tc failed");
        }

        pub fn read_next_tm(&mut self) -> PusTmReader<'_> {
            let next_msg = self.tm_receiver.try_recv();
            assert!(next_msg.is_ok());
            self.current_tm = Some(next_msg.unwrap().packet);
            PusTmReader::new(self.current_tm.as_ref().unwrap(), 7).unwrap()
        }

        pub fn check_no_tm_available(&self) -> bool {
            let next_msg = self.tm_receiver.try_recv();
            if let TryRecvError::Empty = next_msg.unwrap_err() {
                return true;
            }
            false
        }

        pub fn check_next_verification_tm(&self, subservice: u8, expected_request_id: RequestId) {
            let next_msg = self.tm_receiver.try_recv();
            assert!(next_msg.is_ok());
            let next_msg = next_msg.unwrap();
            let tm = PusTmReader::new(next_msg.packet.as_slice(), 7).unwrap();
            assert_eq!(PusPacket::service_type_id(&tm), 1);
            assert_eq!(PusPacket::message_subtype_id(&tm), subservice);
            assert_eq!(tm.apid(), TEST_APID);
            let req_id =
                RequestId::from_bytes(tm.user_data()).expect("generating request ID failed");
            assert_eq!(req_id, expected_request_id);
        }
    }

    pub const APP_DATA_TOO_SHORT: ResultU16 = ResultU16::new(1, 1);

    #[derive(Default)]
    pub struct TestConverter<const SERVICE: u8> {
        pub conversion_request: VecDeque<Vec<u8>>,
    }

    impl<const SERVICE: u8> TestConverter<SERVICE> {
        pub fn check_service(&self, tc: &PusTcReader) -> Result<(), PusPacketHandlingError> {
            if tc.service_type_id() != SERVICE {
                return Err(PusPacketHandlingError::RequestConversion(
                    GenericConversionError::WrongService(tc.service_type_id()),
                ));
            }
            Ok(())
        }

        pub fn is_empty(&self) {
            self.conversion_request.is_empty();
        }

        pub fn check_next_conversion(&mut self, tc: &PusTcCreator) {
            assert!(!self.conversion_request.is_empty());
            assert_eq!(
                self.conversion_request.pop_front().unwrap(),
                tc.to_vec().unwrap()
            );
        }
    }

    pub struct TestRouter<REQUEST> {
        pub routing_requests: RefCell<VecDeque<(ComponentId, REQUEST)>>,
        pub routing_errors: RefCell<VecDeque<(ComponentId, GenericRoutingError)>>,
        pub injected_routing_failure: RefCell<Option<GenericRoutingError>>,
    }

    impl<REQUEST> Default for TestRouter<REQUEST> {
        fn default() -> Self {
            Self {
                routing_requests: Default::default(),
                routing_errors: Default::default(),
                injected_routing_failure: Default::default(),
            }
        }
    }

    impl<REQUEST> TestRouter<REQUEST> {
        pub fn check_for_injected_error(&self) -> Result<(), GenericRoutingError> {
            if self.injected_routing_failure.borrow().is_some() {
                return Err(self.injected_routing_failure.borrow_mut().take().unwrap());
            }
            Ok(())
        }

        pub fn handle_error(
            &self,
            target_id: ComponentId,
            _token: VerificationToken<TcStateAccepted>,
            _tc: &PusTcReader,
            error: GenericRoutingError,
            _time_stamp: &[u8],
            _verif_reporter: &impl VerificationReportingProvider,
        ) {
            self.routing_errors
                .borrow_mut()
                .push_back((target_id, error));
        }

        pub fn no_routing_errors(&self) -> bool {
            self.routing_errors.borrow().is_empty()
        }

        pub fn retrieve_next_routing_error(&mut self) -> (ComponentId, GenericRoutingError) {
            if self.routing_errors.borrow().is_empty() {
                panic!("no routing request available");
            }
            self.routing_errors.borrow_mut().pop_front().unwrap()
        }

        pub fn inject_routing_error(&mut self, error: GenericRoutingError) {
            *self.injected_routing_failure.borrow_mut() = Some(error);
        }

        pub fn is_empty(&self) -> bool {
            self.routing_requests.borrow().is_empty()
        }

        pub fn retrieve_next_request(&mut self) -> (ComponentId, REQUEST) {
            if self.routing_requests.borrow().is_empty() {
                panic!("no routing request available");
            }
            self.routing_requests.borrow_mut().pop_front().unwrap()
        }
    }
}
