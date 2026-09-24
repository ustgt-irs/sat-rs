use crate::TmHeader;
use serde::Serialize;
use spacepackets::{
    CcsdsPacketCreationError, CcsdsPacketCreatorWithReservedData, SpHeader, SpacePacketHeader,
    ccsds_packet_len_for_user_data_len_with_checksum,
};

use crate::TcHeader;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcsdsTcPacketOwned {
    pub sp_header: SpacePacketHeader,
    pub tc_header: TcHeader,
    pub payload: alloc::vec::Vec<u8>,
}

impl CcsdsTcPacketOwned {
    pub fn new_with_request<R: serde::Serialize>(
        sp_header: SpacePacketHeader,
        tc_header: TcHeader,
        request: R,
    ) -> Self {
        let request_serialized = postcard::to_allocvec(&request).unwrap();
        Self::new(sp_header, tc_header, request_serialized)
    }

    pub fn new(
        sp_header: SpacePacketHeader,
        tc_header: TcHeader,
        payload: alloc::vec::Vec<u8>,
    ) -> Self {
        Self {
            sp_header,
            tc_header,
            payload,
        }
    }

    pub fn write_to_bytes(&self, buf: &mut [u8]) -> Result<usize, CcsdsCreationError> {
        let response_len =
            postcard::experimental::serialized_size(&self.tc_header)? + self.payload.len();
        let mut ccsds_tc = CcsdsPacketCreatorWithReservedData::new_tc_with_checksum(
            self.sp_header,
            response_len,
            buf,
        )?;
        let user_data = ccsds_tc.packet_data_mut();
        let ser_len = postcard::to_slice(&self.tc_header, user_data)?.len();
        user_data[ser_len..ser_len + self.payload.len()].copy_from_slice(&self.payload);
        let ccsds_packet_len = ccsds_tc.finish();
        Ok(ccsds_packet_len)
    }

    pub fn len_written(&self) -> usize {
        ccsds_packet_len_for_user_data_len_with_checksum(
            postcard::experimental::serialized_size(&self.tc_header).unwrap() as usize
                + self.payload.len(),
        )
        .unwrap()
    }

    pub fn to_vec(&self) -> alloc::vec::Vec<u8> {
        let mut buf = alloc::vec![0u8; self.len_written()];
        let len = self.write_to_bytes(&mut buf).unwrap();
        buf.truncate(len);
        buf
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CcsdsCreationError {
    #[error("CCSDS packet creation error: {0}")]
    CcsdsPacketCreation(#[from] CcsdsPacketCreationError),
    #[error("postcard error: {0}")]
    Postcard(#[from] postcard::Error),
    #[error("timestamp generation error")]
    Time,
}

/// Unserialized owned TM packet which can be cloned and sent around.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CcsdsTmPacketOwned {
    pub sp_header: SpacePacketHeader,
    pub tm_header: TmHeader,
    pub payload: alloc::vec::Vec<u8>,
}

impl CcsdsTmPacketOwned {
    pub fn new_with_serde_payload(
        sp_header: SpHeader,
        tm_header: &TmHeader,
        payload: &impl Serialize,
    ) -> Result<Self, postcard::Error> {
        Ok(CcsdsTmPacketOwned {
            sp_header,
            tm_header: *tm_header,
            payload: postcard::to_allocvec(&payload)?,
        })
    }

    pub fn write_to_bytes(&self, buf: &mut [u8]) -> Result<usize, CcsdsCreationError> {
        let response_len =
            postcard::experimental::serialized_size(&self.tm_header)? + self.payload.len();
        let mut ccsds_tm = CcsdsPacketCreatorWithReservedData::new_tm_with_checksum(
            self.sp_header,
            response_len,
            buf,
        )?;
        let user_data = ccsds_tm.packet_data_mut();
        let ser_len = postcard::to_slice(&self.tm_header, user_data)?.len();
        user_data[ser_len..ser_len + self.payload.len()].copy_from_slice(&self.payload);
        let ccsds_packet_len = ccsds_tm.finish();
        Ok(ccsds_packet_len)
    }

    pub fn len_written(&self) -> usize {
        ccsds_packet_len_for_user_data_len_with_checksum(
            postcard::experimental::serialized_size(&self.tm_header).unwrap() as usize
                + self.payload.len(),
        )
        .unwrap()
    }

    pub fn to_vec(&self) -> alloc::vec::Vec<u8> {
        let mut buf = alloc::vec![0u8; self.len_written()];
        let len = self.write_to_bytes(&mut buf).unwrap();
        buf.truncate(len);
        buf
    }
}
