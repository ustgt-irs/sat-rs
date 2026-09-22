use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, mpsc},
};

use derive_new::new;
use num_enum::{IntoPrimitive, TryFromPrimitive};
use satrs::spacepackets::CcsdsPacketIdAndPsc;
use satrs_example::TimestampHelper;
use satrs_minisim::{
    SerializableSimMsgPayload, SimReply, SimRequest,
    eps::{PcduReply, PcduRequest},
};
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator as _;
use types::{
    ComponentId, DeviceMode,
    ccsds::{CcsdsTcPacketOwned, CcsdsTmPacketOwned},
    pcdu::{
        self, SwitchId, SwitchMapBinary, SwitchMapBinaryWrapper, SwitchRequest, SwitchState,
        SwitchStateBinary, SwitchesBitfield,
    },
};

use crate::ccsds::pack_ccsds_tm_packet_for_now;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchSet {
    pub valid: bool,
    pub switch_map: SwitchMap,
}

impl SwitchSet {
    pub fn new(switch_map: SwitchMap) -> Self {
        Self {
            valid: true,
            switch_map,
        }
    }

    pub fn new_with_init_switches_unknown() -> Self {
        let wrapper = SwitchMapWrapper::default();
        Self::new(wrapper.0)
    }

    pub fn as_bitfield(&self) -> Option<SwitchesBitfield> {
        for entry in SwitchId::iter() {
            if !self.switch_map.contains_key(&entry) {
                return None;
            }
        }
        Some(
            SwitchesBitfield::builder()
                .with_magnetorquer(*self.switch_map.get(&SwitchId::Mgt).unwrap() == SwitchState::On)
                .with_mgm1(*self.switch_map.get(&SwitchId::Mgm1).unwrap() == SwitchState::On)
                .with_mgm0(*self.switch_map.get(&SwitchId::Mgm0).unwrap() == SwitchState::On)
                .build(),
        )
    }

    #[allow(dead_code)]
    pub fn set_switch_state(&mut self, switch_id: SwitchId, state: SwitchState) -> bool {
        if !self.switch_map.contains_key(&switch_id) {
            return false;
        }
        *self.switch_map.get_mut(&switch_id).unwrap() = state;
        true
    }
}

pub type SwitchMap = HashMap<SwitchId, SwitchState>;

pub struct SwitchMapWrapper(pub SwitchMap);

impl Default for SwitchMapWrapper {
    fn default() -> Self {
        let mut switch_map = SwitchMap::default();
        for entry in SwitchId::iter() {
            switch_map.insert(entry, SwitchState::Unknown);
        }
        Self(switch_map)
    }
}

impl SwitchMapWrapper {
    #[allow(dead_code)]
    pub fn new_with_init_switches_off() -> Self {
        let mut switch_map = SwitchMap::default();
        for entry in SwitchId::iter() {
            switch_map.insert(entry, SwitchState::Off);
        }
        Self(switch_map)
    }

    pub fn from_binary_switch_map_ref(switch_map: &SwitchMapBinary) -> Self {
        Self(
            switch_map
                .iter()
                .map(|(key, value)| (*key, SwitchState::from(*value)))
                .collect(),
        )
    }
}

pub type SharedSwitchSet = Arc<Mutex<SwitchSet>>;

pub trait SerialInterface {
    type Error: core::fmt::Debug;

    /// Send some data via the serial interface.
    fn send(&self, data: &[u8]) -> Result<(), Self::Error>;
    /// Receive all replies received on the serial interface so far. This function takes a closure
    /// and call its for each received packet, passing the received packet into it.
    fn try_recv_replies<ReplyHandler: FnMut(&[u8])>(
        &self,
        f: ReplyHandler,
    ) -> Result<(), Self::Error>;
}

#[derive(new)]
pub struct SerialInterfaceToSim {
    pub sim_request_tx: mpsc::Sender<SimRequest>,
    pub sim_reply_rx: mpsc::Receiver<SimReply>,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, TryFromPrimitive, IntoPrimitive)]
#[repr(u32)]
pub enum SetId {
    SwitcherSet = 0,
}

impl SerialInterface for SerialInterfaceToSim {
    type Error = ();

    fn send(&self, data: &[u8]) -> Result<(), Self::Error> {
        let request: PcduRequest = serde_json::from_slice(data).expect("expected a PCDU request");
        self.sim_request_tx
            .send(SimRequest::new_with_epoch_time(request))
            .expect("failed to send request to simulation");
        Ok(())
    }

