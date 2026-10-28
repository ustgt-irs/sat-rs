use arbitrary_int::u11;
use spacepackets::ByteConversionError;
use spacepackets::SpHeader;
use spacepackets::ecss::CreatorConfig;
use spacepackets::ecss::EcssEnumeration;
use spacepackets::ecss::MessageTypeId;
use spacepackets::ecss::tm::PusTmCreator;
use spacepackets::ecss::tm::PusTmSecondaryHeader;

#[cfg(feature = "alloc")]
pub use alloc_mod::*;

pub use spacepackets::ecss::event::*;

pub struct EventReportCreator {
    apid: u11,
    pub dest_id: u16,
}

impl EventReportCreator {
    pub fn new(apid: u11, dest_id: u16) -> Self {
        Self { dest_id, apid }
    }

    pub fn event_info<'time, 'src_data>(
        &self,
        time_stamp: &'time [u8],
        event_id: impl EcssEnumeration,
        params: Option<&'src_data [u8]>,
        src_data_buf: &'src_data mut [u8],
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        self.generate_and_send_generic_tm(
            MessageSubtypeId::TmInfoReport,
            time_stamp,
            event_id,
            params,
            src_data_buf,
        )
    }

    pub fn event_low_severity<'time, 'src_data>(
        &self,
        time_stamp: &'time [u8],
        event_id: impl EcssEnumeration,
        params: Option<&'src_data [u8]>,
        src_data_buf: &'src_data mut [u8],
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        self.generate_and_send_generic_tm(
            MessageSubtypeId::TmLowSeverityReport,
            time_stamp,
            event_id,
            params,
            src_data_buf,
        )
    }

    pub fn event_medium_severity<'time, 'src_data>(
        &self,
        time_stamp: &'time [u8],
        event_id: impl EcssEnumeration,
        params: Option<&'src_data [u8]>,
        buf: &'src_data mut [u8],
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        self.generate_and_send_generic_tm(
            MessageSubtypeId::TmMediumSeverityReport,
            time_stamp,
            event_id,
            params,
            buf,
        )
    }

    pub fn event_high_severity<'time, 'src_data>(
        &self,
        time_stamp: &'time [u8],
        event_id: impl EcssEnumeration,
        params: Option<&'src_data [u8]>,
        src_data_buf: &'src_data mut [u8],
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        self.generate_and_send_generic_tm(
            MessageSubtypeId::TmHighSeverityReport,
            time_stamp,
            event_id,
            params,
            src_data_buf,
        )
    }

    fn generate_and_send_generic_tm<'time, 'src_data>(
        &self,
        subservice: MessageSubtypeId,
        time_stamp: &'time [u8],
        event_id: impl EcssEnumeration,
        params: Option<&'src_data [u8]>,
        src_data_buf: &'src_data mut [u8],
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        self.generate_generic_event_tm(subservice, time_stamp, event_id, params, src_data_buf)
    }

    fn generate_generic_event_tm<'time, 'src_data>(
        &self,
        subservice: MessageSubtypeId,
        time_stamp: &'time [u8],
        event_id: impl EcssEnumeration,
        params: Option<&'src_data [u8]>,
        src_data_buf: &'src_data mut [u8],
    ) -> Result<PusTmCreator<'time, 'src_data>, ByteConversionError> {
        let mut src_data_len = event_id.size();
        if let Some(aux_data) = params {
            src_data_len += aux_data.len();
        }
        super::source_buffer_large_enough(src_data_buf.len(), src_data_len)?;
        let sec_header = PusTmSecondaryHeader::new(
            MessageTypeId::new(5, subservice.into()),
            0,
            self.dest_id,
            time_stamp,
        );
        let mut current_idx = 0;
        event_id.write_to_slice_be(&mut src_data_buf[0..event_id.size()])?;
        current_idx += event_id.size();
        if let Some(aux_data) = params {
            src_data_buf[current_idx..current_idx + aux_data.len()].copy_from_slice(aux_data);
            current_idx += aux_data.len();
        }
        Ok(PusTmCreator::new(
            SpHeader::new_from_apid(self.apid),
            sec_header,
            &src_data_buf[0..current_idx],
            CreatorConfig::default(),
        ))
    }
}

#[cfg(feature = "alloc")]
mod alloc_mod {
    use super::*;
    use crate::ComponentId;
    use crate::legacy::pus::{EcssTmSender, EcssTmtcError};
    use alloc::vec;
    use alloc::vec::Vec;
    use core::cell::RefCell;
    use spacepackets::ecss::PusError;

