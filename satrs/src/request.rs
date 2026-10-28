use arbitrary_int::{prelude::*, u11, u21};
use core::{fmt, marker::PhantomData};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "alloc")]
pub use alloc_mod::*;

#[cfg(feature = "std")]
pub use std_mod::*;

use spacepackets::{
    ByteConversionError,
    ecss::{PusPacket, tc::IsPusTelecommand},
};

use crate::{
    ComponentId,
    queue::{GenericReceiveError, GenericSendError},
};

/// Generic request ID type. Requests can be associated with an ID to have a unique identifier
/// for them. This can be useful for tasks like tracking their progress.
pub type RequestId = u32;

/// CCSDS APID type definition. Please note that the APID is a 14 bit value.
pub type Apid = u11;

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct UniqueApidTargetId {
    pub apid: Apid,
    pub unique_id: u21,
}

impl UniqueApidTargetId {
    pub const fn new(apid: Apid, target: u21) -> Self {
        Self {
            apid,
            unique_id: target,
        }
    }

    pub fn raw(&self) -> ComponentId {
        ((self.apid.value() as u32) << 21) | (self.unique_id.value())
    }

    pub fn id(&self) -> ComponentId {
        self.raw()
    }

    /// This function attempts to build the ID from a PUS telecommand by extracting the APID
    /// and the first four bytes of the application data field as the target field.
    pub fn from_pus_tc(
        tc: &(impl PusPacket + IsPusTelecommand),
    ) -> Result<Self, ByteConversionError> {
        if tc.user_data().len() < 4 {
            return Err(ByteConversionError::FromSliceTooSmall {
                found: tc.user_data().len(),
                expected: 4,
            });
        }
        Ok(Self::new(
            tc.apid(),
            u21::new(u32::from_be_bytes(tc.user_data()[0..4].try_into().unwrap())),
        ))
    }
}

impl From<ComponentId> for UniqueApidTargetId {
    fn from(raw: ComponentId) -> Self {
        Self {
            apid: u11::new((raw >> 21) as u16),
            unique_id: u21::new(raw & u21::MAX.value()),
        }
    }
}

impl From<UniqueApidTargetId> for ComponentId {
    fn from(target_and_apid_id: UniqueApidTargetId) -> Self {
        target_and_apid_id.raw()
    }
}

impl fmt::Display for UniqueApidTargetId {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Target and APID ID with  APID {:#05x} and target {}",
            self.apid.value(),
            self.unique_id
        )
    }
}

/// This contains metadata information which might be useful when used together with a
/// generic message tpye.
///
/// This could for example be used to build request/reply patterns or state tracking for request.
#[derive(Debug, Copy, PartialEq, Eq, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MessageMetadata {
    request_id: RequestId,
    sender_id: ComponentId,
}

impl MessageMetadata {
    pub const fn new(request_id: RequestId, sender_id: ComponentId) -> Self {
        Self {
            request_id,
            sender_id,
        }
    }

    pub fn request_id(&self) -> RequestId {
        self.request_id
    }

    pub fn sender_id(&self) -> ComponentId {
        self.sender_id
    }
}

/// Generic message type which adds [metadata][MessageMetadata] to a generic message typ.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GenericMessage<Message> {
    pub requestor_info: MessageMetadata,
    pub message: Message,
}

impl<Message> GenericMessage<Message> {
    pub fn new(requestor_info: MessageMetadata, message: Message) -> Self {
        Self {
            requestor_info,
            message,
        }
    }

    delegate::delegate! {
        to self.requestor_info {
            pub fn request_id(&self) -> RequestId;
            pub fn sender_id(&self) -> ComponentId;
        }
    }
}

/// Generic trait for objects which can send targeted messages.
pub trait MessageSenderProvider<MSG>: Send {
    fn send(&self, message: GenericMessage<MSG>) -> Result<(), GenericSendError>;
}

// Generic trait for objects which can receive targeted messages.
pub trait MessageReceiverProvider<MSG> {
    fn try_recv(&self) -> Result<Option<GenericMessage<MSG>>, GenericReceiveError>;
}