    fn try_recv_replies<ReplyHandler: FnMut(&[u8])>(
        &self,
        mut f: ReplyHandler,
    ) -> Result<(), Self::Error> {
        loop {
            match self.sim_reply_rx.try_recv() {
                Ok(reply) => {
                    let reply = serde_json::to_string(&reply).unwrap();
                    f(reply.as_bytes());
                }
                Err(e) => match e {
                    mpsc::TryRecvError::Empty => break,
                    mpsc::TryRecvError::Disconnected => {
                        log::warn!("sim reply sender has disconnected");
                        break;
                    }
                },
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct SerialInterfaceDummy {
    // Need interior mutability here for both fields.
    pub switch_map: RefCell<SwitchMapBinaryWrapper>,
    pub reply_deque: RefCell<VecDeque<SimReply>>,
}

impl SerialInterface for SerialInterfaceDummy {
    type Error = ();

    fn send(&self, data: &[u8]) -> Result<(), Self::Error> {
        let pcdu_req: PcduRequest = serde_json::from_slice(data).unwrap();
        let switch_map_mut = &mut self.switch_map.borrow_mut().0;
        match pcdu_req {
            PcduRequest::SwitchDevice { switch, state } => {
                match switch_map_mut.entry(switch) {
                    std::collections::hash_map::Entry::Occupied(mut val) => {
                        *val.get_mut() = state;
                    }
                    std::collections::hash_map::Entry::Vacant(vacant) => {
                        vacant.insert(state);
                    }
                };
            }
            PcduRequest::RequestSwitchInfo => {
                let mut reply_deque_mut = self.reply_deque.borrow_mut();
                reply_deque_mut.push_back(SimReply::new(&PcduReply::SwitchInfo(
                    switch_map_mut.clone(),
                )));
            }
        };
        Ok(())
    }

    fn try_recv_replies<ReplyHandler: FnMut(&[u8])>(
        &self,
        mut f: ReplyHandler,
    ) -> Result<(), Self::Error> {
        if self.reply_queue_empty() {
            return Ok(());
        }
        loop {
            let reply = self.get_next_reply_as_string();
            f(reply.as_bytes());
            if self.reply_queue_empty() {
                break;
            }
        }
        Ok(())
    }
}

impl SerialInterfaceDummy {
    fn get_next_reply_as_string(&self) -> String {
        let mut reply_deque_mut = self.reply_deque.borrow_mut();
        let next_reply = reply_deque_mut.pop_front().unwrap();
        serde_json::to_string(&next_reply).unwrap()
    }

    fn reply_queue_empty(&self) -> bool {
        self.reply_deque.borrow().is_empty()
    }
}

pub enum SerialSimInterfaceWrapper {
    Dummy(SerialInterfaceDummy),
    Sim(SerialInterfaceToSim),
}

impl SerialInterface for SerialSimInterfaceWrapper {
    type Error = ();

    fn send(&self, data: &[u8]) -> Result<(), Self::Error> {
        match self {
            SerialSimInterfaceWrapper::Dummy(dummy) => dummy.send(data),
            SerialSimInterfaceWrapper::Sim(sim) => sim.send(data),
        }
    }

    fn try_recv_replies<ReplyHandler: FnMut(&[u8])>(
        &self,
        f: ReplyHandler,
    ) -> Result<(), Self::Error> {
        match self {
            SerialSimInterfaceWrapper::Dummy(dummy) => dummy.try_recv_replies(f),
            SerialSimInterfaceWrapper::Sim(sim) => sim.try_recv_replies(f),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum OpCode {
    RegularOp = 0,
    PollAndRecvReplies = 1,
}

/// Example PCDU device handler.
#[allow(clippy::too_many_arguments)]
pub struct PcduHandler<ComInterface: SerialInterface> {
    dev_str: &'static str,
    switch_request_rx: mpsc::Receiver<SwitchRequest>,
    tc_rx: std::sync::mpsc::Receiver<CcsdsTcPacketOwned>,
    tm_tx: mpsc::SyncSender<CcsdsTmPacketOwned>,
    pub com_interface: ComInterface,
    shared_switch_map: Arc<Mutex<SwitchSet>>,
    mode: DeviceMode,
    stamp_helper: TimestampHelper,
    event_tx: mpsc::SyncSender<pcdu::Event>,
}

impl<ComInterface: SerialInterface> PcduHandler<ComInterface> {
    pub fn new(
        tc_rx: std::sync::mpsc::Receiver<CcsdsTcPacketOwned>,
        tm_tx: std::sync::mpsc::SyncSender<CcsdsTmPacketOwned>,
        switch_request_rx: mpsc::Receiver<SwitchRequest>,
        com_interface: ComInterface,
        shared_switch_map: Arc<Mutex<SwitchSet>>,
        init_mode: DeviceMode,
        event_tx: mpsc::SyncSender<pcdu::Event>,
    ) -> Self {
        Self {
            dev_str: "PCDU",
            tc_rx,
            switch_request_rx,
            tm_tx,
            com_interface,
            shared_switch_map,
            stamp_helper: TimestampHelper::default(),
            // Start in normal mode by default. Assume that the PCDU itself is on by default.
            mode: init_mode,
            event_tx,
        }
    }

    pub fn periodic_operation(&mut self, op_code: OpCode) {
        match op_code {
            OpCode::RegularOp => {
                self.stamp_helper.update_from_now();
                // Handle requests.
                self.handle_telecommands();
                self.handle_switch_requests();
                // Poll the switch states and/or telemetry regularly here.
                if self.mode() == DeviceMode::Normal || self.mode() == DeviceMode::On {
                    self.handle_periodic_commands();
                }
            }
            OpCode::PollAndRecvReplies => {
                self.poll_and_handle_replies();
            }
        }
    }

    #[inline]
    pub fn mode(&self) -> DeviceMode {
        self.mode
    }

    pub fn handle_telecommands(&mut self) {
        loop {
            match self.tc_rx.try_recv() {
                Ok(packet) => {
                    let tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&packet.sp_header);
                    match postcard::from_bytes::<pcdu::request::Request>(&packet.payload) {
                        Ok(request) => {
                            log::info!(
                                "received request {:?} with TC ID {:#010x}",
                                request,
                                tc_id.raw()
                            );
                            match request {
                                pcdu::request::Request::Ping => {
                                    self.send_tm(Some(tc_id), pcdu::response::Response::Ok)
                                }
                                pcdu::request::Request::GetSwitches => self.send_tm(
                                    Some(tc_id),
                                    pcdu::response::Response::Switches(
                                        self.shared_switch_map
                                            .lock()
                                            .unwrap()
                                            .as_bitfield()
                                            .expect("could not build switches response"),
                                    ),
                                ),
                                pcdu::request::Request::EnableSwitches(switches) => {
                                    self.handle_switches_bitfield_request(
                                        switches,
                                        SwitchStateBinary::On,
                                    );
                                }
                                pcdu::request::Request::DisableSwitches(switches) => {
                                    self.handle_switches_bitfield_request(
                                        switches,
                                        SwitchStateBinary::Off,
                                    );
                                }
                                pcdu::request::Request::Mode(device_mode) => {
                                    self.switch_mode(tc_id, device_mode)
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("failed to deserialize request: {}", e);
                        }
                    }
                }
                Err(e) => match e {
                    std::sync::mpsc::TryRecvError::Empty => break,
                    std::sync::mpsc::TryRecvError::Disconnected => {
                        log::warn!("packet sender disconnected")
                    }
                },
            }
        }
    }

    pub fn handle_switches_bitfield_request(
        &mut self,
        switches: SwitchesBitfield,
        state: SwitchStateBinary,
    ) {
        if switches.mgm0() {
            self.handle_device_switching(SwitchId::Mgm0, state);
        }
        if switches.mgm1() {
            self.handle_device_switching(SwitchId::Mgm1, state);
        }
        if switches.magnetorquer() {
            self.handle_device_switching(SwitchId::Mgt, state);
        }
    }

    pub fn send_tm(&self, tc_id: Option<CcsdsPacketIdAndPsc>, response: pcdu::response::Response) {
        match pack_ccsds_tm_packet_for_now(ComponentId::EpsPcdu, tc_id, &response) {
            Ok(packet) => {
                if let Err(e) = self.tm_tx.send(packet) {
                    log::warn!("failed to send TM packet: {}", e);
                }
            }
            Err(e) => {
                log::warn!("failed to pack TM packet: {}", e);
            }
        }
    }

    fn switch_mode(&mut self, requestor: CcsdsPacketIdAndPsc, mode: DeviceMode) {
        log::info!("{}: transitioning to mode {:?}", self.dev_str, mode);
        self.mode = mode;
        if self.mode() == DeviceMode::Off {
            self.shared_switch_map.lock().unwrap().valid = false;
        }
        log::info!("{} announcing mode: {:?}", self.dev_str, self.mode);
        self.send_telemetry(Some(requestor), pcdu::response::Response::Ok);
    }

    pub fn send_telemetry(
        &self,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        response: pcdu::response::Response,
    ) {
        match pack_ccsds_tm_packet_for_now(ComponentId::EpsPcdu, tc_id, &response) {
            Ok(packet) => {
                if let Err(e) = self.tm_tx.send(packet) {
                    log::warn!("failed to send TM packet: {}", e);
                }
            }
            Err(e) => {
                log::warn!("failed to pack TM packet: {}", e);
            }
        }
    }

    pub fn handle_periodic_commands(&self) {
        let pcdu_req = PcduRequest::RequestSwitchInfo;
        let pcdu_req_ser = serde_json::to_string(&pcdu_req).unwrap();
        if let Err(_e) = self.com_interface.send(pcdu_req_ser.as_bytes()) {
            log::warn!("polling PCDU switch info failed");
            if let Err(e) = self.event_tx.send(pcdu::Event::SerialCommError) {
                log::warn!("failed to send comm error event: {}", e);
            }
        }
    }

    /*
    pub fn handle_mode_requests(&mut self) {
        loop {
            // TODO: Only allow one set mode request per cycle?
            match self.mode_node.try_recv_mode_request() {
                Ok(opt_msg) => {
                    if let Some(msg) = opt_msg {
                        let result = self.handle_mode_request(msg);
                        // TODO: Trigger event?
                        if result.is_err() {
                            log::warn!(
                                "{}: mode request failed with error {:?}",
                                self.dev_str,
                                result.err().unwrap()
                            );
                        }
                    } else {
                        break;
                    }
                }
                Err(e) => match e {
                    satrs::queue::GenericReceiveError::Empty => {
                        break;
                    }
                    satrs::queue::GenericReceiveError::TxDisconnected(_) => {
                        log::warn!("{}: failed to receive mode request: {:?}", self.dev_str, e);
                    }
                },
            }
        }
    }
    */

    pub fn handle_device_switching(&mut self, switch_id: SwitchId, state: SwitchStateBinary) {
        let pcdu_req = PcduRequest::SwitchDevice {
            switch: switch_id,
            state,
        };
        let pcdu_req_ser = serde_json::to_string(&pcdu_req).unwrap();
        self.com_interface
            .send(pcdu_req_ser.as_bytes())
            .expect("failed to send switch request to PCDU");
    }

    pub fn handle_switch_requests(&mut self) {
        loop {
            match self.switch_request_rx.try_recv() {
                Ok(switch_req) => {
                    self.handle_device_switching(switch_req.switch_id(), switch_req.target_state());
                }
                Err(e) => match e {
                    mpsc::TryRecvError::Empty => break,
                    mpsc::TryRecvError::Disconnected => {
                        log::warn!("switch request receiver has disconnected");
                        break;
                    }
                },
            };
        }
    }

    pub fn poll_and_handle_replies(&mut self) {
        if let Err(e) = self.com_interface.try_recv_replies(|reply| {
            let sim_reply: SimReply = serde_json::from_slice(reply).expect("invalid reply format");
            let pcdu_reply = PcduReply::from_sim_message(&sim_reply).expect("invalid reply format");
            match pcdu_reply {
                PcduReply::SwitchInfo(switch_info) => {
                    let switch_map_wrapper =
                        SwitchMapWrapper::from_binary_switch_map_ref(&switch_info);
                    let mut shared_switch_map = self
                        .shared_switch_map
                        .lock()
                        .expect("failed to lock switch map");
                    shared_switch_map.switch_map = switch_map_wrapper.0;
                    shared_switch_map.valid = true;
                }
            }
        }) {
            log::warn!("receiving PCDU replies failed: {e:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use arbitrary_int::u11;
    use satrs::spacepackets::SpacePacketHeader;
    use types::{
        Apid, TcHeader,
        pcdu::{SwitchMapBinary, SwitchStateBinary},
    };

    use super::*;

    pub fn create_request_tc(
        request: types::pcdu::request::Request,
    ) -> types::ccsds::CcsdsTcPacketOwned {
        types::ccsds::CcsdsTcPacketOwned::new_with_request(
            SpacePacketHeader::new_from_apid(u11::new(Apid::Eps as u16)),
            TcHeader::new(ComponentId::EpsPcdu, request.message_type()),
            request,
        )
    }

    #[derive(Default)]
    pub struct SerialInterfaceTest {
        pub inner: SerialInterfaceDummy,
        pub send_queue: RefCell<VecDeque<Vec<u8>>>,
        pub reply_queue: RefCell<VecDeque<String>>,
        /// Makes the next `send` call fail, to exercise comm-error handling.
        pub fail_next_send: RefCell<bool>,
    }

    impl SerialInterface for SerialInterfaceTest {
        type Error = ();

        fn send(&self, data: &[u8]) -> Result<(), Self::Error> {
            if self.fail_next_send.replace(false) {
                return Err(());
            }
            let mut send_queue_mut = self.send_queue.borrow_mut();
            send_queue_mut.push_back(data.to_vec());
            self.inner.send(data)
        }

        fn try_recv_replies<ReplyHandler: FnMut(&[u8])>(
            &self,
            mut f: ReplyHandler,
        ) -> Result<(), Self::Error> {
            if self.inner.reply_queue_empty() {
                return Ok(());
            }
            loop {
                let reply = self.inner.get_next_reply_as_string();
                self.reply_queue.borrow_mut().push_back(reply.clone());
                f(reply.as_bytes());
                if self.inner.reply_queue_empty() {
                    break;
                }
            }
            Ok(())
        }
    }

    #[allow(dead_code)]
    pub struct PcduTestbench {
        pub mode_request_tx: mpsc::SyncSender<types::pcdu::request::Request>,
        pub mode_reply_rx_to_parent: mpsc::Receiver<types::pcdu::response::Response>,
        pub tc_tx: mpsc::SyncSender<CcsdsTcPacketOwned>,
        pub tm_rx: mpsc::Receiver<CcsdsTmPacketOwned>,
        pub switch_request_tx: mpsc::Sender<SwitchRequest>,
        pub event_rx: mpsc::Receiver<pcdu::Event>,
        pub handler: PcduHandler<SerialInterfaceTest>,
    }

    impl PcduTestbench {
        pub fn new() -> Self {
            let (mode_request_tx, _mode_request_rx) = mpsc::sync_channel(5);
            let (_mode_reply_tx_to_parent, mode_reply_rx_to_parent) = mpsc::sync_channel(5);
            let (tc_tx, tc_rx) = mpsc::sync_channel(5);
            let (tm_tx, tm_rx) = mpsc::sync_channel(5);
            let (switch_request_tx, switch_reqest_rx) = mpsc::channel();
            let (event_tx, event_rx) = mpsc::sync_channel(5);
            let shared_switch_map =
                Arc::new(Mutex::new(SwitchSet::new_with_init_switches_unknown()));
            let handler = PcduHandler::new(
                tc_rx,
                tm_tx.clone(),
                switch_reqest_rx,
                SerialInterfaceTest::default(),
                shared_switch_map,
                DeviceMode::Off,
                event_tx,
            );
            Self {
                mode_request_tx,
                mode_reply_rx_to_parent,
                tc_tx,
                tm_rx,
                switch_request_tx,
                event_rx,
                handler,
            }
        }

        pub fn verify_switch_info_req_was_sent(&self, expected_queue_len: usize) {
            // Check that there is now communication happening.
            let mut send_queue_mut = self.handler.com_interface.send_queue.borrow_mut();
            assert_eq!(send_queue_mut.len(), expected_queue_len);
            let packet_sent = send_queue_mut.pop_front().unwrap();
            drop(send_queue_mut);
            let pcdu_req: PcduRequest = serde_json::from_slice(&packet_sent).unwrap();
            assert_eq!(pcdu_req, PcduRequest::RequestSwitchInfo);
        }

        pub fn verify_switch_req_was_sent(
            &self,
            expected_queue_len: usize,
            switch_id: SwitchId,
            target_state: SwitchStateBinary,
        ) {
            // Check that there is now communication happening.
            let mut send_queue_mut = self.handler.com_interface.send_queue.borrow_mut();
            assert_eq!(send_queue_mut.len(), expected_queue_len);
            let packet_sent = send_queue_mut.pop_front().unwrap();
            drop(send_queue_mut);
            let pcdu_req: PcduRequest = serde_json::from_slice(&packet_sent).unwrap();
            assert_eq!(
                pcdu_req,
                PcduRequest::SwitchDevice {
                    switch: switch_id,
                    state: target_state
                }
            )
        }

        pub fn verify_switch_reply_received(
            &self,
            expected_queue_len: usize,
            expected_map: SwitchMapBinary,
        ) {
            // Check that a switch reply was read back.
            let mut reply_received_mut = self.handler.com_interface.reply_queue.borrow_mut();
            assert_eq!(reply_received_mut.len(), expected_queue_len);
            let reply_received = reply_received_mut.pop_front().unwrap();
            let sim_reply: SimReply = serde_json::from_str(&reply_received).unwrap();
            let pcdu_reply = PcduReply::from_sim_message(&sim_reply).unwrap();
            assert_eq!(pcdu_reply, PcduReply::SwitchInfo(expected_map));
        }
    }

    #[test]
    fn test_periodic_command_send_failure_sends_event() {
        let testbench = PcduTestbench::new();
        *testbench.handler.com_interface.fail_next_send.borrow_mut() = true;
        testbench.handler.handle_periodic_commands();
        let event = testbench
            .event_rx
            .try_recv()
            .expect("expected comm error event");
        assert!(matches!(event, pcdu::Event::SerialCommError));
    }

    #[test]
    fn test_basic_handler() {
        let mut testbench = PcduTestbench::new();
        assert_eq!(testbench.handler.com_interface.send_queue.borrow().len(), 0);
        assert_eq!(
            testbench.handler.com_interface.reply_queue.borrow().len(),
            0
        );
        assert_eq!(testbench.handler.mode(), DeviceMode::Off);
        testbench.handler.periodic_operation(OpCode::RegularOp);
        testbench
            .handler
            .periodic_operation(OpCode::PollAndRecvReplies);
        // Handler is OFF, no changes expected.
        assert_eq!(testbench.handler.com_interface.send_queue.borrow().len(), 0);
        assert_eq!(
            testbench.handler.com_interface.reply_queue.borrow().len(),
            0
        );
        assert_eq!(testbench.handler.mode(), DeviceMode::Off);
    }

    #[test]
    fn test_normal_mode() {
        let mut testbench = PcduTestbench::new();
        testbench
            .tc_tx
            .send(create_request_tc(pcdu::request::Request::Mode(
                DeviceMode::Normal,
            )))
            .unwrap();
        let switch_map_shared = testbench.handler.shared_switch_map.lock().unwrap();
        assert!(switch_map_shared.valid);
        drop(switch_map_shared);
        testbench.handler.periodic_operation(OpCode::RegularOp);
        testbench
            .handler
            .periodic_operation(OpCode::PollAndRecvReplies);
        // Check correctness of mode.
        assert_eq!(testbench.handler.mode(), DeviceMode::Normal);

        testbench.verify_switch_info_req_was_sent(1);
        testbench.verify_switch_reply_received(1, SwitchMapBinaryWrapper::default().0);

        let switch_map_shared = testbench.handler.shared_switch_map.lock().unwrap();
        assert!(switch_map_shared.valid);
        drop(switch_map_shared);
    }

    #[test]
    fn test_switch_request_handling() {
        let mut testbench = PcduTestbench::new();
        testbench
            .tc_tx
            .send(create_request_tc(pcdu::request::Request::Mode(
                DeviceMode::Normal,
            )))
            .unwrap();
        testbench
            .switch_request_tx
            .send(SwitchRequest::new(SwitchId::Mgm0, SwitchStateBinary::On))
            .expect("failed to send switch request");
        testbench.handler.periodic_operation(OpCode::RegularOp);
        testbench
            .handler
            .periodic_operation(OpCode::PollAndRecvReplies);

        testbench.verify_switch_req_was_sent(2, SwitchId::Mgm0, SwitchStateBinary::On);
        testbench.verify_switch_info_req_was_sent(1);
        let mut switch_map = SwitchMapBinaryWrapper::default().0;
        *switch_map
            .get_mut(&SwitchId::Mgm0)
            .expect("switch state setting failed") = SwitchStateBinary::On;
        testbench.verify_switch_reply_received(1, switch_map);

        let switch_map_shared = testbench.handler.shared_switch_map.lock().unwrap();
        assert!(switch_map_shared.valid);
        drop(switch_map_shared);
    }
}