    pub trait EventTmHook {
        fn modify_tm(&self, tm: &mut PusTmCreator);
    }

    #[derive(Default)]
    pub struct DummyEventHook {}

    impl EventTmHook for DummyEventHook {
        fn modify_tm(&self, _tm: &mut PusTmCreator) {}
    }

    pub struct EventReporter<EventTmHookInstance: EventTmHook = DummyEventHook> {
        id: ComponentId,
        // Use interior mutability pattern here. This is just an intermediate buffer to the PUS event packet
        // generation.
        source_data_buf: RefCell<Vec<u8>>,
        pub report_creator: EventReportCreator,
        pub tm_hook: EventTmHookInstance,
    }

    impl EventReporter<DummyEventHook> {
        pub fn new(
            id: ComponentId,
            default_apid: u11,
            default_dest_id: u16,
            max_event_id_and_aux_data_size: usize,
        ) -> Self {
            let reporter = EventReportCreator::new(default_apid, default_dest_id);
            Self {
                id,
                source_data_buf: RefCell::new(vec![0; max_event_id_and_aux_data_size]),
                report_creator: reporter,
                tm_hook: DummyEventHook::default(),
            }
        }
    }
    impl<EventTmHookInstance: EventTmHook> EventReporter<EventTmHookInstance> {
        pub fn new_with_hook(
            id: ComponentId,
            default_apid: u11,
            default_dest_id: u16,
            max_event_id_and_aux_data_size: usize,
            tm_hook: EventTmHookInstance,
        ) -> Self {
            let reporter = EventReportCreator::new(default_apid, default_dest_id);
            Self {
                id,
                source_data_buf: RefCell::new(vec![0; max_event_id_and_aux_data_size]),
                report_creator: reporter,
                tm_hook,
            }
        }

        pub fn event_info(
            &self,
            sender: &(impl EcssTmSender + ?Sized),
            time_stamp: &[u8],
            event_id: impl EcssEnumeration,
            params: Option<&[u8]>,
        ) -> Result<(), EcssTmtcError> {
            let mut mut_buf = self.source_data_buf.borrow_mut();
            let mut tm_creator = self
                .report_creator
                .event_info(time_stamp, event_id, params, mut_buf.as_mut_slice())
                .map_err(PusError::ByteConversion)?;
            self.tm_hook.modify_tm(&mut tm_creator);
            sender.send_tm(self.id, tm_creator.into())?;
            Ok(())
        }

        pub fn event_low_severity(
            &self,
            sender: &(impl EcssTmSender + ?Sized),
            time_stamp: &[u8],
            event_id: impl EcssEnumeration,
            params: Option<&[u8]>,
        ) -> Result<(), EcssTmtcError> {
            let mut mut_buf = self.source_data_buf.borrow_mut();
            let mut tm_creator = self
                .report_creator
                .event_low_severity(time_stamp, event_id, params, mut_buf.as_mut_slice())
                .map_err(PusError::ByteConversion)?;
            self.tm_hook.modify_tm(&mut tm_creator);
            sender.send_tm(self.id, tm_creator.into())?;
            Ok(())
        }

        pub fn event_medium_severity(
            &self,
            sender: &(impl EcssTmSender + ?Sized),
            time_stamp: &[u8],
            event_id: impl EcssEnumeration,
            params: Option<&[u8]>,
        ) -> Result<(), EcssTmtcError> {
            let mut mut_buf = self.source_data_buf.borrow_mut();
            let mut tm_creator = self
                .report_creator
                .event_medium_severity(time_stamp, event_id, params, mut_buf.as_mut_slice())
                .map_err(PusError::ByteConversion)?;
            self.tm_hook.modify_tm(&mut tm_creator);
            sender.send_tm(self.id, tm_creator.into())?;
            Ok(())
        }

        pub fn event_high_severity(
            &self,
            sender: &(impl EcssTmSender + ?Sized),
            time_stamp: &[u8],
            event_id: impl EcssEnumeration,
            params: Option<&[u8]>,
        ) -> Result<(), EcssTmtcError> {
            let mut mut_buf = self.source_data_buf.borrow_mut();
            let mut tm_creator = self
                .report_creator
                .event_high_severity(time_stamp, event_id, params, mut_buf.as_mut_slice())
                .map_err(PusError::ByteConversion)?;
            self.tm_hook.modify_tm(&mut tm_creator);
            sender.send_tm(self.id, tm_creator.into())?;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {}
