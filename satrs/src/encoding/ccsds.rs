use spacepackets::SpHeader;

use crate::{ComponentId, tmtc::PacketHandler};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SpValidity {
    Valid,
    /// The space packet can be assumed to have a valid format, but the packet should
    /// be skipped.
    Skip,
    /// The space packet or space packet header has an invalid format, for example a CRC check
    /// failed. In that case, the parser loses the packet synchronization and needs to check for
    /// the start of a new space packet header start again. The space packet header
    /// [spacepackets::PacketId] can be used as a synchronization marker to detect the start
    /// of a possible valid packet again.
    Invalid,
}

/// Simple trait to allow user code to check the validity of a space packet.
pub trait SpacePacketValidator {
    fn validate(&self, sp_header: &SpHeader, raw_buf: &[u8]) -> SpValidity;
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct ParseResult {
    pub packets_found: u32,
    pub parsed_bytes: usize,
    // If an incomplete space packet was found, its start index is indicated by this value.
    //pub incomplete_packet_start: Option<usize>,
}

/// This function parses a given buffer for tightly packed CCSDS space packets.
///
/// Please note that it is recommended to use a proper data link layer instead to have proper
/// packet framing and to allow more reliable recovery from packet loss.
/// It uses the [spacepackets::SpHeader] of the CCSDS packets and a user provided
/// [SpacePacketValidator] to check whether a received space packet is relevant for processing.
///
/// This function is also able to deal with broken tail packets at the end as long a the parser
/// can read the full 7 bytes which constitue a space packet header plus one byte minimal size.
/// If broken tail packets are detected, they are moved to the front of the buffer, and the write
/// index for future write operations will be written to the `next_write_idx` argument.
///
/// The parses will behave differently based on the [SpValidity] returned from the user provided
/// [SpacePacketValidator]:
///
///  1. [SpValidity::Valid]: The parser will forward all packets to the given `packet_sender` and
///     return the number of packets found.If the [PacketHandler::handle_packet] calls fails, the
///     error will be returned.
///  2. [SpValidity::Invalid]: The parser assumes that the synchronization is lost and tries to
///     find the start of a new space packet header by scanning all the following bytes.
///  3. [SpValidity::Skip]: The parser skips the packet using the packet length determined from the
///     space packet header.
///
pub fn parse_buffer_for_ccsds_space_packets<SendError>(
    buf: &[u8],
    packet_validator: &(impl SpacePacketValidator + ?Sized),
    sender_id: ComponentId,
    packet_sender: &(impl PacketHandler<Error = SendError> + ?Sized),
) -> Result<ParseResult, SendError> {
    let mut parse_result = ParseResult::default();
    let mut current_idx = 0;
    let buf_len = buf.len();
    loop {
        if current_idx + 7 > buf.len() {
            break;
        }
        let sp_header = SpHeader::from_be_bytes(&buf[current_idx..]).unwrap().0;
        match packet_validator.validate(&sp_header, &buf[current_idx..]) {
            SpValidity::Valid => {
                let packet_size = sp_header.packet_len();
                if (current_idx + packet_size) <= buf_len {
                    packet_sender
                        .handle_packet(sender_id, &buf[current_idx..current_idx + packet_size])?;
                    parse_result.packets_found += 1;
                } else {
                    // Move packet to start of buffer if applicable.
                    //parse_result.incomplete_packet_start = Some(current_idx);
                    break;
                }
                current_idx += packet_size;
                continue;
            }
            SpValidity::Skip => {
                current_idx += sp_header.packet_len();
            }
            // We might have lost sync. Try to find the start of a new space packet header.
            SpValidity::Invalid => {
                current_idx += 1;
            }
        }
    }
    parse_result.parsed_bytes = current_idx;
    Ok(parse_result)
}

#[cfg(test)]
mod tests {
    use arbitrary_int::{u11, u14};
    use spacepackets::{
        CcsdsPacket, PacketId, PacketSequenceControl, PacketType, SequenceFlags, SpHeader,
        ecss::{CreatorConfig, MessageTypeId, tc::PusTcCreator},
    };

    use crate::{ComponentId, encoding::tests::TcCacher};

    use super::{SpValidity, SpacePacketValidator, parse_buffer_for_ccsds_space_packets};