pub struct MessageWithSenderIdReceiver<Msg, Receiver: MessageReceiverProvider<Msg>>(
    pub Receiver,
    PhantomData<Msg>,
);

impl<MSG, R: MessageReceiverProvider<MSG>> From<R> for MessageWithSenderIdReceiver<MSG, R> {
    fn from(receiver: R) -> Self {
        MessageWithSenderIdReceiver(receiver, PhantomData)
    }
}

impl<MSG, R: MessageReceiverProvider<MSG>> MessageWithSenderIdReceiver<MSG, R> {
    pub fn try_recv_message(&self) -> Result<Option<GenericMessage<MSG>>, GenericReceiveError> {
        self.0.try_recv()
    }
}

pub struct MessageReceiverWithId<MSG, R: MessageReceiverProvider<MSG>> {
    local_channel_id: ComponentId,
    reply_receiver: MessageWithSenderIdReceiver<MSG, R>,
}

impl<MSG, R: MessageReceiverProvider<MSG>> MessageReceiverWithId<MSG, R> {
    pub fn new(local_channel_id: ComponentId, reply_receiver: R) -> Self {
        Self {
            local_channel_id,
            reply_receiver: MessageWithSenderIdReceiver::from(reply_receiver),
        }
    }

    pub fn local_channel_id(&self) -> ComponentId {
        self.local_channel_id
    }
}

impl<MSG, R: MessageReceiverProvider<MSG>> MessageReceiverWithId<MSG, R> {
    pub fn try_recv_message(&self) -> Result<Option<GenericMessage<MSG>>, GenericReceiveError> {
        self.reply_receiver.0.try_recv()
    }
}

pub trait MessageSenderStoreProvider<Message, Sender>: Default {
    fn add_message_target(&mut self, target_id: ComponentId, message_sender: Sender);

    fn send_message(
        &self,
        requestor_info: MessageMetadata,
        target_channel_id: ComponentId,
        message: Message,
    ) -> Result<(), GenericSendError>;
}

#[cfg(feature = "alloc")]
pub mod alloc_mod {
    use crate::queue::GenericSendError;
    use std::convert::From;

    use super::*;
    use hashbrown::HashMap;

    pub struct OneMessageSender<Msg, S: MessageSenderProvider<Msg>> {
        pub id_and_sender: Option<(ComponentId, S)>,
        pub(crate) phantom: PhantomData<Msg>,
    }

    impl<Msg, S: MessageSenderProvider<Msg>> Default for OneMessageSender<Msg, S> {
        fn default() -> Self {
            Self {
                id_and_sender: Default::default(),
                phantom: Default::default(),
            }
        }
    }

    impl<Msg, Sender: MessageSenderProvider<Msg>> MessageSenderStoreProvider<Msg, Sender>
        for OneMessageSender<Msg, Sender>
    {
        fn add_message_target(&mut self, target_id: ComponentId, message_sender: Sender) {
            if self.id_and_sender.is_some() {
                return;
            }
            self.id_and_sender = Some((target_id, message_sender));
        }

        fn send_message(
            &self,
            requestor_info: MessageMetadata,
            target_channel_id: ComponentId,
            message: Msg,
        ) -> Result<(), GenericSendError> {
            if let Some((current_id, sender)) = &self.id_and_sender
                && *current_id == target_channel_id
            {
                sender.send(GenericMessage::new(requestor_info, message))?;
                return Ok(());
            }
            Err(GenericSendError::TargetDoesNotExist(target_channel_id))
        }
    }

    pub struct MessageSenderList<MSG, S: MessageSenderProvider<MSG>>(
        pub alloc::vec::Vec<(ComponentId, S)>,
        pub(crate) PhantomData<MSG>,
    );

    impl<MSG, S: MessageSenderProvider<MSG>> Default for MessageSenderList<MSG, S> {
        fn default() -> Self {
            Self(Default::default(), PhantomData)
        }
    }

