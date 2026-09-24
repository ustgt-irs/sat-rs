use arbitrary_int::u11;
use satrs::spacepackets::{
    CcsdsPacketIdAndPsc, SpHeader,
    time::{StdTimestampError, cds::CdsTime},
};
use serde::Serialize;
use types::{Apid, ComponentId, Message, TmHeader, ccsds::CcsdsTmPacketOwned};

#[derive(Debug, thiserror::Error)]
pub enum CcsdsTmCreationError {
    #[error("postcard error: {0}")]
    Postcard(#[from] postcard::Error),
    #[error("timestamp error: {0}")]
    Time(#[from] StdTimestampError),
}

pub fn pack_ccsds_tm_packet_for_now(
    sender_id: ComponentId,
    tc_id: Option<CcsdsPacketIdAndPsc>,
    payload: &(impl Serialize + Message),
) -> Result<CcsdsTmPacketOwned, CcsdsTmCreationError> {
    let now = CdsTime::now_with_u16_days()?;
    let sp_header = SpHeader::new_from_apid(u11::new(Apid::Tmtc as u16));
    let tm_header = TmHeader::new(
        sender_id,
        ComponentId::Ground,
        payload.message_type(),
        tc_id,
        &now,
    );
    Ok(CcsdsTmPacketOwned::new_with_serde_payload(
        sp_header, &tm_header, payload,
    )?)
}
