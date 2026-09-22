use satrs::fdir::FaultCounterStd;
use satrs::health::{HealthState, HealthTableMapSync, HealthTableProvider};
use satrs::spacepackets::CcsdsPacketIdAndPsc;
use satrs_example::{HkHelperSingleSet, TimestampHelper, TmtcQueues};
use satrs_minisim::acs::MgmRequestLis3Mdl;
use satrs_minisim::acs::lis3mdl::{
    FIELD_LSB_PER_GAUSS_4_SENS, GAUSS_TO_MICROTESLA_FACTOR, MgmLis3MdlReply, MgmLis3RawValues,
};
use satrs_minisim::{SerializableSimMsgPayload, SimReply, SimRequest};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use types::acs::mgm::SensorData;
use types::acs::mgm::request::ModeRequest;
use types::acs::mgm::response::ModeResponse;
use types::pcdu::SwitchId;
use types::{ComponentId, DeviceMode, HkRequestType, acs::mgm};

use crate::ccsds::pack_ccsds_tm_packet_for_now;
use crate::device_mode::{ModeTransitionEvent, SwitchAndModeHelper};
use crate::eps::PowerSwitchHelper;

pub const NR_OF_DATA_AND_CFG_REGISTERS: usize = 14;

// Register adresses to access various bytes from the raw reply.
pub const X_LOWBYTE_IDX: usize = 9;
pub const Y_LOWBYTE_IDX: usize = 11;
pub const Z_LOWBYTE_IDX: usize = 13;

// FDIR configuration for a stuck SPI bus (data pinned to all-1s). Chosen so a handful of
// transient errors are tolerated but a persistently faulty bus is caught quickly.
//
// SPI itself cannot time out: the master clocks bytes in lockstep, so a transfer always
// completes. An unresponsive or dead device does not withhold a reply, it just leaves the bus
// floating, which is read back as this same all-1s pattern.
pub const SPI_FAULT_THRESHOLD: u32 = 2;
pub const SPI_FAULT_DECREMENT_AFTER: Duration = Duration::from_secs(30);

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum MgmId {
    _0,
    _1,
}

impl MgmId {
    pub const fn str(&self) -> &str {
        match self {
            MgmId::_0 => "MGM 0",
            MgmId::_1 => "MGM 1",
        }
    }

    #[inline]
    pub const fn component_id(&self) -> ComponentId {
        match self {
            MgmId::_0 => ComponentId::AcsMgm0,
            MgmId::_1 => ComponentId::AcsMgm1,
        }
    }

    #[inline]
    pub const fn switch_id(&self) -> SwitchId {
        match self {
            MgmId::_0 => SwitchId::Mgm0,
            MgmId::_1 => SwitchId::Mgm1,
        }
    }
}

#[derive(Default)]
pub struct SpiDummyInterface {
    pub dummy_values: MgmLis3RawValues,
}

impl SpiDummyInterface {
    fn transfer(&mut self, _tx: &[u8], rx: &mut [u8]) {
        rx[X_LOWBYTE_IDX..X_LOWBYTE_IDX + 2].copy_from_slice(&self.dummy_values.x.to_le_bytes());
        rx[Y_LOWBYTE_IDX..Y_LOWBYTE_IDX + 2].copy_from_slice(&self.dummy_values.y.to_be_bytes());
        rx[Z_LOWBYTE_IDX..Z_LOWBYTE_IDX + 2].copy_from_slice(&self.dummy_values.z.to_be_bytes());
    }
}

#[derive(Default)]
pub struct TestSpiInterface {
    pub call_count: u32,
    pub next_mgm_data: MgmLis3RawValues,
}

impl TestSpiInterface {
    fn transfer(&mut self, _tx: &[u8], rx: &mut [u8]) {
        rx[X_LOWBYTE_IDX..X_LOWBYTE_IDX + 2].copy_from_slice(&self.next_mgm_data.x.to_le_bytes());
        rx[Y_LOWBYTE_IDX..Y_LOWBYTE_IDX + 2].copy_from_slice(&self.next_mgm_data.y.to_le_bytes());
        rx[Z_LOWBYTE_IDX..Z_LOWBYTE_IDX + 2].copy_from_slice(&self.next_mgm_data.z.to_le_bytes());
        self.call_count += 1;
    }
}

pub struct SpiSimInterface {
    pub sim_request_tx: mpsc::Sender<SimRequest>,
    pub sim_reply_rx: mpsc::Receiver<SimReply>,
}

