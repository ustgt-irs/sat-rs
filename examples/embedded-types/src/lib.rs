#![no_std]

use spacepackets::{
    CcsdsPacketCreationError, CcsdsPacketCreatorWithReservedData, CcsdsPacketIdAndPsc,
    SpacePacketHeader, ccsds_packet_len_for_user_data_len_with_checksum,
};

pub mod stm32f3 {
    use arbitrary_int::u11;
    use core::time::Duration;

    pub const PUS_APID: u11 = u11::new(0x02);

    #[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize)]
    #[cfg_attr(feature = "defmt", derive(defmt::Format))]
    pub enum Request {
        Ping,
        ChangeBlinkFrequency(Duration),
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    #[cfg_attr(feature = "defmt", derive(defmt::Format))]
    pub enum Response {
        Ok,
    }
}

/// This might look like a duplication, but we intentionally keep those separate so they can
/// change independently.
pub mod stm32h7 {
    use arbitrary_int::u11;
    use core::time::Duration;

    pub const PUS_APID: u11 = u11::new(0x03);

    #[derive(Copy, Clone, Debug, serde::Serialize, serde::Deserialize)]
    #[cfg_attr(feature = "defmt", derive(defmt::Format))]
    pub enum Request {
        Ping,
        ChangeBlinkFrequency(Duration),
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    #[cfg_attr(feature = "defmt", derive(defmt::Format))]
    pub enum Response {
        Ok,
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct TmHeader {
    pub tc_packet_id: Option<CcsdsPacketIdAndPsc>,
    pub uptime_millis: u64,
}

pub fn tm_size<Response: serde::Serialize>(tm_header: &TmHeader, response: &Response) -> usize {
    ccsds_packet_len_for_user_data_len_with_checksum(
        postcard::experimental::serialized_size(tm_header).unwrap()
            + postcard::experimental::serialized_size(response).unwrap(),
    )
    .unwrap()
}

pub fn create_tm_packet<Response: serde::Serialize>(
    buf: &mut [u8],
    sp_header: SpacePacketHeader,
    tm_header: TmHeader,
    response: Response,
) -> Result<usize, CcsdsPacketCreationError> {
    let packet_data_size = postcard::experimental::serialized_size(&tm_header).unwrap()
        + postcard::experimental::serialized_size(&response).unwrap();
    let mut creator =
        CcsdsPacketCreatorWithReservedData::new_tm_with_checksum(sp_header, packet_data_size, buf)?;

    let current_index = postcard::to_slice(&tm_header, creator.packet_data_mut())
        .unwrap()
        .len();
    postcard::to_slice(&response, &mut creator.packet_data_mut()[current_index..]).unwrap();
    Ok(creator.finish())
}

#[cfg(test)]
mod tests {}