    const PARSER_ID: ComponentId = 0x05;
    const TEST_APID_0: u11 = u11::new(0x02);
    const TEST_APID_1: u11 = u11::new(0x10);
    const TEST_PACKET_ID_0: PacketId = PacketId::new_for_tc(true, TEST_APID_0);
    const TEST_PACKET_ID_1: PacketId = PacketId::new_for_tc(true, TEST_APID_1);

    #[derive(Default)]
    struct SimpleVerificator {
        pub enable_second_id: bool,
    }

    impl SimpleVerificator {
        pub fn new_with_second_id() -> Self {
            Self {
                enable_second_id: true,
            }
        }
    }

    impl SpacePacketValidator for SimpleVerificator {
        fn validate(&self, sp_header: &SpHeader, _raw_buf: &[u8]) -> super::SpValidity {
            if sp_header.packet_id() == TEST_PACKET_ID_0
                || (self.enable_second_id && sp_header.packet_id() == TEST_PACKET_ID_1)
            {
                return SpValidity::Valid;
            }
            SpValidity::Skip
        }
    }

    #[test]
    fn test_basic() {
        let sph = SpHeader::new_from_apid(TEST_APID_0);
        let ping_tc = PusTcCreator::new_simple(
            sph,
            MessageTypeId::new(17, 1),
            &[],
            CreatorConfig::default(),
        );
        let mut buffer: [u8; 32] = [0; 32];
        let packet_len = ping_tc
            .write_to_slice(&mut buffer)
            .expect("writing packet failed");
        let tc_cacher = TcCacher::default();
        let parse_result = parse_buffer_for_ccsds_space_packets(
            &buffer,
            &SimpleVerificator::default(),
            PARSER_ID,
            &tc_cacher,
        );
        assert!(parse_result.is_ok());
        let parse_result = parse_result.unwrap();
        assert_eq!(parse_result.packets_found, 1);
        let mut queue = tc_cacher.tc_queue.borrow_mut();
        assert_eq!(queue.len(), 1);
        let packet_with_sender = queue.pop_front().unwrap();
        assert_eq!(packet_with_sender.packet, buffer[..packet_len]);
        assert_eq!(packet_with_sender.sender_id, PARSER_ID);
    }

    #[test]
    fn test_multi_packet() {
        let sph = SpHeader::new_from_apid(TEST_APID_0);
        let ping_tc = PusTcCreator::new_simple(
            sph,
            MessageTypeId::new(17, 1),
            &[],
            CreatorConfig::default(),
        );
        let action_tc =
            PusTcCreator::new_simple(sph, MessageTypeId::new(8, 0), &[], CreatorConfig::default());
        let mut buffer: [u8; 32] = [0; 32];
        let packet_len_ping = ping_tc
            .write_to_slice(&mut buffer)
            .expect("writing packet failed");
        let packet_len_action = action_tc
            .write_to_slice(&mut buffer[packet_len_ping..])
            .expect("writing packet failed");
        let tc_cacher = TcCacher::default();
        let parse_result = parse_buffer_for_ccsds_space_packets(
            &buffer,
            &SimpleVerificator::default(),
            PARSER_ID,
            &tc_cacher,
        );
        assert!(parse_result.is_ok());
        let parse_result = parse_result.unwrap();
        assert_eq!(parse_result.packets_found, 2);
        let mut queue = tc_cacher.tc_queue.borrow_mut();
        assert_eq!(queue.len(), 2);
        let packet_with_addr = queue.pop_front().unwrap();
        assert_eq!(packet_with_addr.packet, buffer[..packet_len_ping]);
        assert_eq!(packet_with_addr.sender_id, PARSER_ID);
        let packet_with_addr = queue.pop_front().unwrap();
        assert_eq!(packet_with_addr.sender_id, PARSER_ID);
        assert_eq!(
            packet_with_addr.packet,
            buffer[packet_len_ping..packet_len_ping + packet_len_action]
        );
    }