impl SpiSimInterface {
    // Right now, we only support requesting sensor data and not configuration of the sensor.
    fn transfer(&mut self, _tx: &[u8], rx: &mut [u8]) {
        let mgm_sensor_request = MgmRequestLis3Mdl::RequestSensorData;
        if let Err(e) = self
            .sim_request_tx
            .send(SimRequest::new_with_epoch_time(mgm_sensor_request))
        {
            log::error!("failed to send MGM LIS3 request: {e}");
        }
        match self.sim_reply_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(sim_reply) => {
                let sim_reply_lis3 = MgmLis3MdlReply::from_sim_message(&sim_reply)
                    .expect("failed to parse LIS3 reply");
                rx[X_LOWBYTE_IDX..X_LOWBYTE_IDX + 2]
                    .copy_from_slice(&sim_reply_lis3.raw.x.to_le_bytes());
                rx[Y_LOWBYTE_IDX..Y_LOWBYTE_IDX + 2]
                    .copy_from_slice(&sim_reply_lis3.raw.y.to_le_bytes());
                rx[Z_LOWBYTE_IDX..Z_LOWBYTE_IDX + 2]
                    .copy_from_slice(&sim_reply_lis3.raw.z.to_le_bytes());
            }
            Err(e) => {
                log::warn!("MGM LIS3 SIM reply timeout: {e}");
            }
        }
    }
}

pub enum SpiCommunication {
    Dummy(SpiDummyInterface),
    Sim(SpiSimInterface),
    #[allow(dead_code)]
    Test(TestSpiInterface),
}

impl SpiCommunication {
    fn transfer(&mut self, tx: &[u8], rx: &mut [u8]) {
        match self {
            SpiCommunication::Dummy(dummy) => dummy.transfer(tx, rx),
            SpiCommunication::Sim(sim_if) => sim_if.transfer(tx, rx),
            SpiCommunication::Test(test_if) => test_if.transfer(tx, rx),
        }
    }
}

#[derive(Default)]
pub struct BufWrapper {
    tx_buf: [u8; 32],
    rx_buf: [u8; 32],
}

/// Helper component for communication with a parent component, which is usually as assembly.
pub struct ModeLeafHelper {
    pub request_rx: mpsc::Receiver<ModeRequest>,
    pub report_tx: mpsc::SyncSender<ModeResponse>,
}

/// Example MGM device handler strongly based on the LIS3MDL MEMS device.
pub struct MgmHandlerLis3Mdl {
    id: MgmId,
    tmtc_queues: TmtcQueues,
    pub spi_com: SpiCommunication,
    shared_mgm_set: Arc<Mutex<SensorData>>,
    buffers: BufWrapper,
    stamp_helper: TimestampHelper,
    hk_helper: HkHelperSingleSet,
    switch_and_mode_helper: SwitchAndModeHelper<DeviceMode>,
    mode_leaf_helper: ModeLeafHelper,
    spi_fault_counter: FaultCounterStd,
    health_table: HealthTableMapSync,
    event_tx: mpsc::SyncSender<(ComponentId, mgm::Event)>,
}

