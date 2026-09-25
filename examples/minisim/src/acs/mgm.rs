use std::f32::consts::PI;

use minisim_types::{acs::mgm, SimReply};
use nexosim::{
    model::{Context, Model},
    ports::Output,
};
use serde::{Deserialize, Serialize};
use types::pcdu::SwitchStateBinary;

use crate::time::current_millis;
// Earth magnetic field varies between roughly -30 uT and 30 uT
const AMPLITUDE_MGM_UT: f32 = 30.0;
// Lets start with a simple frequency here.
const FREQUENCY_MGM: f32 = 1.0;
const PHASE_X: f32 = 0.0;
// Different phases to have different values on the other axes.
const PHASE_Y: f32 = 0.1;
const PHASE_Z: f32 = 0.2;

/// Simple model for a magnetometer where the measure magnetic fields are modeled with sine waves.
///
/// An ideal sensor would sample the magnetic field at a high fixed rate. This might not be
/// possible for a general purpose OS, but self self-sampling at a relatively high rate (20-40 ms)
/// might still be possible and is probably sufficient for many OBSW needs.
#[derive(Serialize, Deserialize)]
pub struct MgmModel {
    id: mgm::Id,
    switch_state: SwitchStateBinary,
    external_mag_field: Option<mgm::SensorValuesMicroTesla>,
    spi_fault: mgm::SpiFault,
    pub reply: Output<SimReply>,
}

#[Model]
impl MgmModel {
    pub fn new(mgm_id: mgm::Id) -> Self {
        Self {
            id: mgm_id,
            switch_state: SwitchStateBinary::Off,
            external_mag_field: None,
            spi_fault: mgm::SpiFault::default(),
            reply: Output::new(),
        }
    }

    pub async fn switch_device(&mut self, switch_state: SwitchStateBinary) {
        self.switch_state = switch_state;
        if switch_state == SwitchStateBinary::Off && self.spi_fault.cleared_by_power_cycle {
            self.spi_fault = mgm::SpiFault::default();
        }
    }

    /// Force (or clear) a stuck-bus SPI fault, for FDIR testing purposes.
    pub async fn set_spi_fault(&mut self, fault: mgm::SpiFault) {
        self.spi_fault = fault;
    }

    pub async fn send_sensor_values(&mut self, _: (), cx: &Context<Self>) {
        let reply = SimReply::Mgm {
            id: self.id,
            reply: create_reply(
                self.switch_state,
                self.calculate_current_mgm_tuple(current_millis(cx.time())),
                self.spi_fault.mode,
            ),
        };
        self.reply.send(reply).await;
    }

    // Devices like magnetorquers generate a strong magnetic field which overrides the default
    // model for the measured magnetic field.
    pub async fn apply_external_magnetic_field(&mut self, field: mgm::SensorValuesMicroTesla) {
        self.external_mag_field = Some(field);
    }

    pub async fn clear_external_magnetic_field(&mut self, _: ()) {
        self.external_mag_field = None;
    }