    impl<Msg, Sender: MessageSenderProvider<Msg>> MessageSenderStoreProvider<Msg, Sender>
        for MessageSenderList<Msg, Sender>
    {
        fn add_message_target(&mut self, target_id: ComponentId, message_sender: Sender) {
            self.0.push((target_id, message_sender));
        }

        fn send_message(
            &self,
            requestor_info: MessageMetadata,
            target_channel_id: ComponentId,
            message: Msg,
        ) -> Result<(), GenericSendError> {
            for (current_id, sender) in &self.0 {
                if *current_id == target_channel_id {
                    sender.send(GenericMessage::new(requestor_info, message))?;
                    return Ok(());
                }
            }
            Err(GenericSendError::TargetDoesNotExist(target_channel_id))
        }
    }

    pub struct MessageSenderMap<MSG, S: MessageSenderProvider<MSG>>(
        pub HashMap<ComponentId, S>,
        pub(crate) PhantomData<MSG>,
    );

    impl<MSG, S: MessageSenderProvider<MSG>> Default for MessageSenderMap<MSG, S> {
        fn default() -> Self {
            Self(Default::default(), PhantomData)
        }
    }

    impl<Msg, Sender: MessageSenderProvider<Msg>> MessageSenderStoreProvider<Msg, Sender>
        for MessageSenderMap<Msg, Sender>
    {
        fn add_message_target(&mut self, target_id: ComponentId, message_sender: Sender) {
            self.0.insert(target_id, message_sender);
        }

        fn send_message(
            &self,
            requestor_info: MessageMetadata,
            target_channel_id: ComponentId,
            message: Msg,
        ) -> Result<(), GenericSendError> {
            if self.0.contains_key(&target_channel_id) {
                return self
                    .0
                    .get(&target_channel_id)
                    .unwrap()
                    .send(GenericMessage::new(requestor_info, message));
            }
            Err(GenericSendError::TargetDoesNotExist(target_channel_id))
        }
    }

    pub struct MessageSenderAndReceiver<
        To,
        From,
        Sender: MessageSenderProvider<To>,
        Receiver: MessageReceiverProvider<From>,
        SenderStore: MessageSenderStoreProvider<To, Sender>,
    > {
        pub local_channel_id: ComponentId,
        pub message_sender_store: SenderStore,
        pub message_receiver: MessageWithSenderIdReceiver<From, Receiver>,
        pub(crate) phantom: PhantomData<(To, Sender)>,
    }

    impl<
        To,
        From,
        Sender: MessageSenderProvider<To>,
        Receiver: MessageReceiverProvider<From>,
        SenderStore: MessageSenderStoreProvider<To, Sender>,
    > MessageSenderAndReceiver<To, From, Sender, Receiver, SenderStore>
    {
        pub fn new(local_channel_id: ComponentId, message_receiver: Receiver) -> Self {
            Self {
                local_channel_id,
                message_sender_store: Default::default(),
                message_receiver: MessageWithSenderIdReceiver::from(message_receiver),
                phantom: PhantomData,
            }
        }

        pub fn add_message_target(&mut self, target_id: ComponentId, message_sender: Sender) {
            self.message_sender_store
                .add_message_target(target_id, message_sender)
        }

        pub fn local_channel_id_generic(&self) -> ComponentId {
            self.local_channel_id
        }

        /// Try to send a message, which can be a reply or a request, depending on the generics.
        pub fn send_message(
            &self,
            request_id: RequestId,
            target_id: ComponentId,
            message: To,
        ) -> Result<(), GenericSendError> {
            self.message_sender_store.send_message(
                MessageMetadata::new(request_id, self.local_channel_id_generic()),
                target_id,
                message,
            )
        }

        /// Try to receive a message, which can be a reply or a request, depending on the generics.
        pub fn try_recv_message(
            &self,
        ) -> Result<Option<GenericMessage<From>>, GenericReceiveError> {
            self.message_receiver.try_recv_message()
        }
    }