    #[test]
    fn test_multi_apid() {
        let sph = SpHeader::new_from_apid(TEST_APID_0);
        let ping_tc = PusTcCreator::new_simple(
            sph,
            MessageTypeId::new(17, 1),
            &[],
            CreatorConfig::default(),
        );
        let sph = SpHeader::new_from_apid(TEST_APID_1);
        let action_tc =
            PusTcCreator::new_simple(sph, MessageTypeId::new(8, 0), &[], CreatorConfig::default());
        let mut buffer: [u8; 32] = [0; 32];
        let packet_len_ping = ping_tc
            .write_to_slice(&mut buffer)
            .expect("writing packet failed");
        let packet_len_action = action_tc
            .write_to_slice(&mut buffer[packet_len_ping..])
            .expect("writing packet failed");
        let tc_cacher = TcCacher::default();
        let verificator = SimpleVerificator::new_with_second_id();
        let parse_result =
            parse_buffer_for_ccsds_space_packets(&buffer, &verificator, PARSER_ID, &tc_cacher);
        assert!(parse_result.is_ok());
        let parse_result = parse_result.unwrap();
        assert_eq!(parse_result.packets_found, 2);
        let mut queue = tc_cacher.tc_queue.borrow_mut();
        assert_eq!(queue.len(), 2);
        let packet_with_addr = queue.pop_front().unwrap();
        assert_eq!(packet_with_addr.packet, buffer[..packet_len_ping]);
        let packet_with_addr = queue.pop_front().unwrap();
        assert_eq!(
            packet_with_addr.packet,
            buffer[packet_len_ping..packet_len_ping + packet_len_action]
        );
    }

    #[test]
    fn test_split_packet_multi() {
        let ping_tc = PusTcCreator::new_simple(
            SpHeader::new_from_apid(TEST_APID_0),
            MessageTypeId::new(17, 1),
            &[],
            CreatorConfig::default(),
        );
        let action_tc = PusTcCreator::new_simple(
            SpHeader::new_from_apid(TEST_APID_1),
            MessageTypeId::new(8, 0),
            &[],
            CreatorConfig::default(),
        );
        let mut buffer: [u8; 32] = [0; 32];
        let packet_len_ping = ping_tc
            .write_to_slice(&mut buffer)
            .expect("writing packet failed");
        let packet_len_action = action_tc
            .write_to_slice(&mut buffer[packet_len_ping..])
            .expect("writing packet failed");
        let tc_cacher = TcCacher::default();
        let verificator = SimpleVerificator::new_with_second_id();
        let parse_result = parse_buffer_for_ccsds_space_packets(
            &buffer[..packet_len_ping + packet_len_action - 4],
            &verificator,
            PARSER_ID,
            &tc_cacher,
        );
        assert!(parse_result.is_ok());
        let parse_result = parse_result.unwrap();
        assert_eq!(parse_result.packets_found, 1);
        let incomplete_tail_idx = parse_result.parsed_bytes;
        assert_eq!(incomplete_tail_idx, packet_len_ping);

        let queue = tc_cacher.tc_queue.borrow();
        assert_eq!(queue.len(), 1);
        // The broken packet was moved to the start, so the next write index should be after the
        // last segment missing 4 bytes.
    }

    #[test]
    fn test_one_split_packet() {
        let ping_tc = PusTcCreator::new_simple(
            SpHeader::new_from_apid(TEST_APID_0),
            MessageTypeId::new(17, 1),
            &[],
            CreatorConfig::default(),
        );
        let mut buffer: [u8; 32] = [0; 32];
        let packet_len_ping = ping_tc
            .write_to_slice(&mut buffer)
            .expect("writing packet failed");
        let tc_cacher = TcCacher::default();

        let verificator = SimpleVerificator::new_with_second_id();
        let parse_result = parse_buffer_for_ccsds_space_packets(
            &buffer[..packet_len_ping - 4],
            &verificator,
            PARSER_ID,
            &tc_cacher,
        );
        assert!(parse_result.is_ok());
        let parse_result = parse_result.unwrap();
        assert_eq!(parse_result.packets_found, 0);
        let queue = tc_cacher.tc_queue.borrow();
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn test_smallest_packet() {
        let ccsds_header_only = SpHeader::new(
            PacketId::new(PacketType::Tc, true, TEST_APID_0),
            PacketSequenceControl::new(SequenceFlags::Unsegmented, u14::new(0)),
            0,
        );
        let mut buf: [u8; 7] = [0; 7];
        ccsds_header_only
            .write_to_slice(&mut buf)
            .expect("writing failed");
        let verificator = SimpleVerificator::default();
        let tc_cacher = TcCacher::default();
        let parse_result =
            parse_buffer_for_ccsds_space_packets(&buf, &verificator, PARSER_ID, &tc_cacher);
        assert!(parse_result.is_ok());
        let parse_result = parse_result.unwrap();
        assert_eq!(parse_result.packets_found, 1);
    }
}