    fn calculate_current_mgm_tuple(&self, time_ms: u64) -> mgm::SensorValuesMicroTesla {
        if SwitchStateBinary::On == self.switch_state {
            if let Some(ext_field) = self.external_mag_field {
                return ext_field;
            }
            let base_sin_val = 2.0 * PI * FREQUENCY_MGM * (time_ms as f32 / 1000.0);
            return mgm::SensorValuesMicroTesla {
                x: AMPLITUDE_MGM_UT * (base_sin_val + PHASE_X).sin(),
                y: AMPLITUDE_MGM_UT * (base_sin_val + PHASE_Y).sin(),
                z: AMPLITUDE_MGM_UT * (base_sin_val + PHASE_Z).sin(),
            };
        }
        mgm::SensorValuesMicroTesla {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }
}

/// Builds the reply of the simulated LIS3MDL, including the raw register values.
fn create_reply(
    switch_state: SwitchStateBinary,
    sensor_values: mgm::SensorValuesMicroTesla,
    fault_mode: mgm::SpiFaultMode,
) -> mgm::Reply {
    // An injected fault always wins. A switched off device reads back like an undriven bus.
    let raw = match (fault_mode, switch_state) {
        (mgm::SpiFaultMode::AllZeros, _) => mgm::RawValues::splat(mgm::ALL_ZEROS_SENSOR_VAL),
        (mgm::SpiFaultMode::AllOnes, _) | (mgm::SpiFaultMode::None, SwitchStateBinary::Off) => {
            mgm::RawValues::splat(mgm::ALL_ONES_SENSOR_VAL)
        }
        (mgm::SpiFaultMode::None, SwitchStateBinary::On) => {
            raw_values_from_microtesla(sensor_values)
        }
    };
    mgm::Reply {
        switch_state,
        sensor_values,
        raw,
    }
}

fn raw_values_from_microtesla(values: mgm::SensorValuesMicroTesla) -> mgm::RawValues {
    let to_raw = |microtesla: f32| {
        (microtesla / (mgm::GAUSS_TO_MICROTESLA_FACTOR as f32 * mgm::FIELD_LSB_PER_GAUSS_4_SENS))
            .round() as i16
    };
    mgm::RawValues {
        x: to_raw(values.x),
        y: to_raw(values.y),
        z: to_raw(values.z),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use minisim_types::{acs::mgm, SimReply, SimRequest};
    use types::pcdu::{SwitchId, SwitchStateBinary};

    use crate::{
        eps::tests::{switch_device_off, switch_device_on},
        test_helpers::SimTestbench,
    };

    fn request_sensor_data(sim_testbench: &mut SimTestbench, id: mgm::Id) -> mgm::Reply {
        let sim_reply = sim_testbench
            .request_reply(SimRequest::Mgm {
                id,
                request: mgm::Request::RequestSensorData,
            })
            .expect("no MGM reply received");
        let SimReply::Mgm {
            id: reply_id,
            reply,
        } = sim_reply
        else {
            panic!("unexpected reply {sim_reply:?}");
        };
        assert_eq!(reply_id, id);
        reply
    }

    fn inject_spi_fault(sim_testbench: &mut SimTestbench, cleared_by_power_cycle: bool) {
        sim_testbench.send_and_step(SimRequest::Mgm {
            id: mgm::Id::Mgm0,
            request: mgm::Request::SetSpiFault(mgm::SpiFault {
                mode: mgm::SpiFaultMode::AllOnes,
                cleared_by_power_cycle,
            }),
        });
    }

    fn is_stuck_bus_reply(reply: &mgm::Reply) -> bool {
        reply.raw.x == -1 && reply.raw.y == -1 && reply.raw.z == -1
    }

    #[test]
    fn test_basic_mgm_request() {
        let mut sim_testbench = SimTestbench::new();
        let reply = request_sensor_data(&mut sim_testbench, mgm::Id::Mgm0);
        assert_eq!(reply.switch_state, SwitchStateBinary::Off);
        assert_eq!(reply.sensor_values.x, 0.0);
        assert_eq!(reply.sensor_values.y, 0.0);
        assert_eq!(reply.sensor_values.z, 0.0);
    }

    #[test]
    fn test_mgm_spi_fault_injection_all_ones() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);
        inject_spi_fault(&mut sim_testbench, false);

        let reply = request_sensor_data(&mut sim_testbench, mgm::Id::Mgm0);
        // Even though the device is switched on, the injected fault forces a stuck-bus reply.
        assert_eq!(reply.switch_state, SwitchStateBinary::On);
        assert!(is_stuck_bus_reply(&reply));
    }

    #[test]
    fn test_mgm_spi_fault_cleared_by_power_cycle() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);
        inject_spi_fault(&mut sim_testbench, true);
        let reply = request_sensor_data(&mut sim_testbench, mgm::Id::Mgm0);
        assert!(is_stuck_bus_reply(&reply));

        switch_device_off(&mut sim_testbench, SwitchId::Mgm0);
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);
        sim_testbench.step_until(Duration::from_millis(50)).unwrap();
        let reply = request_sensor_data(&mut sim_testbench, mgm::Id::Mgm0);
        assert!(!is_stuck_bus_reply(&reply));
    }

    #[test]
    fn test_mgm_spi_fault_persists_after_power_cycle() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);
        inject_spi_fault(&mut sim_testbench, false);

        switch_device_off(&mut sim_testbench, SwitchId::Mgm0);
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);
        let reply = request_sensor_data(&mut sim_testbench, mgm::Id::Mgm0);
        assert_eq!(reply.switch_state, SwitchStateBinary::On);
        assert!(is_stuck_bus_reply(&reply));
    }

    #[test]
    fn test_basic_mgm_request_switched_on() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgm0);

        let first_reply = request_sensor_data(&mut sim_testbench, mgm::Id::Mgm0);
        sim_testbench.step_until(Duration::from_millis(50)).unwrap();
        let second_reply = request_sensor_data(&mut sim_testbench, mgm::Id::Mgm0);

        let to_microtesla = |raw: i16| {
            raw as f32 * mgm::FIELD_LSB_PER_GAUSS_4_SENS * mgm::GAUSS_TO_MICROTESLA_FACTOR as f32
        };
        let values = second_reply.sensor_values;
        let raw = second_reply.raw;
        for (value, raw) in [(values.x, raw.x), (values.y, raw.y), (values.z, raw.z)] {
            let diff = (value - to_microtesla(raw)).abs();
            assert!(diff < 0.01, "raw value conversion diff too large: {diff}");
        }
        // Check that the values are changing.
        assert_ne!(first_reply, second_reply);
    }

    #[test]
    fn test_mgm_1_request_switched_on() {
        let mut sim_testbench = SimTestbench::new();
        switch_device_on(&mut sim_testbench, SwitchId::Mgm1);

        let mgm_0_reply = request_sensor_data(&mut sim_testbench, mgm::Id::Mgm0);
        assert_eq!(mgm_0_reply.switch_state, SwitchStateBinary::Off);
        let mgm_1_reply = request_sensor_data(&mut sim_testbench, mgm::Id::Mgm1);
        assert_eq!(mgm_1_reply.switch_state, SwitchStateBinary::On);
    }
}