    pub struct RequestAndReplySenderAndReceiver<
        Request,
        ReqSender: MessageSenderProvider<Request>,
        ReqReceiver: MessageReceiverProvider<Request>,
        ReqSenderStore: MessageSenderStoreProvider<Request, ReqSender>,
        Reply,
        ReplySender: MessageSenderProvider<Reply>,
        ReplyReceiver: MessageReceiverProvider<Reply>,
        ReplySenderStore: MessageSenderStoreProvider<Reply, ReplySender>,
    > {
        pub local_channel_id: ComponentId,
        // These 2 are a functional group.
        pub request_sender_store: ReqSenderStore,
        pub reply_receiver: MessageWithSenderIdReceiver<Reply, ReplyReceiver>,
        // These 2 are a functional group.
        pub request_receiver: MessageWithSenderIdReceiver<Request, ReqReceiver>,
        pub reply_sender_store: ReplySenderStore,
        phantom: PhantomData<(ReqSender, ReplySender)>,
    }

    impl<
        Request,
        ReqSender: MessageSenderProvider<Request>,
        ReqReceiver: MessageReceiverProvider<Request>,
        ReqSenderStore: MessageSenderStoreProvider<Request, ReqSender>,
        Reply,
        ReplySender: MessageSenderProvider<Reply>,
        ReplyReceiver: MessageReceiverProvider<Reply>,
        ReplySenderStore: MessageSenderStoreProvider<Reply, ReplySender>,
    >
        RequestAndReplySenderAndReceiver<
            Request,
            ReqSender,
            ReqReceiver,
            ReqSenderStore,
            Reply,
            ReplySender,
            ReplyReceiver,
            ReplySenderStore,
        >
    {
        pub fn new(
            local_channel_id: ComponentId,
            request_receiver: ReqReceiver,
            reply_receiver: ReplyReceiver,
        ) -> Self {
            Self {
                local_channel_id,
                request_receiver: request_receiver.into(),
                reply_receiver: reply_receiver.into(),
                request_sender_store: Default::default(),
                reply_sender_store: Default::default(),
                phantom: PhantomData,
            }
        }

        pub fn local_channel_id_generic(&self) -> ComponentId {
            self.local_channel_id
        }
    }
}

#[cfg(feature = "std")]
pub mod std_mod {

    use super::*;
    use std::sync::mpsc;

    use crate::queue::{GenericReceiveError, GenericSendError};

    impl<MSG: Send> MessageSenderProvider<MSG> for mpsc::Sender<GenericMessage<MSG>> {
        fn send(&self, message: GenericMessage<MSG>) -> Result<(), GenericSendError> {
            self.send(message)
                .map_err(|_| GenericSendError::RxDisconnected)?;
            Ok(())
        }
    }
    impl<MSG: Send> MessageSenderProvider<MSG> for mpsc::SyncSender<GenericMessage<MSG>> {
        fn send(&self, message: GenericMessage<MSG>) -> Result<(), GenericSendError> {
            if let Err(e) = self.try_send(message) {
                return match e {
                    mpsc::TrySendError::Full(_) => Err(GenericSendError::QueueFull(None)),
                    mpsc::TrySendError::Disconnected(_) => Err(GenericSendError::RxDisconnected),
                };
            }
            Ok(())
        }
    }

    pub type MessageSenderMapMpsc<MSG> = MessageReceiverWithId<MSG, mpsc::Sender<MSG>>;
    pub type MessageSenderMapBoundedMpsc<MSG> = MessageReceiverWithId<MSG, mpsc::SyncSender<MSG>>;

    impl<MSG> MessageReceiverProvider<MSG> for mpsc::Receiver<GenericMessage<MSG>> {
        fn try_recv(&self) -> Result<Option<GenericMessage<MSG>>, GenericReceiveError> {
            match self.try_recv() {
                Ok(msg) => Ok(Some(msg)),
                Err(e) => match e {
                    mpsc::TryRecvError::Empty => Ok(None),
                    mpsc::TryRecvError::Disconnected => {
                        Err(GenericReceiveError::TxDisconnected(None))
                    }
                },
            }
        }
    }

    pub type MessageReceiverWithIdMpsc<MSG> = MessageReceiverWithId<MSG, mpsc::Receiver<MSG>>;
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use alloc::string::ToString;
    use arbitrary_int::{u11, u14, u21};
    use spacepackets::{
        ByteConversionError, SpHeader,
        ecss::{
            CreatorConfig, MessageTypeId,
            tc::{PusTcCreator, PusTcSecondaryHeader},
        },
    };

