//! Parameter types and enums.
//!
//! This module contains various helper types.
//!
//! # Primtive Parameter Wrappers and Enumeration
//!
//! This module includes wrapper for primitive rust types using the newtype pattern.
//! This was also done for pairs and triplets of these primitive types.
//! The [WritableToBeBytes] was implemented for all those types as well, which allows to easily
//! convert them into a network friendly raw byte format. The [ParamsRaw] enumeration groups
//! all newtypes and implements the [WritableToBeBytes] trait itself.
//!
//! ## Example for primitive type wrapper
//!
//! ```
//! use satrs::params::{ParamsRaw, ToBeBytes, U32Pair, WritableToBeBytes};
//!
//! let u32_pair = U32Pair(0x1010, 25);
//! assert_eq!(u32_pair.0, 0x1010);
//! assert_eq!(u32_pair.1, 25);
//! // Convert to raw stream
//! let raw_buf = u32_pair.to_be_bytes();
//! assert_eq!(raw_buf, [0, 0, 0x10, 0x10, 0, 0, 0, 25]);
//!
//! // Convert to enum variant
//! let params_raw: ParamsRaw = u32_pair.into();
//! assert_eq!(params_raw, (0x1010_u32, 25_u32).into());
//!
//! // Convert to stream using the enum variant
//! let mut other_raw_buf: [u8; 8] = [0; 8];
//! params_raw.write_to_be_bytes(&mut other_raw_buf).expect("Writing parameter to buffer failed");
//! assert_eq!(other_raw_buf, [0, 0, 0x10, 0x10, 0, 0, 0, 25]);
//!
//! // Create a pair from a raw stream
//! let u32_pair_from_stream: U32Pair = raw_buf.as_slice().try_into().unwrap();
//! assert_eq!(u32_pair_from_stream.0, 0x1010);
//! assert_eq!(u32_pair_from_stream.1, 25);
//! ```
//!
//! # Generic Parameter Enumeration
//!
//! The module also contains generic parameter enumerations.
//! This includes the [ParamsHeapless] enumeration for contained values which do not require heap
//! allocation, and the [Params] which enumerates [ParamsHeapless] and some additional types which
//! require [alloc] support but allow for more flexbility.
use crate::pool::PoolAddr;
use core::fmt::Debug;
use core::mem::size_of;
use paste::paste;
use spacepackets::ByteConversionError;
use spacepackets::ecss::{EcssEnumU8, EcssEnumU16, EcssEnumU32, EcssEnumU64};
pub use spacepackets::util::ToBeBytes;
use spacepackets::util::UnsignedEnum;

#[cfg(feature = "alloc")]
use alloc::string::{String, ToString};
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// Generic trait which is used for objects which can be converted into a raw network (big) endian
/// byte format.
pub trait WritableToBeBytes {
    fn written_len(&self) -> usize;
    /// Writes the object to a raw buffer in network endianness (big)
    fn write_to_be_bytes(&self, buf: &mut [u8]) -> Result<usize, ByteConversionError>;

    #[cfg(feature = "alloc")]
    fn to_vec(&self) -> Result<Vec<u8>, ByteConversionError> {
        let mut vec = alloc::vec![0; self.written_len()];
        self.write_to_be_bytes(&mut vec)?;
        Ok(vec)
    }
}

macro_rules! param_to_be_bytes_impl {
    ($Newtype: ident) => {
        impl WritableToBeBytes for $Newtype {
            #[inline]
            fn written_len(&self) -> usize {
                size_of::<<Self as ToBeBytes>::ByteArray>()
            }

            fn write_to_be_bytes(&self, buf: &mut [u8]) -> Result<usize, ByteConversionError> {
                let raw_len = WritableToBeBytes::written_len(self);
                if buf.len() < raw_len {
                    return Err(ByteConversionError::ToSliceTooSmall {
                        found: buf.len(),
                        expected: raw_len,
                    });
                }
                buf[0..raw_len].copy_from_slice(&self.to_be_bytes());
                Ok(raw_len)
            }
        }
    };
}

macro_rules! primitive_newtypes_with_eq {
    ($($ty: ty,)+) => {
        $(
            paste! {
                #[derive(Debug, Copy, Clone, PartialEq, Eq)]
                pub struct [<$ty:upper>](pub $ty);
                #[derive(Debug, Copy, Clone, PartialEq, Eq)]
                pub struct [<$ty:upper Pair>](pub $ty, pub $ty);
                #[derive(Debug, Copy, Clone, PartialEq, Eq)]
                pub struct [<$ty:upper Triplet>](pub $ty, pub $ty, pub $ty);

                param_to_be_bytes_impl!([<$ty:upper>]);
                param_to_be_bytes_impl!([<$ty:upper Pair>]);
                param_to_be_bytes_impl!([<$ty:upper Triplet>]);

                impl From<$ty> for [<$ty:upper>] {
                    fn from(v: $ty) -> Self {
                        Self(v)
                    }
                }
                impl From<($ty, $ty)> for [<$ty:upper Pair>] {
                    fn from(v: ($ty, $ty)) -> Self {
                        Self(v.0, v.1)
                    }
                }
                impl From<($ty, $ty, $ty)> for [<$ty:upper Triplet>] {
                    fn from(v: ($ty, $ty, $ty)) -> Self {
                        Self(v.0, v.1, v.2)
                    }
                }
            }
        )+
    }
}