impl MgmHandlerLis3Mdl {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: MgmId,
        tmtc_queues: TmtcQueues,
        switch_helper: PowerSwitchHelper,
        spi_com: SpiCommunication,
        shared_mgm_set: Arc<Mutex<SensorData>>,
        mode_leaf_helper: ModeLeafHelper,
        mode_timeout: Duration,
        health_table: HealthTableMapSync,
        event_tx: mpsc::SyncSender<(ComponentId, mgm::Event)>,
    ) -> Self {
        Self {
            id,
            tmtc_queues,
            spi_com,
            shared_mgm_set,
            switch_and_mode_helper: SwitchAndModeHelper::new(
                DeviceMode::Off,
                mode_timeout,
                switch_helper,
                id.switch_id(),
            ),
            buffers: BufWrapper::default(),
            stamp_helper: TimestampHelper::default(),
            hk_helper: HkHelperSingleSet::new(false, Duration::from_millis(200)),
            mode_leaf_helper,
            spi_fault_counter: FaultCounterStd::new(SPI_FAULT_THRESHOLD, SPI_FAULT_DECREMENT_AFTER),
            health_table,
            event_tx,
        }
    }

    #[inline]
    pub fn mode(&self) -> DeviceMode {
        self.switch_and_mode_helper.mode()
    }

    pub fn periodic_operation(&mut self) {
        // Update current time.
        self.stamp_helper.update_from_now();

        // Handle requests.
        self.handle_telecommands();

        // Handle assembly related messages.
        self.handle_mode_leaf_handling();

        // Handle mode transitions first.
        if let Some(event) = self.switch_and_mode_helper.handle_mode_transition() {
            match event {
                ModeTransitionEvent::Reached(tc_commander) => {
                    self.handle_mode_reached(tc_commander)
                }
                ModeTransitionEvent::Failed(tc_commander) => {
                    self.handle_mode_transition_failure(tc_commander)
                }
            }
        }

        // Poll sensor before checking and generating HK.
        if self.mode() == DeviceMode::Normal {
            log::trace!("polling LIS3MDL sensor {}", self.id.str());
            self.poll_sensor();
        }

        // Finally check whether any HK generation is necessary.
        if self.hk_helper.needs_generation() {
            self.generate_hk(None);
        }
    }

    pub fn handle_telecommands(&mut self) {
        loop {
            match self.tmtc_queues.tc_rx.try_recv() {
                Ok(packet) => {
                    let tc_id = CcsdsPacketIdAndPsc::new_from_ccsds_packet(&packet.sp_header);
                    match postcard::from_bytes::<mgm::request::Request>(&packet.payload) {
                        Ok(request) => {
                            log::info!(
                                "received request {:?} with TC ID {:#010x}",
                                request,
                                tc_id.raw()
                            );
                            match request {
                                mgm::request::Request::Ping => {
                                    self.send_telemetry(Some(tc_id), mgm::response::Response::Ok)
                                }
                                mgm::request::Request::Hk(hk_request) => {
                                    self.handle_hk_request(Some(tc_id), &hk_request)
                                }
                                mgm::request::Request::Mode(device_mode) => match device_mode {
                                    ModeRequest::SetMode(device_mode) => {
                                        self.start_transition(device_mode, Some(tc_id));
                                    }
                                    ModeRequest::ReadMode => self.send_telemetry(
                                        Some(tc_id),
                                        mgm::response::Response::Mode(ModeResponse::Mode(
                                            self.mode(),
                                        )),
                                    ),
                                },
                                mgm::request::Request::Health(health_request) => {
                                    match health_request {
                                        mgm::request::HealthRequest::SetHealth(health_state) => {
                                            log::info!(
                                                "{}: setting health to {:?} via ground command",
                                                self.id.str(),
                                                health_state
                                            );
                                            self.health_table.set_health(
                                                self.id.component_id().into(),
                                                health_state,
                                            );
                                            self.send_telemetry(
                                                Some(tc_id),
                                                mgm::response::Response::Ok,
                                            );
                                        }
                                    }
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

    pub fn handle_mode_leaf_handling(&mut self) {
        loop {
            match self.mode_leaf_helper.request_rx.try_recv() {
                Ok(request) => match request {
                    ModeRequest::SetMode(device_mode) => self.start_transition(device_mode, None),
                    ModeRequest::ReadMode => self.report_mode_to_parent(),
                },
                Err(e) => match e {
                    std::sync::mpsc::TryRecvError::Empty => break,
                    std::sync::mpsc::TryRecvError::Disconnected => {
                        log::warn!("packet sender disconnected")
                    }
                },
            }
        }
    }

    pub fn send_telemetry(
        &self,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        response: mgm::response::Response,
    ) {
        match pack_ccsds_tm_packet_for_now(self.id.component_id(), tc_id, &response) {
            Ok(packet) => {
                if let Err(e) = self.tmtc_queues.tm_tx.send(packet) {
                    log::warn!("failed to send TM packet: {}", e);
                }
            }
            Err(e) => {
                log::warn!("failed to pack TM packet: {}", e);
            }
        }
    }

    pub fn handle_hk_request(
        &mut self,
        tc_id: Option<CcsdsPacketIdAndPsc>,
        hk_request: &types::acs::mgm::request::HkRequest,
    ) {
        match hk_request.req_type {
            HkRequestType::OneShot => {
                self.generate_hk(tc_id);
            }
            HkRequestType::EnablePeriodic(duration) => {
                self.hk_helper.enabled = true;
                self.hk_helper.frequency = duration;
            }
            HkRequestType::DisablePeriodic => {
                self.hk_helper.enabled = false;
            }
            HkRequestType::ModifyInterval(duration) => {
                self.hk_helper.frequency = duration;
            }
            _ => log::warn!("unhandled HK request"),
        }
    }

    pub fn generate_hk(&self, opt_tc_id: Option<CcsdsPacketIdAndPsc>) {
        let mgm_snapshot = *self.shared_mgm_set.lock().unwrap();
        self.send_telemetry(
            opt_tc_id,
            mgm::response::Response::Hk(mgm::response::HkResponse::MgmData(mgm_snapshot)),
        )
    }

    pub fn poll_sensor(&mut self) {
        // Communicate with the device. This is actually how to read the data from the LIS3 device
        // SPI interface.
        self.spi_com.transfer(
            &self.buffers.tx_buf[0..NR_OF_DATA_AND_CFG_REGISTERS + 1],
            &mut self.buffers.rx_buf[0..NR_OF_DATA_AND_CFG_REGISTERS + 1],
        );
        let x_raw = i16::from_le_bytes(
            self.buffers.rx_buf[X_LOWBYTE_IDX..X_LOWBYTE_IDX + 2]
                .try_into()
                .unwrap(),
        );
        let y_raw = i16::from_le_bytes(
            self.buffers.rx_buf[Y_LOWBYTE_IDX..Y_LOWBYTE_IDX + 2]
                .try_into()
                .unwrap(),
        );
        let z_raw = i16::from_le_bytes(
            self.buffers.rx_buf[Z_LOWBYTE_IDX..Z_LOWBYTE_IDX + 2]
                .try_into()
                .unwrap(),
        );
        // A stuck-high SPI bus (undriven MISO) reads back as all-1s on every register,
        // regardless of what was actually requested. This is the pattern this codebase already
        // uses for "no real device behind the bus" (see the switched-off MGM sim reply). An
        // all-0s reading is not used here, since it collides with a legitimate zero-field
        // reading and would cause false positives.
        if x_raw == -1 && y_raw == -1 && z_raw == -1 {
            self.register_spi_fault();
            return;
        }
        self.spi_fault_counter.try_decrement();
        // Simple scaling to retrieve the float value, assuming the best sensor resolution.
        let mut mgm_guard = self.shared_mgm_set.lock().unwrap();
        mgm_guard.x = x_raw as f32 * GAUSS_TO_MICROTESLA_FACTOR as f32 * FIELD_LSB_PER_GAUSS_4_SENS;
        mgm_guard.y = y_raw as f32 * GAUSS_TO_MICROTESLA_FACTOR as f32 * FIELD_LSB_PER_GAUSS_4_SENS;
        mgm_guard.z = z_raw as f32 * GAUSS_TO_MICROTESLA_FACTOR as f32 * FIELD_LSB_PER_GAUSS_4_SENS;
        mgm_guard.valid = true;
        drop(mgm_guard);
    }

    /// Registers one stuck-bus SPI fault with the FDIR fault counter, invalidating the current
    /// sensor set. If the failure threshold is exceeded, the component is marked faulty in the
    /// global health table.
    fn register_spi_fault(&mut self) {
        log::warn!("{}: stuck-bus SPI fault", self.id.str());
        self.shared_mgm_set.lock().unwrap().valid = false;
        if !self.spi_fault_counter.increment_and_check() {
            return;
        }
        // Ground may have taken manual control, or already given up on this component.
        // Autonomous FDIR should not override that decision.
        let component_id = self.id.component_id().into();
        match self.health_table.health(component_id) {
            Some(HealthState::ExternalControl) | Some(HealthState::PermanentFaulty) => {
                log::info!(
                    "{}: SPI fault threshold exceeded, but health is externally controlled, \
                     not overriding",
                    self.id.str()
                );
            }
            _ => {
                log::error!(
                    "{}: SPI fault threshold exceeded, marking component faulty",
                    self.id.str()
                );
                self.health_table
                    .set_health(component_id, HealthState::Faulty);
                if let Err(e) = self.event_tx.send((
                    self.id.component_id(),
                    mgm::Event::SpiFaultThresholdExceeded,
                )) {
                    log::warn!("{}: failed to send fault event: {}", self.id.str(), e);
                }
                // Do not restart an already pending Off transition: poll_sensor still calls
                // this every cycle the fault persists, and current stays Normal until the
                // transition completes, so re-triggering here would keep resetting the
                // transition state machine before it can ever finish.
                if self.switch_and_mode_helper.target() != Some(DeviceMode::Off) {
                    log::warn!("{}: commanding device off due to fault", self.id.str());
                    self.start_transition(DeviceMode::Off, None);
                }
            }
        }
    }

    fn start_transition(
        &mut self,
        target_mode: DeviceMode,
        tc_commander: Option<CcsdsPacketIdAndPsc>,
    ) {
        log::info!("{}: transitioning to mode {:?}", self.id.str(), target_mode);
        if target_mode == DeviceMode::Off {
            self.shared_mgm_set.lock().unwrap().valid = false;
        }
        self.switch_and_mode_helper
            .start_transition(target_mode, tc_commander);
    }

    // Should be called to complete a mode transition which failed.
    fn handle_mode_transition_failure(&mut self, tc_commander: Option<CcsdsPacketIdAndPsc>) {
        if tc_commander.is_some() {
            self.send_telemetry(
                tc_commander,
                mgm::response::Response::Mode(ModeResponse::SetModeTimeout),
            );
        }
        self.mode_leaf_helper
            .report_tx
            .send(ModeResponse::SetModeTimeout)
            .unwrap();
    }

    // Should be called to complete a mode transition successfully.
    fn handle_mode_reached(&mut self, tc_commander: Option<CcsdsPacketIdAndPsc>) {
        self.announce_mode();
        if let Some(requestor) = tc_commander {
            self.send_mode_tm(requestor);
        }
        // Inform our parent about mode changes.
        self.report_mode_to_parent();
    }

    fn announce_mode(&self) {
        log::info!("{} announcing mode: {:?}", self.id.str(), self.mode());
        if let Err(e) = self
            .event_tx
            .send((self.id.component_id(), mgm::Event::ModeChanged(self.mode())))
        {
            log::warn!(
                "{}: failed to send mode changed event: {}",
                self.id.str(),
                e
            );
        }
    }

    fn report_mode_to_parent(&self) {
        self.mode_leaf_helper
            .report_tx
            .send(ModeResponse::Mode(self.mode()))
            .unwrap();
    }

    fn send_mode_tm(&self, requestor: CcsdsPacketIdAndPsc) {
        self.send_telemetry(Some(requestor), mgm::response::Response::Ok);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        mpsc::{self, TryRecvError},
    };

    use arbitrary_int::u11;
    use satrs::spacepackets::SpacePacketHeader;
    use satrs_minisim::acs::lis3mdl::MgmLis3RawValues;
    use types::{
        Apid, ComponentId, TcHeader,
        acs::mgm::request::HkRequest,
        ccsds::{CcsdsTcPacketOwned, CcsdsTmPacketOwned},
        pcdu::{SwitchRequest, SwitchState, SwitchStateBinary},
    };

    use crate::eps::pcdu::{SharedSwitchSet, SwitchMap, SwitchSet};

    use super::*;

    #[derive(Debug, Copy, Clone)]
    pub enum MgmSelect {
        _0,
        _1,
    }

    impl MgmSelect {
        pub fn id(&self) -> ComponentId {
            match self {
                MgmSelect::_0 => ComponentId::AcsMgm0,
                MgmSelect::_1 => ComponentId::AcsMgm1,
            }
        }
    }

    pub fn create_request_tc(
        select: MgmSelect,
        request: types::acs::mgm::request::Request,
    ) -> types::ccsds::CcsdsTcPacketOwned {
        types::ccsds::CcsdsTcPacketOwned::new_with_request(
            SpacePacketHeader::new_from_apid(u11::new(Apid::Acs as u16)),
            TcHeader::new(select.id(), types::MessageType::Ping),
            request,
        )
    }

    #[allow(dead_code)]
    pub struct MgmTestbench {
        pub assembly_mode_request_tx: mpsc::SyncSender<ModeRequest>,
        pub mode_report_rx: mpsc::Receiver<ModeResponse>,
        pub shared_switch_set: SharedSwitchSet,
        pub tc_tx: mpsc::SyncSender<CcsdsTcPacketOwned>,
        pub tm_rx: mpsc::Receiver<CcsdsTmPacketOwned>,
        pub switch_rx: mpsc::Receiver<SwitchRequest>,
        pub health_table: HealthTableMapSync,
        pub event_rx: mpsc::Receiver<(ComponentId, mgm::Event)>,
        pub handler: MgmHandlerLis3Mdl,
    }

    impl MgmTestbench {
        pub fn new() -> Self {
            let (assembly_mode_request_tx, assembly_mode_request_rx) = mpsc::sync_channel(5);
            let (mode_report_tx, mode_report_rx) = mpsc::sync_channel(5);
            let mode_leaf_helper = ModeLeafHelper {
                request_rx: assembly_mode_request_rx,
                report_tx: mode_report_tx,
            };
            let (tc_tx, tc_rx) = mpsc::sync_channel(10);
            let (tm_tx, tm_rx) = mpsc::sync_channel(10);
            let (switcher_tx, switch_rx) = mpsc::sync_channel(10);
            let shared_mgm_set = Arc::default();
            let mut switch_map = SwitchMap::new();
            switch_map.insert(SwitchId::Mgm0, SwitchState::Off);
            let switch_map = SwitchSet::new(switch_map);
            let shared_switch_set = SharedSwitchSet::new(Mutex::new(switch_map));
            let health_table = HealthTableMapSync::default();
            let (event_tx, event_rx) = mpsc::sync_channel(5);
            let handler = MgmHandlerLis3Mdl::new(
                MgmId::_0,
                TmtcQueues { tc_rx, tm_tx },
                PowerSwitchHelper::new(switcher_tx, shared_switch_set.clone()),
                SpiCommunication::Test(TestSpiInterface::default()),
                shared_mgm_set,
                mode_leaf_helper,
                Duration::from_millis(100),
                health_table.clone(),
                event_tx,
            );
            Self {
                assembly_mode_request_tx,
                mode_report_rx,
                shared_switch_set,
                switch_rx,
                health_table,
                event_rx,
                handler,
                tm_rx,
                tc_tx,
            }
        }

        /// Switches the MGM to `Normal` mode, completing the power-switch handshake.
        pub fn switch_to_normal(&mut self) {
            self.tc_tx
                .send(create_request_tc(
                    MgmSelect::_0,
                    mgm::request::Request::Mode(ModeRequest::SetMode(DeviceMode::Normal)),
                ))
                .unwrap();
            self.handler.periodic_operation();
            self.shared_switch_set
                .lock()
                .unwrap()
                .set_switch_state(SwitchId::Mgm0, SwitchState::On);
            self.handler.periodic_operation();
            assert_eq!(self.handler.mode(), DeviceMode::Normal);
        }

        pub fn test_spi_interface(&mut self) -> &mut TestSpiInterface {
            match &mut self.handler.spi_com {
                SpiCommunication::Dummy(_) | SpiCommunication::Sim(_) => {
                    panic!("unexpected SPI interface")
                }
                SpiCommunication::Test(test_spi_interface) => test_spi_interface,
            }
        }
    }

    #[test]
    fn test_basic_handler() {
        let mut testbench = MgmTestbench::new();
        assert_eq!(testbench.test_spi_interface().call_count, 0);
        assert_eq!(testbench.handler.mode(), DeviceMode::Off);
        testbench.handler.periodic_operation();
        // Handler is OFF, no changes expected.
        assert_eq!(testbench.test_spi_interface().call_count, 0);
        assert_eq!(testbench.handler.mode(), DeviceMode::Off);
    }

    #[test]
    fn test_normal_handler() {
        let mut testbench = MgmTestbench::new();
        testbench
            .tc_tx
            .send(create_request_tc(
                MgmSelect::_0,
                mgm::request::Request::Mode(ModeRequest::SetMode(DeviceMode::Normal)),
            ))
            .unwrap();
        testbench.handler.periodic_operation();
        assert_eq!(testbench.handler.mode(), DeviceMode::Off);

        // Verify power switch handling.
        let switch_req = testbench.switch_rx.try_recv().expect("no switch request");
        assert_eq!(switch_req.switch_id, SwitchId::Mgm0);
        assert_eq!(switch_req.target_state, SwitchStateBinary::On);

        // This simulates one cycle for the power switch to update.
        testbench
            .shared_switch_set
            .lock()
            .unwrap()
            .set_switch_state(SwitchId::Mgm0, SwitchState::On);

        // Now the power switch is updated and the mode request should be completed.
        testbench.handler.periodic_operation();

        assert_eq!(testbench.handler.mode(), DeviceMode::Normal);

        let tm_packet = testbench.tm_rx.try_recv().expect("no mode reply generated");

        assert_eq!(tm_packet.tm_header.sender_id, ComponentId::AcsMgm0);

        let response =
            postcard::from_bytes::<types::acs::mgm::response::Response>(&tm_packet.payload)
                .expect("failed to deserialize mode reply");
        matches!(response, types::acs::mgm::response::Response::Ok);

        let (sender_id, event) = testbench
            .event_rx
            .try_recv()
            .expect("expected mode changed event");
        assert_eq!(sender_id, ComponentId::AcsMgm0);
        assert!(matches!(event, mgm::Event::ModeChanged(DeviceMode::Normal)));
        // The device should have been polled once.
        assert_eq!(testbench.test_spi_interface().call_count, 1);
        let mgm_set = *testbench.handler.shared_mgm_set.lock().unwrap();
        assert!(mgm_set.x < 0.001);
        assert!(mgm_set.y < 0.001);
        assert!(mgm_set.z < 0.001);
        assert!(mgm_set.valid);

        matches!(testbench.tm_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn test_normal_handler_mgm_set_conversion() {
        let mut testbench = MgmTestbench::new();
        let raw_values = MgmLis3RawValues {
            x: 1000,
            y: -1000,
            z: 1000,
        };
        testbench.test_spi_interface().next_mgm_data = raw_values;
        testbench
            .tc_tx
            .send(create_request_tc(
                MgmSelect::_0,
                mgm::request::Request::Mode(ModeRequest::SetMode(DeviceMode::Normal)),
            ))
            .unwrap();
        testbench.handler.periodic_operation();

        // This simulates one cycle for the power switch to update.
        testbench
            .shared_switch_set
            .lock()
            .unwrap()
            .set_switch_state(SwitchId::Mgm0, SwitchState::On);

        // Now the power switch is updated and the mode request should be completed.
        testbench.handler.periodic_operation();

        let mgm_set = *testbench.handler.shared_mgm_set.lock().unwrap();
        let expected_x =
            raw_values.x as f32 * GAUSS_TO_MICROTESLA_FACTOR as f32 * FIELD_LSB_PER_GAUSS_4_SENS;
        let expected_y =
            raw_values.y as f32 * GAUSS_TO_MICROTESLA_FACTOR as f32 * FIELD_LSB_PER_GAUSS_4_SENS;
        let expected_z =
            raw_values.z as f32 * GAUSS_TO_MICROTESLA_FACTOR as f32 * FIELD_LSB_PER_GAUSS_4_SENS;
        let x_diff = (mgm_set.x - expected_x).abs();
        let y_diff = (mgm_set.y - expected_y).abs();
        let z_diff = (mgm_set.z - expected_z).abs();
        assert!(x_diff < 0.001, "x diff too large: {}", x_diff);
        assert!(y_diff < 0.001, "y diff too large: {}", y_diff);
        assert!(z_diff < 0.001, "z diff too large: {}", z_diff);
        assert!(mgm_set.valid);
    }

    #[test]
    fn test_hk_one_shot_device_off() {
        let mut testbench = MgmTestbench::new();
        // Device handler is initially off, first set will be invalid.
        testbench
            .tc_tx
            .send(create_request_tc(
                MgmSelect::_0,
                mgm::request::Request::Hk(HkRequest {
                    id: mgm::request::HkId::Sensor,
                    req_type: HkRequestType::OneShot,
                }),
            ))
            .unwrap();
        testbench.handler.periodic_operation();

        // This simulates one cycle for the power switch to update.
        testbench
            .shared_switch_set
            .lock()
            .unwrap()
            .set_switch_state(SwitchId::Mgm0, SwitchState::On);

        // Now the power switch is updated and the mode request should be completed.
        testbench.handler.periodic_operation();

        let tm_packet = testbench.tm_rx.try_recv().expect("no mode reply generated");

        assert_eq!(tm_packet.tm_header.sender_id, ComponentId::AcsMgm0);

        let response =
            postcard::from_bytes::<types::acs::mgm::response::Response>(&tm_packet.payload)
                .expect("failed to deserialize mode reply");
        if let types::acs::mgm::response::Response::Hk(mgm::response::HkResponse::MgmData(data)) =
            response
        {
            assert_eq!(data.valid, false);
            assert!(data.x < 0.001);
            assert!(data.y < 0.001);
            assert!(data.z < 0.001);
        } else {
            panic!("expected hk response");
        }

        matches!(testbench.tm_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn test_hk_device_normal() {
        let mut testbench = MgmTestbench::new();
        testbench
            .tc_tx
            .send(create_request_tc(
                MgmSelect::_0,
                mgm::request::Request::Mode(ModeRequest::SetMode(DeviceMode::Normal)),
            ))
            .unwrap();
        // This simulates one cycle for the power switch to update.
        testbench
            .shared_switch_set
            .lock()
            .unwrap()
            .set_switch_state(SwitchId::Mgm0, SwitchState::On);
        testbench.handler.periodic_operation();
        assert_eq!(testbench.handler.mode(), DeviceMode::Normal);

        testbench
            .tc_tx
            .send(create_request_tc(
                MgmSelect::_0,
                mgm::request::Request::Hk(HkRequest {
                    id: mgm::request::HkId::Sensor,
                    req_type: HkRequestType::OneShot,
                }),
            ))
            .unwrap();
        testbench.handler.periodic_operation();

        let mode_tm = testbench.tm_rx.try_recv().expect("no mode reply generated");

        assert_eq!(mode_tm.tm_header.sender_id, ComponentId::AcsMgm0);

        let response =
            postcard::from_bytes::<types::acs::mgm::response::Response>(&mode_tm.payload)
                .expect("failed to deserialize mode reply");
        matches!(response, types::acs::mgm::response::Response::Ok);

        let hk_tm = testbench.tm_rx.try_recv().expect("no hk reply generated");

        assert_eq!(hk_tm.tm_header.sender_id, ComponentId::AcsMgm0);

        let response = postcard::from_bytes::<types::acs::mgm::response::Response>(&hk_tm.payload)
            .expect("failed to deserialize mode reply");
        if let types::acs::mgm::response::Response::Hk(mgm::response::HkResponse::MgmData(data)) =
            response
        {
            // Set is now valid.
            assert_eq!(data.valid, true);
            assert!(data.x < 0.001);
            assert!(data.y < 0.001);
            assert!(data.z < 0.001);
        } else {
            panic!("expected hk response");
        }

        matches!(testbench.tm_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn test_spi_fault_below_threshold_stays_healthy() {
        let mut testbench = MgmTestbench::new();
        testbench.switch_to_normal();
        testbench.test_spi_interface().next_mgm_data = MgmLis3RawValues {
            x: -1,
            y: -1,
            z: -1,
        };
        // One stuck-bus reading should not be enough to trip SPI_FAULT_THRESHOLD.
        testbench.handler.periodic_operation();
        assert_eq!(
            testbench.health_table.health(ComponentId::AcsMgm0.into()),
            None,
            "component should not be marked faulty yet"
        );
        assert!(!testbench.handler.shared_mgm_set.lock().unwrap().valid);
    }

    #[test]
    fn test_spi_fault_above_threshold_marks_component_faulty() {
        let mut testbench = MgmTestbench::new();
        testbench.switch_to_normal();
        // Drain the mode changed event emitted by switch_to_normal().
        testbench
            .event_rx
            .try_recv()
            .expect("expected mode changed event");
        testbench.test_spi_interface().next_mgm_data = MgmLis3RawValues {
            x: -1,
            y: -1,
            z: -1,
        };
        // SPI_FAULT_THRESHOLD is exceeded on the (threshold + 1)-th stuck-bus reading.
        for _ in 0..SPI_FAULT_THRESHOLD + 1 {
            testbench.handler.periodic_operation();
        }
        assert_eq!(
            testbench.health_table.health(ComponentId::AcsMgm0.into()),
            Some(HealthState::Faulty)
        );
        assert!(!testbench.handler.shared_mgm_set.lock().unwrap().valid);
        let (sender_id, event) = testbench.event_rx.try_recv().expect("expected fault event");
        assert_eq!(sender_id, ComponentId::AcsMgm0);
        assert!(matches!(event, mgm::Event::SpiFaultThresholdExceeded));
    }

    #[test]
    fn test_spi_fault_above_threshold_commands_device_off() {
        let mut testbench = MgmTestbench::new();
        testbench.switch_to_normal();
        // Drain the switch-on request left over from switch_to_normal().
        testbench
            .switch_rx
            .try_recv()
            .expect("no switch-on request sent");
        testbench.test_spi_interface().next_mgm_data = MgmLis3RawValues {
            x: -1,
            y: -1,
            z: -1,
        };
        for _ in 0..SPI_FAULT_THRESHOLD + 1 {
            testbench.handler.periodic_operation();
        }
        assert_eq!(
            testbench.health_table.health(ComponentId::AcsMgm0.into()),
            Some(HealthState::Faulty)
        );
        // The Off transition was only started on the last iteration above, so it has not sent
        // its switch-off request yet: drive one more cycle to let it do so.
        testbench.handler.periodic_operation();
        let switch_req = testbench
            .switch_rx
            .try_recv()
            .expect("no switch-off request sent after fault");
        assert_eq!(switch_req.switch_id, SwitchId::Mgm0);
        assert_eq!(switch_req.target_state, SwitchStateBinary::Off);

        // Simulate the PCDU acting on the switch-off request.
        testbench
            .shared_switch_set
            .lock()
            .unwrap()
            .set_switch_state(SwitchId::Mgm0, SwitchState::Off);
        testbench.handler.periodic_operation();

        assert_eq!(testbench.handler.mode(), DeviceMode::Off);
    }

    #[test]
    fn test_spi_fault_does_not_override_external_control() {
        let mut testbench = MgmTestbench::new();
        testbench.switch_to_normal();
        testbench
            .health_table
            .set_health(ComponentId::AcsMgm0.into(), HealthState::ExternalControl);
        testbench.test_spi_interface().next_mgm_data = MgmLis3RawValues {
            x: -1,
            y: -1,
            z: -1,
        };
        for _ in 0..SPI_FAULT_THRESHOLD + 1 {
            testbench.handler.periodic_operation();
        }
        // Ground took manual control; autonomous FDIR must not override that decision.
        assert_eq!(
            testbench.health_table.health(ComponentId::AcsMgm0.into()),
            Some(HealthState::ExternalControl)
        );
    }

    #[test]
    fn test_recovering_from_spi_fault_clears_invalid_data_flag() {
        let mut testbench = MgmTestbench::new();
        testbench.switch_to_normal();
        testbench.test_spi_interface().next_mgm_data = MgmLis3RawValues {
            x: -1,
            y: -1,
            z: -1,
        };
        testbench.handler.periodic_operation();
        assert!(!testbench.handler.shared_mgm_set.lock().unwrap().valid);

        // Bus recovers before the threshold is exceeded.
        testbench.test_spi_interface().next_mgm_data = MgmLis3RawValues::default();
        testbench.handler.periodic_operation();
        assert_eq!(
            testbench.health_table.health(ComponentId::AcsMgm0.into()),
            None
        );
        assert!(testbench.handler.shared_mgm_set.lock().unwrap().valid);
    }
}
