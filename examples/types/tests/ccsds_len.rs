use arbitrary_int::u11;
use spacepackets::{SpacePacketHeader, time::cds::CdsTime};
use types::{
    Apid, ComponentId, MessageType, TcHeader, TmHeader,
    ccsds::{CcsdsTcPacketOwned, CcsdsTmPacketOwned},
};

#[test]
fn tc_len_written_matches_actual_serialized_length() {
    let packet = CcsdsTcPacketOwned::new_with_request(
        SpacePacketHeader::new_from_apid(u11::new(Apid::Tmtc as u16)),
        TcHeader::new(ComponentId::Controller, MessageType::Ping),
        types::control::request::Request::Ping,
    );

    let actual = packet.to_vec();

    println!("TC len_written(): {}", packet.len_written());
    println!("TC actual len:    {}", actual.len());

    assert_eq!(packet.len_written(), actual.len());
}

#[test]
fn tm_len_written_matches_actual_serialized_length() {
    let timestamp = CdsTime::new_with_u16_days(0, 0);

    let tm_header = TmHeader::new(
        ComponentId::Controller,
        ComponentId::Ground,
        MessageType::Verification,
        None,
        &timestamp,
    );

    let packet = CcsdsTmPacketOwned::new_with_serde_payload(
        SpacePacketHeader::new_from_apid(u11::new(Apid::Tmtc as u16)),
        &tm_header,
        &types::control::response::Response::Ok,
    )
    .unwrap();

    let actual = packet.to_vec();

    println!("TM len_written(): {}", packet.len_written());
    println!("TM actual len:    {}", actual.len());

    assert_eq!(packet.len_written(), actual.len());
}
