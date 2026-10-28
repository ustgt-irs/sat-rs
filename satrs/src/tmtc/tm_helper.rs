use arbitrary_int::{u11, u14};
use spacepackets::SpHeader;
use spacepackets::ecss::tm::{PusTmCreator, PusTmSecondaryHeader};
use spacepackets::ecss::{CreatorConfig, MessageTypeId};
use spacepackets::time::cds::CdsTime;

pub struct PusTmWithCdsShortHelper {
    apid: u11,
    cds_short_buf: [u8; 7],
}

impl PusTmWithCdsShortHelper {
    pub fn new(apid: u11) -> Self {
        Self {
            apid,
            cds_short_buf: [0; 7],
        }
    }

    #[cfg(feature = "std")]
    pub fn create_pus_tm_timestamp_now<'data>(
        &mut self,
        service: u8,
        subservice: u8,
        source_data: &'data [u8],
        seq_count: u14,
    ) -> PusTmCreator<'_, 'data> {
        let time_stamp = CdsTime::now_with_u16_days().unwrap();
        time_stamp.write_to_slice(&mut self.cds_short_buf).unwrap();
        self.create_pus_tm_common(service, subservice, source_data, seq_count)
    }

    pub fn create_pus_tm_with_stamper<'data>(
        &mut self,
        service: u8,
        subservice: u8,
        source_data: &'data [u8],
        stamper: &CdsTime,
        seq_count: u14,
    ) -> PusTmCreator<'_, 'data> {
        stamper.write_to_slice(&mut self.cds_short_buf).unwrap();
        self.create_pus_tm_common(service, subservice, source_data, seq_count)
    }

    fn create_pus_tm_common<'data>(
        &self,
        service: u8,
        subservice: u8,
        source_data: &'data [u8],
        seq_count: u14,
    ) -> PusTmCreator<'_, 'data> {
        let reply_header = SpHeader::new_for_unseg_tm(self.apid, seq_count, 0);
        let tc_header = PusTmSecondaryHeader::new_simple(
            MessageTypeId::new(service, subservice),
            &self.cds_short_buf,
        );
        PusTmCreator::new(
            reply_header,
            tc_header,
            source_data,
            CreatorConfig::default(),
        )
    }
}

#[cfg(test)]
mod tests {
    use arbitrary_int::{u11, u14};
    use spacepackets::{CcsdsPacket, ecss::PusPacket, time::cds::CdsTime};

    use super::PusTmWithCdsShortHelper;

    #[test]
    fn test_helper_with_stamper() {
        let mut pus_tm_helper = PusTmWithCdsShortHelper::new(u11::new(0x123));
        let stamper = CdsTime::new_with_u16_days(0, 0);
        let tm =
            pus_tm_helper.create_pus_tm_with_stamper(17, 1, &[1, 2, 3, 4], &stamper, u14::new(25));
        assert_eq!(tm.service_type_id(), 17);
        assert_eq!(tm.message_subtype_id(), 1);
        assert_eq!(tm.user_data(), &[1, 2, 3, 4]);
        assert_eq!(tm.seq_count().value(), 25);
        assert_eq!(tm.timestamp(), [64, 0, 0, 0, 0, 0, 0])
    }

    #[test]
    fn test_helper_from_now() {
        let mut pus_tm_helper = PusTmWithCdsShortHelper::new(u11::new(0x123));
        let tm = pus_tm_helper.create_pus_tm_timestamp_now(17, 1, &[1, 2, 3, 4], u14::new(25));
        assert_eq!(tm.service_type_id(), 17);
        assert_eq!(tm.message_subtype_id(), 1);
        assert_eq!(tm.user_data(), &[1, 2, 3, 4]);
        assert_eq!(tm.seq_count().value(), 25);
        assert_eq!(tm.timestamp().len(), 7);
    }
}