macro_rules! primitive_newtypes {
    ($($ty: ty,)+) => {
        $(
            paste! {
                #[derive(Debug, Copy, Clone, PartialEq)]
                pub struct [<$ty:upper>](pub $ty);
                #[derive(Debug, Copy, Clone, PartialEq)]
                pub struct [<$ty:upper Pair>](pub $ty, pub $ty);
                #[derive(Debug, Copy, Clone, PartialEq)]
                pub struct [<$ty:upper Triplet>](pub $ty, pub $ty, pub $ty);

                param_to_be_bytes_impl!([<$ty:upper>]);
                param_to_be_bytes_impl!([<$ty:upper Pair>]);
                param_to_be_bytes_impl!([<$ty:upper Triplet>]);

                impl From<$ty> for [<$ty:upper>] {
                    fn from(v: $ty) -> Self {
                        Self(v)
                    }
                }
                impl From<($ty, $ty)> for [<$ty:upper Pair>] {
                    fn from(v: ($ty, $ty)) -> Self {
                        Self(v.0, v.1)
                    }
                }
                impl From<($ty, $ty, $ty)> for [<$ty:upper Triplet>] {
                    fn from(v: ($ty, $ty, $ty)) -> Self {
                        Self(v.0, v.1, v.2)
                    }
                }
            }
        )+
    }
}

primitive_newtypes_with_eq!(u8, u16, u32, u64, i8, i16, i32, i64,);
primitive_newtypes!(f32, f64,);

macro_rules! scalar_byte_conversions_impl {
    ($($ty: ty,)+) => {
        $(
            paste! {
                impl ToBeBytes for [<$ty:upper>] {
                    type ByteArray = [u8; size_of::<$ty>()];

                    fn written_len(&self) -> usize {
                        size_of::<Self::ByteArray>()
                    }

                    fn to_be_bytes(&self) -> Self::ByteArray {
                        self.0.to_be_bytes()
                    }
                }

                impl TryFrom<&[u8]> for [<$ty:upper>] {
                    type Error = ByteConversionError;

                    fn try_from(v: &[u8]) -> Result<Self, Self::Error>  {
                        if v.len() < size_of::<$ty>() {
                            return Err(ByteConversionError::FromSliceTooSmall{
                                expected: size_of::<$ty>(),
                                found: v.len()
                            });
                        }
                        Ok([<$ty:upper>]($ty::from_be_bytes(v[0..size_of::<$ty>()].try_into().unwrap())))
                    }
                }
            }
        )+
    }
}

macro_rules! pair_byte_conversions_impl {
    ($($ty: ty,)+) => {
        $(
            paste! {
                impl ToBeBytes for [<$ty:upper Pair>] {
                    type ByteArray = [u8; size_of::<$ty>() * 2];

                    fn written_len(&self) -> usize {
                        size_of::<Self::ByteArray>()
                    }

                    fn to_be_bytes(&self) -> Self::ByteArray {
                        let mut array = [0; size_of::<$ty>() * 2];
                        array[0..size_of::<$ty>()].copy_from_slice(&self.0.to_be_bytes());
                        array[
                            size_of::<$ty>()..2 * size_of::<$ty>()
                        ].copy_from_slice(&self.1.to_be_bytes());
                        array
                    }
                }

                impl TryFrom<&[u8]> for [<$ty:upper Pair>] {
                    type Error = ByteConversionError;

                    fn try_from(v: &[u8]) -> Result<Self, Self::Error>  {
                        if v.len() < 2 * size_of::<$ty>() {
                            return Err(ByteConversionError::FromSliceTooSmall{
                                expected: 2 * size_of::<$ty>(),
                                found: v.len()
                            });
                        }
                        Ok([<$ty:upper Pair>](
                            $ty::from_be_bytes(v[0..size_of::<$ty>()].try_into().unwrap()),
                            $ty::from_be_bytes(v[size_of::<$ty>()..2 * size_of::<$ty>()].try_into().unwrap())
                        ))
                    }
                }
            }
        )+
    }
}

macro_rules! triplet_to_be_bytes_impl {
    ($($ty: ty,)+) => {
        $(
            paste! {
                impl ToBeBytes for [<$ty:upper Triplet>] {
                    type ByteArray = [u8; size_of::<$ty>() * 3];

                    fn written_len(&self) -> usize {
                        size_of::<Self::ByteArray>()
                    }

                    fn to_be_bytes(&self) -> Self::ByteArray {
                        let mut array = [0; size_of::<$ty>() * 3];
                        array[0..size_of::<$ty>()].copy_from_slice(&self.0.to_be_bytes());
                        array[
                            size_of::<$ty>()..2 * size_of::<$ty>()
                        ].copy_from_slice(&self.1.to_be_bytes());
                        array[
                            2 * size_of::<$ty>()..3 * size_of::<$ty>()
                        ].copy_from_slice(&self.2.to_be_bytes());
                        array
                    }
                }
                impl TryFrom<&[u8]> for [<$ty:upper Triplet>] {
                    type Error = ByteConversionError;

                    fn try_from(v: &[u8]) -> Result<Self, Self::Error>  {
                        if v.len() < 3 * size_of::<$ty>() {
                            return Err(ByteConversionError::FromSliceTooSmall{
                                expected: 3 * size_of::<$ty>(),
                                found: v.len()
                            });
                        }
                        Ok([<$ty:upper Triplet>](
                            $ty::from_be_bytes(v[0..size_of::<$ty>()].try_into().unwrap()),
                            $ty::from_be_bytes(v[size_of::<$ty>()..2 * size_of::<$ty>()].try_into().unwrap()),
                            $ty::from_be_bytes(v[2 * size_of::<$ty>()..3 * size_of::<$ty>()].try_into().unwrap())
                        ))
                    }
                }
            }
        )+
    }
}