    use crate::{
        ComponentId,
        queue::{GenericReceiveError, GenericSendError},
        request::{Apid, MessageMetadata, MessageSenderMap, MessageSenderStoreProvider},
    };

    use super::{GenericMessage, MessageReceiverWithId, UniqueApidTargetId};

    const TEST_CHANNEL_ID_0: ComponentId = 1;
    const TEST_CHANNEL_ID_1: ComponentId = 2;
    const TEST_CHANNEL_ID_2: ComponentId = 3;

    #[test]
    fn test_basic_target_id_with_apid() {
        let id = UniqueApidTargetId::new(Apid::new(0x111), u21::new(0x01));
        assert_eq!(id.apid.value(), 0x111);
        assert_eq!(id.unique_id.value(), 0x01);
        assert_eq!(id.id(), id.raw());
        assert_eq!(ComponentId::from(id), id.raw());
        let id_raw = id.raw();
        let id_from_raw = UniqueApidTargetId::from(id_raw);
        assert_eq!(id_from_raw, id);
        assert_eq!(id.id(), (0x111 << 21) | 0x01);
        let string = id.to_string();
        assert_eq!(
            string,
            "Target and APID ID with  APID 0x111 and target 1".to_string()
        );
    }

    #[test]
    fn test_basic_target_id_with_apid_from_pus_tc() {
        let sp_header = SpHeader::new_for_unseg_tc(u11::new(0x111), u14::new(5), 0);
        let app_data = 1_u32.to_be_bytes();
        let pus_tc = PusTcCreator::new_simple(
            sp_header,
            MessageTypeId::new(17, 1),
            &app_data,
            CreatorConfig::default(),
        );
        let id = UniqueApidTargetId::from_pus_tc(&pus_tc).unwrap();
        assert_eq!(id.apid.value(), 0x111);
        assert_eq!(id.unique_id.value(), 1);
    }

    #[test]
    fn test_basic_target_id_with_apid_from_pus_tc_invalid_app_data() {
        let sp_header = SpHeader::new_for_unseg_tc(u11::new(0x111), u14::new(5), 0);
        let sec_header = PusTcSecondaryHeader::new_simple(MessageTypeId::new(17, 1));
        let pus_tc = PusTcCreator::new_no_app_data(sp_header, sec_header, CreatorConfig::default());
        let error = UniqueApidTargetId::from_pus_tc(&pus_tc);
        assert!(error.is_err());
        let error = error.unwrap_err();
        if let ByteConversionError::FromSliceTooSmall { found, expected } = error {
            assert_eq!(found, 0);
            assert_eq!(expected, 4);
        } else {
            panic!("Unexpected error type");
        }
    }

    #[test]
    fn test_receiver_only() {
        let (sender, receiver) = mpsc::channel();
        // Test structure with only a receiver which has a channel ID.
        let receiver = MessageReceiverWithId::new(TEST_CHANNEL_ID_0, receiver);
        let request_id = 5;
        sender
            .send(GenericMessage::new(
                MessageMetadata::new(request_id, TEST_CHANNEL_ID_1),
                5,
            ))
            .unwrap();
        let reply = receiver.try_recv_message().unwrap();
        assert!(reply.is_some());
        assert_eq!(receiver.local_channel_id(), TEST_CHANNEL_ID_0);
        let reply = reply.unwrap();
        assert_eq!(reply.requestor_info.request_id, request_id);
        assert_eq!(reply.requestor_info.sender_id, TEST_CHANNEL_ID_1);
        assert_eq!(reply.message, 5);
    }

    #[test]
    fn test_receiver_empty() {
        let (_sender, receiver) = mpsc::sync_channel::<GenericMessage<i32>>(2);
        // Test structure with only a receiver which has a channel ID.
        let receiver = MessageReceiverWithId::new(TEST_CHANNEL_ID_0, receiver);
        let reply = receiver.try_recv_message().unwrap();
        assert!(reply.is_none());
    }

