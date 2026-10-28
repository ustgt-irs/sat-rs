#![allow(dead_code)]
use core::mem::size_of;
use serde::{Deserialize, Serialize};
use spacepackets::ecss::{PfcReal, PfcUnsigned, Ptc};
use spacepackets::time::CcsdsTimeProvider;
use spacepackets::time::cds::CdsTime;

enum NumOfParamsInfo {
    /// The parameter entry is a scalar field
    Scalar = 0b00,
    /// The parameter entry is a vector, and its length field is one byte wide (max. 255 entries)
    VecLenFieldOneByte = 0b01,
    /// The parameter entry is a vecotr, and its length field is two bytes wide (max. 65565 entries)
    VecLenFieldTwoBytes = 0b10,
    /// The parameter entry is a matrix, and its length field contains a one byte row number
    /// and a one byte column number.
    MatrixRowsAndColumns = 0b11,
}

const HAS_VALIDITY_MASK: u8 = 1 << 7;

struct ParamWithValidity<T> {
    valid: bool,
    val: T,
}

struct TestMgmHk {
    temp: f32,
    mgm_vals: [u16; 3],
}

struct TestMgmHkWithIndividualValidity {
    temp: ParamWithValidity<f32>,
    mgm_vals: ParamWithValidity<[u16; 3]>,
}

#[derive(Serialize, Deserialize)]
struct TestMgmHkWithGroupValidity {
    last_valid_stamp: CdsTime,
    valid: bool,
    temp: f32,
    mgm_vals: [u16; 3],
}

impl TestMgmHk {
    pub fn write_to_be_bytes(&self, buf: &mut [u8]) -> Result<usize, ()> {
        let mut curr_idx = 0;
        buf[curr_idx..curr_idx + size_of::<f32>()].copy_from_slice(&self.temp.to_be_bytes());
        curr_idx += size_of::<f32>();
        for val in self.mgm_vals {
            buf[curr_idx..curr_idx + size_of::<u16>()].copy_from_slice(&val.to_be_bytes());
            curr_idx += size_of::<u16>();
        }
        Ok(curr_idx)
    }
}

/// This could in principle be auto-generated.
impl TestMgmHkWithIndividualValidity {
    pub fn write_to_be_bytes_self_describing(&self, buf: &mut [u8]) -> Result<usize, ()> {
        let mut curr_idx = 0;
        buf[curr_idx] = 0;
        buf[curr_idx] |= HAS_VALIDITY_MASK | (self.temp.valid as u8) << 6;
        curr_idx += 1;
        buf[curr_idx] = Ptc::Real as u8;
        curr_idx += 1;
        buf[curr_idx] = PfcReal::Float as u8;
        curr_idx += 1;
        buf[curr_idx..curr_idx + size_of::<f32>()].copy_from_slice(&self.temp.val.to_be_bytes());
        curr_idx += size_of::<f32>();
        buf[curr_idx] = 0;
        buf[curr_idx] |= HAS_VALIDITY_MASK
            | (self.mgm_vals.valid as u8) << 6
            | (NumOfParamsInfo::VecLenFieldOneByte as u8) << 4;
        curr_idx += 1;
        buf[curr_idx] = Ptc::UnsignedInt as u8;
        curr_idx += 1;
        buf[curr_idx] = PfcUnsigned::TwoBytes as u8;
        curr_idx += 1;
        buf[curr_idx] = 3;
        curr_idx += 1;
        for val in self.mgm_vals.val {
            buf[curr_idx..curr_idx + size_of::<u16>()].copy_from_slice(&val.to_be_bytes());
            curr_idx += size_of::<u16>();
        }
        Ok(curr_idx)
    }
}

impl TestMgmHkWithGroupValidity {
    pub fn write_to_be_bytes_self_describing(&self, buf: &mut [u8]) -> Result<usize, ()> {
        let mut curr_idx = 0;
        buf[curr_idx] = self.valid as u8;
        curr_idx += 1;
        self.last_valid_stamp
            .write_to_slice(&mut buf[curr_idx..curr_idx + self.last_valid_stamp.len_as_bytes()])
            .unwrap();
        curr_idx += self.last_valid_stamp.len_as_bytes();
        buf[curr_idx] = 0;
        curr_idx += 1;
        buf[curr_idx] = Ptc::Real as u8;
        curr_idx += 1;
        buf[curr_idx] = PfcReal::Float as u8;
        curr_idx += 1;
        buf[curr_idx..curr_idx + size_of::<f32>()].copy_from_slice(&self.temp.to_be_bytes());
        curr_idx += size_of::<f32>();
        buf[curr_idx] = 0;
        buf[curr_idx] |= (NumOfParamsInfo::VecLenFieldOneByte as u8) << 4;
        curr_idx += 1;
        buf[curr_idx] = Ptc::UnsignedInt as u8;
        curr_idx += 1;
        buf[curr_idx] = PfcUnsigned::TwoBytes as u8;
        curr_idx += 1;
        buf[curr_idx] = 3;
        for val in self.mgm_vals {
            buf[curr_idx..curr_idx + size_of::<u16>()].copy_from_slice(&val.to_be_bytes());
            curr_idx += size_of::<u16>();
        }
        Ok(curr_idx)
    }
}

#[test]
pub fn main() {
    let mut raw_buf: [u8; 32] = [0; 32];
    let mgm_hk = TestMgmHk {
        temp: 20.0,
        mgm_vals: [0x1f1f, 0x2f2f, 0x3f3f],
    };
    // 4 byte float + 3 * 2 bytes MGM values
    let written = mgm_hk.write_to_be_bytes(&mut raw_buf).unwrap();
    assert_eq!(written, 10);

    let mgm_hk_individual_validity = TestMgmHkWithIndividualValidity {
        temp: ParamWithValidity {
            valid: true,
            val: 20.0,
        },
        mgm_vals: ParamWithValidity {
            valid: true,
            val: [0x1f1f, 0x2f2f, 0x3f3f],
        },
    };
    let written = mgm_hk_individual_validity
        .write_to_be_bytes_self_describing(&mut raw_buf)
        .unwrap();
    // 3 byte float description, 4 byte float, 4 byte MGM val description, 3 * 2 bytes MGM values
    assert_eq!(written, 17);

    // The easiest and probably best approach, trading off big advantages for TM downlink capacity:
    // Use a JSON format
    let mgm_hk_group_validity = TestMgmHkWithGroupValidity {
        last_valid_stamp: CdsTime::now_with_u16_days().unwrap(),
        valid: false,
        temp: 20.0,
        mgm_vals: [0x1f1f, 0x2f2f, 0x3f3f],
    };
    let mgm_as_json_str = serde_json::to_string(&mgm_hk_group_validity).unwrap();
    println!(
        "JSON string with length {}: {}",
        mgm_as_json_str.len(),
        mgm_as_json_str
    );
}