scalar_byte_conversions_impl!(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64,);

impl ToBeBytes for U8Pair {
    type ByteArray = [u8; 2];

    fn written_len(&self) -> usize {
        size_of::<Self::ByteArray>()
    }

    fn to_be_bytes(&self) -> Self::ByteArray {
        let mut array = [0; 2];
        array[0] = self.0;
        array[1] = self.1;
        array
    }
}

impl ToBeBytes for I8Pair {
    type ByteArray = [u8; 2];

    fn written_len(&self) -> usize {
        size_of::<Self::ByteArray>()
    }

    fn to_be_bytes(&self) -> Self::ByteArray {
        let mut array = [0; 2];
        array[0] = self.0 as u8;
        array[1] = self.1 as u8;
        array
    }
}

impl ToBeBytes for U8Triplet {
    type ByteArray = [u8; 3];

    fn written_len(&self) -> usize {
        size_of::<Self::ByteArray>()
    }

    fn to_be_bytes(&self) -> Self::ByteArray {
        let mut array = [0; 3];
        array[0] = self.0;
        array[1] = self.1;
        array[2] = self.2;
        array
    }
}

impl ToBeBytes for I8Triplet {
    type ByteArray = [u8; 3];

    fn written_len(&self) -> usize {
        size_of::<Self::ByteArray>()
    }

    fn to_be_bytes(&self) -> Self::ByteArray {
        let mut array = [0; 3];
        array[0] = self.0 as u8;
        array[1] = self.1 as u8;
        array[2] = self.2 as u8;
        array
    }
}

pair_byte_conversions_impl!(u16, u32, u64, i16, i32, i64, f32, f64,);
triplet_to_be_bytes_impl!(u16, u32, u64, i16, i32, i64, f32, f64,);

/// Generic enumeration for additonal parameters only consisting of primitive data types.
///
/// All contained variants and the enum itself implement the [WritableToBeBytes] trait, which
/// allows to easily convert them into a network-friendly format.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ParamsRaw {
    U8(U8),
    U8Pair(U8Pair),
    U8Triplet(U8Triplet),
    I8(I8),
    I8Pair(I8Pair),
    I8Triplet(I8Triplet),
    U16(U16),
    U16Pair(U16Pair),
    U16Triplet(U16Triplet),
    I16(I16),
    I16Pair(I16Pair),
    I16Triplet(I16Triplet),
    U32(U32),
    U32Pair(U32Pair),
    U32Triplet(U32Triplet),
    I32(I32),
    I32Pair(I32Pair),
    I32Triplet(I32Triplet),
    F32(F32),
    F32Pair(F32Pair),
    F32Triplet(F32Triplet),
    U64(U64),
    I64(I64),
    F64(F64),
}