    #[test]
    fn test_all_tx_disconnected() {
        let (sender, receiver) = mpsc::sync_channel::<GenericMessage<i32>>(2);
        // Test structure with only a receiver which has a channel ID.
        let receiver = MessageReceiverWithId::new(TEST_CHANNEL_ID_0, receiver);
        drop(sender);
        let reply = receiver.try_recv_message();
        assert!(reply.is_err());
        let error = reply.unwrap_err();
        if let GenericReceiveError::TxDisconnected(None) = error {
        } else {
            panic!("unexpected error type");
        }
    }

    #[test]
    fn test_sender_map() {
        let (sender0, receiver0) = mpsc::channel();
        let (sender1, receiver1) = mpsc::channel();
        let mut sender_map = MessageSenderMap::default();
        sender_map.add_message_target(TEST_CHANNEL_ID_1, sender0);
        sender_map.add_message_target(TEST_CHANNEL_ID_2, sender1);
        sender_map
            .send_message(
                MessageMetadata::new(1, TEST_CHANNEL_ID_0),
                TEST_CHANNEL_ID_1,
                5,
            )
            .expect("sending message failed");
        let mut reply = receiver0.recv().expect("receiving message failed");
        assert_eq!(reply.request_id(), 1);
        assert_eq!(reply.sender_id(), TEST_CHANNEL_ID_0);
        assert_eq!(reply.message, 5);
        sender_map
            .send_message(
                MessageMetadata::new(2, TEST_CHANNEL_ID_0),
                TEST_CHANNEL_ID_2,
                10,
            )
            .expect("sending message failed");
        reply = receiver1.recv().expect("receiving message failed");
        assert_eq!(reply.request_id(), 2);
        assert_eq!(reply.sender_id(), TEST_CHANNEL_ID_0);
        assert_eq!(reply.message, 10);
    }

    #[test]
    fn test_sender_map_target_does_not_exist() {
        let (sender0, _) = mpsc::channel();
        let mut sender_map_with_id = MessageSenderMap::default();
        sender_map_with_id.add_message_target(TEST_CHANNEL_ID_1, sender0);
        let result = sender_map_with_id.send_message(
            MessageMetadata::new(1, TEST_CHANNEL_ID_0),
            TEST_CHANNEL_ID_2,
            5,
        );
        assert!(result.is_err());
        let error = result.unwrap_err();
        if let GenericSendError::TargetDoesNotExist(target) = error {
            assert_eq!(target, TEST_CHANNEL_ID_2);
        } else {
            panic!("Unexpected error type");
        }
    }
    #[test]
    fn test_sender_map_queue_full() {
        let (sender0, _receiver0) = mpsc::sync_channel(1);
        let mut sender_map_with_id = MessageSenderMap::default();
        sender_map_with_id.add_message_target(TEST_CHANNEL_ID_1, sender0);
        sender_map_with_id
            .send_message(
                MessageMetadata::new(1, TEST_CHANNEL_ID_0),
                TEST_CHANNEL_ID_1,
                5,
            )
            .expect("sending message failed");
        let result = sender_map_with_id.send_message(
            MessageMetadata::new(1, TEST_CHANNEL_ID_0),
            TEST_CHANNEL_ID_1,
            5,
        );
        assert!(result.is_err());
        let error = result.unwrap_err();
        if let GenericSendError::QueueFull(capacity) = error {
            assert!(capacity.is_none());
        } else {
            panic!("Unexpected error type {}", error);
        }
    }

    #[test]
    fn test_sender_map_queue_receiver_disconnected() {
        let (sender0, receiver0) = mpsc::sync_channel(1);
        let mut sender_map_with_id = MessageSenderMap::default();
        sender_map_with_id.add_message_target(TEST_CHANNEL_ID_1, sender0);
        drop(receiver0);
        let result = sender_map_with_id.send_message(
            MessageMetadata::new(1, TEST_CHANNEL_ID_0),
            TEST_CHANNEL_ID_1,
            5,
        );
        assert!(result.is_err());
        let error = result.unwrap_err();
        if let GenericSendError::RxDisconnected = error {
        } else {
            panic!("Unexpected error type {}", error);
        }
    }
}
