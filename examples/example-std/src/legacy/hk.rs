use arbitrary_int::traits::Integer as _;
use derive_new::new;
use satrs::hk::UniqueId;
use satrs::request::UniqueApidTargetId;
use satrs::spacepackets::ecss::tm::{PusTmCreator, PusTmSecondaryHeader};
use satrs::spacepackets::ecss::{hk, CreatorConfig, MessageTypeId};
use satrs::spacepackets::{ByteConversionError, SpHeader};

#[derive(Debug, new, Copy, Clone)]
pub struct HkUniqueId {
    target_id: UniqueApidTargetId,
    set_id: UniqueId,
}

impl HkUniqueId {
    #[allow(dead_code)]
    pub fn target_id(&self) -> UniqueApidTargetId {
        self.target_id
    }
    #[allow(dead_code)]
    pub fn set_id(&self) -> UniqueId {
        self.set_id
    }

    #[allow(dead_code)]
    pub fn write_to_be_bytes(&self, buf: &mut [u8]) -> Result<usize, ByteConversionError> {
        if buf.len() < 8 {
            return Err(ByteConversionError::ToSliceTooSmall {
                found: buf.len(),
                expected: 8,
            });
        }
        buf[0..4].copy_from_slice(&self.target_id.unique_id.as_u32().to_be_bytes());
        buf[4..8].copy_from_slice(&self.set_id.to_be_bytes());

        Ok(8)
    }
}

#[derive(new)]
pub struct PusHkHelper {
    component_id: UniqueApidTargetId,
}

impl PusHkHelper {
    pub fn generate_hk_report_packet<
        'a,
        'b,
        HkWriter: FnMut(&mut [u8]) -> Result<usize, ByteConversionError>,
    >(
        &self,
        timestamp: &'a [u8],
        set_id: u32,
        hk_data_writer: &mut HkWriter,
        buf: &'b mut [u8],
    ) -> Result<PusTmCreator<'a, 'b>, ByteConversionError> {
        let sec_header = PusTmSecondaryHeader::new(
            MessageTypeId::new(3, hk::MessageSubtypeId::TmHkPacket as u8),
            0,
            0,
            timestamp,
        );
        buf[0..4].copy_from_slice(&self.component_id.unique_id.as_u32().to_be_bytes());
        buf[4..8].copy_from_slice(&set_id.to_be_bytes());
        let (_, second_half) = buf.split_at_mut(8);
        let hk_data_len = hk_data_writer(second_half)?;
        Ok(PusTmCreator::new(
            SpHeader::new_from_apid(self.component_id.apid),
            sec_header,
            &buf[0..8 + hk_data_len],
            CreatorConfig::default(),
        ))
    }
}