impl WritableToBeBytes for ParamsRaw {
    fn written_len(&self) -> usize {
        match self {
            ParamsRaw::U8(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::U8Pair(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::U8Triplet(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::I8(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::I8Pair(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::I8Triplet(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::U16(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::U16Pair(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::U16Triplet(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::I16(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::I16Pair(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::I16Triplet(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::U32(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::U32Pair(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::U32Triplet(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::I32(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::I32Pair(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::I32Triplet(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::F32(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::F32Pair(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::F32Triplet(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::U64(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::I64(v) => WritableToBeBytes::written_len(v),
            ParamsRaw::F64(v) => WritableToBeBytes::written_len(v),
        }
    }

    fn write_to_be_bytes(&self, buf: &mut [u8]) -> Result<usize, ByteConversionError> {
        match self {
            ParamsRaw::U8(v) => v.write_to_be_bytes(buf),
            ParamsRaw::U8Pair(v) => v.write_to_be_bytes(buf),
            ParamsRaw::U8Triplet(v) => v.write_to_be_bytes(buf),
            ParamsRaw::I8(v) => v.write_to_be_bytes(buf),
            ParamsRaw::I8Pair(v) => v.write_to_be_bytes(buf),
            ParamsRaw::I8Triplet(v) => v.write_to_be_bytes(buf),
            ParamsRaw::U16(v) => v.write_to_be_bytes(buf),
            ParamsRaw::U16Pair(v) => v.write_to_be_bytes(buf),
            ParamsRaw::U16Triplet(v) => v.write_to_be_bytes(buf),
            ParamsRaw::I16(v) => v.write_to_be_bytes(buf),
            ParamsRaw::I16Pair(v) => v.write_to_be_bytes(buf),
            ParamsRaw::I16Triplet(v) => v.write_to_be_bytes(buf),
            ParamsRaw::U32(v) => v.write_to_be_bytes(buf),
            ParamsRaw::U32Pair(v) => v.write_to_be_bytes(buf),
            ParamsRaw::U32Triplet(v) => v.write_to_be_bytes(buf),
            ParamsRaw::I32(v) => v.write_to_be_bytes(buf),
            ParamsRaw::I32Pair(v) => v.write_to_be_bytes(buf),
            ParamsRaw::I32Triplet(v) => v.write_to_be_bytes(buf),
            ParamsRaw::F32(v) => v.write_to_be_bytes(buf),
            ParamsRaw::F32Pair(v) => v.write_to_be_bytes(buf),
            ParamsRaw::F32Triplet(v) => v.write_to_be_bytes(buf),
            ParamsRaw::U64(v) => v.write_to_be_bytes(buf),
            ParamsRaw::I64(v) => v.write_to_be_bytes(buf),
            ParamsRaw::F64(v) => v.write_to_be_bytes(buf),
        }
    }
}

macro_rules! params_raw_from_newtype {
    ($($newtype: ident,)+) => {
        $(
            impl From<$newtype> for ParamsRaw {
                fn from(v: $newtype) -> Self {
                    Self::$newtype(v)
                }
            }
        )+
    }
}

params_raw_from_newtype!(
    U8, U8Pair, U8Triplet, U16, U16Pair, U16Triplet, U32, U32Pair, U32Triplet, I8, I8Pair,
    I8Triplet, I16, I16Pair, I16Triplet, I32, I32Pair, I32Triplet, F32, F32Pair, F32Triplet, U64,
    I64, F64,
);

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ParamsEcssEnum {
    U8(EcssEnumU8),
    U16(EcssEnumU16),
    U32(EcssEnumU32),
    U64(EcssEnumU64),
}

macro_rules! writable_as_be_bytes_ecss_enum_impl {
    ($EnumIdent: ident, $Ty: ident) => {
        impl From<$EnumIdent> for ParamsEcssEnum {
            fn from(e: $EnumIdent) -> Self {
                Self::$Ty(e)
            }
        }

        impl WritableToBeBytes for $EnumIdent {
            fn written_len(&self) -> usize {
                self.size()
            }

            fn write_to_be_bytes(&self, buf: &mut [u8]) -> Result<usize, ByteConversionError> {
                <Self as UnsignedEnum>::write_to_slice_be(self, buf).map(|_| self.written_len())
            }
        }
    };
}

writable_as_be_bytes_ecss_enum_impl!(EcssEnumU8, U8);
writable_as_be_bytes_ecss_enum_impl!(EcssEnumU16, U16);
writable_as_be_bytes_ecss_enum_impl!(EcssEnumU32, U32);
writable_as_be_bytes_ecss_enum_impl!(EcssEnumU64, U64);

impl WritableToBeBytes for ParamsEcssEnum {
    fn written_len(&self) -> usize {
        match self {
            ParamsEcssEnum::U8(e) => e.written_len(),
            ParamsEcssEnum::U16(e) => e.written_len(),
            ParamsEcssEnum::U32(e) => e.written_len(),
            ParamsEcssEnum::U64(e) => e.written_len(),
        }
    }

    fn write_to_be_bytes(&self, buf: &mut [u8]) -> Result<usize, ByteConversionError> {
        match self {
            ParamsEcssEnum::U8(e) => WritableToBeBytes::write_to_be_bytes(e, buf),
            ParamsEcssEnum::U16(e) => WritableToBeBytes::write_to_be_bytes(e, buf),
            ParamsEcssEnum::U32(e) => WritableToBeBytes::write_to_be_bytes(e, buf),
            ParamsEcssEnum::U64(e) => WritableToBeBytes::write_to_be_bytes(e, buf),
        }
    }
}

/// Generic enumeration for parameters which do not rely on heap allocations.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ParamsHeapless {
    Raw(ParamsRaw),
    EcssEnum(ParamsEcssEnum),
}

impl From<ParamsRaw> for ParamsHeapless {
    fn from(v: ParamsRaw) -> Self {
        Self::Raw(v)
    }
}

impl From<ParamsEcssEnum> for ParamsHeapless {
    fn from(v: ParamsEcssEnum) -> Self {
        Self::EcssEnum(v)
    }
}

macro_rules! from_conversions_for_raw {
    ($(($raw_ty: ty, $TargetPath: path),)+) => {
        $(
            impl From<$raw_ty> for ParamsRaw {
                fn from(val: $raw_ty) -> Self {
                    $TargetPath(val.into())
                }
            }

            impl From<$raw_ty> for ParamsHeapless {
                fn from(val: $raw_ty) -> Self {
                    ParamsHeapless::Raw(val.into())
                }
            }
        )+
    };
}

from_conversions_for_raw!(
    (u8, Self::U8),
    ((u8, u8), Self::U8Pair),
    ((u8, u8, u8), Self::U8Triplet),
    (i8, Self::I8),
    ((i8, i8), Self::I8Pair),
    ((i8, i8, i8), Self::I8Triplet),
    (u16, Self::U16),
    ((u16, u16), Self::U16Pair),
    ((u16, u16, u16), Self::U16Triplet),
    (i16, Self::I16),
    ((i16, i16), Self::I16Pair),
    ((i16, i16, i16), Self::I16Triplet),
    (u32, Self::U32),
    ((u32, u32), Self::U32Pair),
    ((u32, u32, u32), Self::U32Triplet),
    (i32, Self::I32),
    ((i32, i32), Self::I32Pair),
    ((i32, i32, i32), Self::I32Triplet),
    (f32, Self::F32),
    ((f32, f32), Self::F32Pair),
    ((f32, f32, f32), Self::F32Triplet),
    (u64, Self::U64),
    (f64, Self::F64),
);

/// Generic enumeration for additional parameters, including parameters which rely on heap
/// allocations.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Params {
    Heapless(ParamsHeapless),
    Store(PoolAddr),
    #[cfg(feature = "alloc")]
    Vec(Vec<u8>),
    #[cfg(feature = "alloc")]
    String(String),
}

impl From<PoolAddr> for Params {
    fn from(x: PoolAddr) -> Self {
        Self::Store(x)
    }
}

impl From<ParamsHeapless> for Params {
    fn from(x: ParamsHeapless) -> Self {
        Self::Heapless(x)
    }
}

impl From<ParamsRaw> for Params {
    fn from(x: ParamsRaw) -> Self {
        Self::Heapless(ParamsHeapless::Raw(x))
    }
}

#[cfg(feature = "alloc")]
impl From<Vec<u8>> for Params {
    fn from(val: Vec<u8>) -> Self {
        Self::Vec(val)
    }
}

/// Converts a byte slice into the [Params::Vec] variant
#[cfg(feature = "alloc")]
impl From<&[u8]> for Params {
    fn from(val: &[u8]) -> Self {
        Self::Vec(val.to_vec())
    }
}

#[cfg(feature = "alloc")]
impl From<String> for Params {
    fn from(val: String) -> Self {
        Self::String(val)
    }
}

#[cfg(feature = "alloc")]
/// Converts a string slice into the [Params::String] variant
impl From<&str> for Params {
    fn from(val: &str) -> Self {
        Self::String(val.to_string())
    }
}

impl WritableToBeBytes for ParamsHeapless {
    fn written_len(&self) -> usize {
        match self {
            ParamsHeapless::Raw(raw) => raw.written_len(),
            ParamsHeapless::EcssEnum(ecss_enum) => ecss_enum.written_len(),
        }
    }

    fn write_to_be_bytes(&self, buf: &mut [u8]) -> Result<usize, ByteConversionError> {
        match self {
            ParamsHeapless::Raw(raw) => raw.write_to_be_bytes(buf),
            ParamsHeapless::EcssEnum(ecss_enum) => ecss_enum.write_to_be_bytes(buf),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cloning_works(param_raw: &impl WritableToBeBytes) {
        let _new_param = param_raw;
    }

    fn test_writing_fails(param_raw: &(impl WritableToBeBytes + ToBeBytes)) {
        let pair_size = WritableToBeBytes::written_len(param_raw);
        assert_eq!(pair_size, ToBeBytes::written_len(param_raw));
        let mut vec = alloc::vec![0; pair_size - 1];
        let result = param_raw.write_to_be_bytes(&mut vec);
        if let Err(ByteConversionError::ToSliceTooSmall { found, expected }) = result {
            assert_eq!(found, pair_size - 1);
            assert_eq!(expected, pair_size);
        } else {
            panic!("Expected ByteConversionError::ToSliceTooSmall");
        }
    }

    fn test_writing(params_raw: &ParamsRaw, writeable: &impl WritableToBeBytes) {
        assert_eq!(params_raw.written_len(), writeable.written_len());
        let mut vec = alloc::vec![0; writeable.written_len()];
        writeable
            .write_to_be_bytes(&mut vec)
            .expect("writing parameter to buffer failed");
        let mut other_vec = alloc::vec![0; writeable.written_len()];
        params_raw
            .write_to_be_bytes(&mut other_vec)
            .expect("writing parameter to buffer failed");
        assert_eq!(vec, other_vec);
    }

    #[test]
    fn test_basic_u32_pair() {
        let u32_pair = U32Pair(4, 8);
        assert_eq!(u32_pair.0, 4);
        assert_eq!(u32_pair.1, 8);
        let raw = u32_pair.to_be_bytes();
        let mut u32_conv_back = u32::from_be_bytes(raw[0..4].try_into().unwrap());
        assert_eq!(u32_conv_back, 4);
        u32_conv_back = u32::from_be_bytes(raw[4..8].try_into().unwrap());
        assert_eq!(u32_conv_back, 8);
        test_writing_fails(&u32_pair);
        test_cloning_works(&u32_pair);
        let u32_praw = ParamsRaw::from(u32_pair);
        test_writing(&u32_praw, &u32_pair);
    }

    #[test]
    fn test_u16_pair_writing_fails() {
        let u16_pair = U16Pair(4, 8);
        test_writing_fails(&u16_pair);
        test_cloning_works(&u16_pair);
        let u16_praw = ParamsRaw::from(u16_pair);
        test_writing(&u16_praw, &u16_pair);
    }

    #[test]
    fn test_u8_pair_writing_fails() {
        let u8_pair = U8Pair(4, 8);
        test_writing_fails(&u8_pair);
        test_cloning_works(&u8_pair);
        let u8_praw = ParamsRaw::from(u8_pair);
        test_writing(&u8_praw, &u8_pair);
    }

    #[test]
    fn basic_i8_test() {
        let i8_pair = I8Pair(-3, -16);
        assert_eq!(i8_pair.0, -3);
        assert_eq!(i8_pair.1, -16);
        let raw = i8_pair.to_be_bytes();
        let mut i8_conv_back = i8::from_be_bytes(raw[0..1].try_into().unwrap());
        assert_eq!(i8_conv_back, -3);
        i8_conv_back = i8::from_be_bytes(raw[1..2].try_into().unwrap());
        assert_eq!(i8_conv_back, -16);
        test_writing_fails(&i8_pair);
        test_cloning_works(&i8_pair);
        let i8_praw = ParamsRaw::from(i8_pair);
        test_writing(&i8_praw, &i8_pair);
    }

    #[test]
    fn test_from_u32_triplet() {
        let raw_params = U32Triplet::from((1, 2, 3));
        assert_eq!(raw_params.0, 1);
        assert_eq!(raw_params.1, 2);
        assert_eq!(raw_params.2, 3);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 12);
        assert_eq!(
            raw_params.to_be_bytes(),
            [0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3]
        );
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let u32_triplet = ParamsRaw::from(raw_params);
        test_writing(&u32_triplet, &raw_params);
    }

    #[test]
    fn test_i8_triplet() {
        let i8_triplet = I8Triplet(-3, -16, -126);
        assert_eq!(i8_triplet.0, -3);
        assert_eq!(i8_triplet.1, -16);
        assert_eq!(i8_triplet.2, -126);
        let raw = i8_triplet.to_be_bytes();
        let mut i8_conv_back = i8::from_be_bytes(raw[0..1].try_into().unwrap());
        assert_eq!(i8_conv_back, -3);
        i8_conv_back = i8::from_be_bytes(raw[1..2].try_into().unwrap());
        assert_eq!(i8_conv_back, -16);
        i8_conv_back = i8::from_be_bytes(raw[2..3].try_into().unwrap());
        assert_eq!(i8_conv_back, -126);
        test_writing_fails(&i8_triplet);
        test_cloning_works(&i8_triplet);
        let i8_praw = ParamsRaw::from(i8_triplet);
        test_writing(&i8_praw, &i8_triplet);
    }

    #[test]
    fn conversion_test_string() {
        let param: Params = "Test String".into();
        if let Params::String(str) = param {
            assert_eq!(str, String::from("Test String"));
        } else {
            panic!("Params type is not String")
        }
    }

    #[test]
    fn conversion_from_slice() {
        let test_slice: [u8; 5] = [0; 5];
        let vec_param: Params = test_slice.as_slice().into();
        if let Params::Vec(vec) = vec_param {
            assert_eq!(vec, test_slice.to_vec());
        } else {
            panic!("Params type is not a vector")
        }
    }

    #[test]
    fn test_params_written_len_raw() {
        let param_raw = ParamsRaw::from((500_u32, 1000_u32));
        assert_eq!(param_raw.written_len(), 8);
        let mut buf: [u8; 8] = [0; 8];
        param_raw
            .write_to_be_bytes(&mut buf)
            .expect("writing to buffer failed");
        assert_eq!(u32::from_be_bytes(buf[0..4].try_into().unwrap()), 500);
        assert_eq!(u32::from_be_bytes(buf[4..8].try_into().unwrap()), 1000);
    }

    #[test]
    fn test_heapless_param_writable_trait_raw() {
        let param_heapless = ParamsHeapless::Raw(ParamsRaw::from((500_u32, 1000_u32)));
        assert_eq!(param_heapless.written_len(), 8);
        let mut buf: [u8; 8] = [0; 8];
        let size = param_heapless
            .write_to_be_bytes(&mut buf)
            .expect("writing failed");
        assert_eq!(size, 8);
        assert_eq!(u32::from_be_bytes(buf[0..4].try_into().unwrap()), 500);
        assert_eq!(u32::from_be_bytes(buf[4..8].try_into().unwrap()), 1000);
    }

    #[test]
    fn test_heapless_param_writable_trait_ecss_enum() {
        let param_heapless = ParamsHeapless::EcssEnum(ParamsEcssEnum::U16(5.into()));
        assert_eq!(param_heapless.written_len(), 2);
        let mut buf: [u8; 2] = [0; 2];
        let size = param_heapless
            .write_to_be_bytes(&mut buf)
            .expect("writing failed");
        assert_eq!(size, 2);
        assert_eq!(u16::from_be_bytes(buf[0..2].try_into().unwrap()), 5);
    }

    #[test]
    fn test_u32_single() {
        let raw_params = U32::from(20);
        assert_eq!(raw_params.0, 20);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 4);
        assert_eq!(raw_params.to_be_bytes(), [0, 0, 0, 20]);
        let other = U32::from(20);
        assert_eq!(raw_params, other);
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let u32_praw = ParamsRaw::from(raw_params);
        test_writing(&u32_praw, &raw_params);
    }

    #[test]
    fn test_i8_single() {
        let neg_number: i8 = -5_i8;
        let raw_params = I8::from(neg_number);
        assert_eq!(raw_params.0, neg_number);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 1);
        assert_eq!(raw_params.to_be_bytes(), neg_number.to_be_bytes());
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let u8_praw = ParamsRaw::from(raw_params);
        test_writing(&u8_praw, &raw_params);
    }

    #[test]
    fn test_u8_single() {
        let raw_params = U8::from(20);
        assert_eq!(raw_params.0, 20);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 1);
        assert_eq!(raw_params.to_be_bytes(), [20]);
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let u32_praw = ParamsRaw::from(raw_params);
        test_writing(&u32_praw, &raw_params);
    }

    #[test]
    fn test_u16_single() {
        let raw_params = U16::from(0x123);
        assert_eq!(raw_params.0, 0x123);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 2);
        assert_eq!(raw_params.to_be_bytes(), [0x01, 0x23]);
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let u16_praw = ParamsRaw::from(raw_params);
        test_writing(&u16_praw, &raw_params);
    }

    #[test]
    fn test_u16_triplet() {
        let raw_params = U16Triplet::from((1, 2, 3));
        assert_eq!(raw_params.0, 1);
        assert_eq!(raw_params.1, 2);
        assert_eq!(raw_params.2, 3);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 6);
        assert_eq!(raw_params.to_be_bytes(), [0, 1, 0, 2, 0, 3]);
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let u16_praw = ParamsRaw::from(raw_params);
        test_writing(&u16_praw, &raw_params);
    }

    #[test]
    fn test_u8_triplet() {
        let raw_params = U8Triplet::from((1, 2, 3));
        assert_eq!(raw_params.0, 1);
        assert_eq!(raw_params.1, 2);
        assert_eq!(raw_params.2, 3);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 3);
        assert_eq!(raw_params.to_be_bytes(), [1, 2, 3]);
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let u8_praw = ParamsRaw::from(raw_params);
        test_writing(&u8_praw, &raw_params);
    }

    #[test]
    fn test_i16_single() {
        let value = -300_i16;
        let raw_params = I16::from(value);
        assert_eq!(raw_params.0, value);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 2);
        assert_eq!(raw_params.to_be_bytes(), value.to_be_bytes());
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let i16_praw = ParamsRaw::from(raw_params);
        test_writing(&i16_praw, &raw_params);
    }

    #[test]
    fn test_i16_pair() {
        let raw_params = I16Pair::from((-300, -400));
        assert_eq!(raw_params.0, -300);
        assert_eq!(raw_params.1, -400);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 4);
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let i16_praw = ParamsRaw::from(raw_params);
        test_writing(&i16_praw, &raw_params);
    }

    #[test]
    fn test_i16_triplet() {
        let raw_params = I16Triplet::from((-300, -400, -350));
        assert_eq!(raw_params.0, -300);
        assert_eq!(raw_params.1, -400);
        assert_eq!(raw_params.2, -350);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 6);
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let i16_praw = ParamsRaw::from(raw_params);
        test_writing(&i16_praw, &raw_params);
    }

    #[test]
    fn test_i32_single() {
        let raw_params = I32::from(-80000);
        assert_eq!(raw_params.0, -80000);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 4);
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let i32_praw = ParamsRaw::from(raw_params);
        test_writing(&i32_praw, &raw_params);
    }

    #[test]
    fn test_i32_pair() {
        let raw_params = I32Pair::from((-80000, -200));
        assert_eq!(raw_params.0, -80000);
        assert_eq!(raw_params.1, -200);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 8);
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let i32_praw = ParamsRaw::from(raw_params);
        test_writing(&i32_praw, &raw_params);
    }

    #[test]
    fn test_i32_triplet() {
        let raw_params = I32Triplet::from((-80000, -5, -200));
        assert_eq!(raw_params.0, -80000);
        assert_eq!(raw_params.1, -5);
        assert_eq!(raw_params.2, -200);
        assert_eq!(WritableToBeBytes::written_len(&raw_params), 12);
        test_writing_fails(&raw_params);
        test_cloning_works(&raw_params);
        let i32_praw = ParamsRaw::from(raw_params);
        test_writing(&i32_praw, &raw_params);
    }

    #[test]
    fn test_f32_single() {
        let param = F32::from(0.1);
        assert_eq!(param.0, 0.1);
        assert_eq!(WritableToBeBytes::written_len(&param), 4);
        let f32_pair_raw = param.to_be_bytes();
        let f32_0 = f32::from_be_bytes(f32_pair_raw[0..4].try_into().unwrap());
        assert_eq!(f32_0, 0.1);
        test_writing_fails(&param);
        test_cloning_works(&param);
        let praw = ParamsRaw::from(param);
        test_writing(&praw, &param);
        let p_try_from = F32::try_from(param.to_be_bytes().as_ref()).expect("try_from failed");
        assert_eq!(p_try_from, param);
    }

    #[test]
    fn test_f32_pair() {
        let param = F32Pair::from((0.1, 0.2));
        assert_eq!(param.0, 0.1);
        assert_eq!(param.1, 0.2);
        assert_eq!(WritableToBeBytes::written_len(&param), 8);
        let f32_pair_raw = param.to_be_bytes();
        let f32_0 = f32::from_be_bytes(f32_pair_raw[0..4].try_into().unwrap());
        assert_eq!(f32_0, 0.1);
        let f32_1 = f32::from_be_bytes(f32_pair_raw[4..8].try_into().unwrap());
        assert_eq!(f32_1, 0.2);
        let other_pair = F32Pair::from((0.1, 0.2));
        assert_eq!(param, other_pair);
        test_writing_fails(&param);
        test_cloning_works(&param);
        let praw = ParamsRaw::from(param);
        test_writing(&praw, &param);
        let p_try_from = F32Pair::try_from(param.to_be_bytes().as_ref()).expect("try_from failed");
        assert_eq!(p_try_from, param);
    }

    #[test]
    fn test_f32_triplet() {
        let f32 = F32Triplet::from((0.1, -0.1, -5.2));
        assert_eq!(f32.0, 0.1);
        assert_eq!(f32.1, -0.1);
        assert_eq!(f32.2, -5.2);
        assert_eq!(WritableToBeBytes::written_len(&f32), 12);
        let f32_pair_raw = f32.to_be_bytes();
        let f32_0 = f32::from_be_bytes(f32_pair_raw[0..4].try_into().unwrap());
        assert_eq!(f32_0, 0.1);
        let f32_1 = f32::from_be_bytes(f32_pair_raw[4..8].try_into().unwrap());
        assert_eq!(f32_1, -0.1);
        let f32_2 = f32::from_be_bytes(f32_pair_raw[8..12].try_into().unwrap());
        assert_eq!(f32_2, -5.2);
        test_writing_fails(&f32);
        test_cloning_works(&f32);
        let f32_praw = ParamsRaw::from(f32);
        test_writing(&f32_praw, &f32);
        let f32_try_from =
            F32Triplet::try_from(f32.to_be_bytes().as_ref()).expect("try_from failed");
        assert_eq!(f32_try_from, f32);
    }

    #[test]
    fn test_u64_single() {
        let u64 = U64::from(0x1010101010);
        assert_eq!(u64.0, 0x1010101010);
        assert_eq!(WritableToBeBytes::written_len(&u64), 8);
        test_writing_fails(&u64);
        test_cloning_works(&u64);
        let praw = ParamsRaw::from(u64);
        test_writing(&praw, &u64);
    }

    #[test]
    fn test_i64_single() {
        let i64 = I64::from(-0xfffffffff);
        assert_eq!(i64.0, -0xfffffffff);
        assert_eq!(WritableToBeBytes::written_len(&i64), 8);
        test_writing_fails(&i64);
        test_cloning_works(&i64);
        let praw = ParamsRaw::from(i64);
        test_writing(&praw, &i64);
    }

    #[test]
    fn test_f64_single() {
        let value = 823_823_812_832.232_3;
        let f64 = F64::from(value);
        assert_eq!(f64.0, value);
        assert_eq!(WritableToBeBytes::written_len(&f64), 8);
        test_writing_fails(&f64);
        test_cloning_works(&f64);
        let praw = ParamsRaw::from(f64);
        test_writing(&praw, &f64);
    }

    #[test]
    fn test_f64_triplet() {
        let f64_triplet = F64Triplet::from((0.1, 0.2, 0.3));
        assert_eq!(f64_triplet.0, 0.1);
        assert_eq!(f64_triplet.1, 0.2);
        assert_eq!(f64_triplet.2, 0.3);
        assert_eq!(WritableToBeBytes::written_len(&f64_triplet), 24);
        let f64_triplet_raw = f64_triplet.to_be_bytes();
        let f64_0 = f64::from_be_bytes(f64_triplet_raw[0..8].try_into().unwrap());
        assert_eq!(f64_0, 0.1);
        let f64_1 = f64::from_be_bytes(f64_triplet_raw[8..16].try_into().unwrap());
        assert_eq!(f64_1, 0.2);
        let f64_2 = f64::from_be_bytes(f64_triplet_raw[16..24].try_into().unwrap());
        assert_eq!(f64_2, 0.3);
        test_writing_fails(&f64_triplet);
        test_cloning_works(&f64_triplet);
    }

    #[test]
    fn test_u8_ecss_enum() {
        let value = 200;
        let u8p = EcssEnumU8::new(value);
        test_cloning_works(&u8p);
        let praw = ParamsEcssEnum::from(u8p);
        assert_eq!(praw.written_len(), 1);
        let mut buf = [0; 1];
        praw.write_to_be_bytes(&mut buf)
            .expect("writing to buffer failed");
    }

    #[test]
    fn test_u16_ecss_enum() {
        let value = 60000;
        let u16p = EcssEnumU16::new(value);
        test_cloning_works(&u16p);
        let praw = ParamsEcssEnum::from(u16p);
        assert_eq!(praw.written_len(), 2);
        let mut buf = [0; 2];
        praw.write_to_be_bytes(&mut buf)
            .expect("writing to buffer failed");
        assert_eq!(u16::from_be_bytes(buf), value);
    }

    #[test]
    fn test_u32_ecss_enum() {
        let value = 70000;
        let u32p = EcssEnumU32::new(value);
        test_cloning_works(&u32p);
        let praw = ParamsEcssEnum::from(u32p);
        assert_eq!(praw.written_len(), 4);
        let mut buf = [0; 4];
        praw.write_to_be_bytes(&mut buf)
            .expect("writing to buffer failed");
        assert_eq!(u32::from_be_bytes(buf), value);
    }

    #[test]
    fn test_u64_ecss_enum() {
        let value = 0xffffffffff;
        let u64p = EcssEnumU64::new(value);
        test_cloning_works(&u64p);
        let praw = ParamsEcssEnum::from(u64p);
        assert_eq!(praw.written_len(), 8);
        let mut buf = [0; 8];
        praw.write_to_be_bytes(&mut buf)
            .expect("writing to buffer failed");
        assert_eq!(u64::from_be_bytes(buf), value);
    }
}
