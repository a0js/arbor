//! `rkyv` `with`-wrapper types for the four field types in this crate's
//! reachable data model that have no native `rkyv` support compatible with
//! our `rkyv` version.
//!
//! `Uuid`, `RapidHashMap`, `std::net::IpAddr`, and `BTreeMap` all have
//! native support and need nothing here. `chrono`'s and `ordered-float`'s
//! own "rkyv" features exist but target `rkyv` 0.7, not our 0.8 -- using
//! them fails to compile with a "multiple different versions of crate
//! `rkyv`" error, confirmed directly. `RoaringBitmap` and `IpNet` have no
//! rkyv support in any version. All four need a hand-written `ArchiveWith`/
//! `SerializeWith`/`DeserializeWith` impl on a local marker type -- the
//! only legal path around the orphan rule, since neither `Archive` (rkyv's
//! trait) nor these types (foreign crates) are local to this crate.
//!
//! Each was round-trip verified in isolation under `benches/src/bin/` before
//! being moved here: `rkyv_wrapper_types_check.rs` (Uuid, DateTime, IpNet),
//! `rkyv_roaring_wrapper_check.rs` (RoaringBitmap), `rkyv_orderedfloat_check.rs`
//! (OrderedFloat).

use chrono::{DateTime, Utc};
use ipnet::IpNet;
use ordered_float::OrderedFloat;
use roaring::RoaringBitmap;
use std::net::IpAddr;

use rkyv::rancor::Fallible;
use rkyv::ser::{Allocator, Writer};
use rkyv::vec::{ArchivedVec, VecResolver};
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Archive, Archived, Deserialize, Place, Resolver, Serialize};

// ---------------------------------------------------------------------------
// TimestampMillis: DateTime<Utc> <-> i64 (millis since epoch)
// ---------------------------------------------------------------------------

pub struct TimestampMillis;

impl ArchiveWith<DateTime<Utc>> for TimestampMillis {
    type Archived = Archived<i64>;
    type Resolver = Resolver<i64>;

    fn resolve_with(field: &DateTime<Utc>, resolver: Self::Resolver, out: Place<Self::Archived>) {
        field.timestamp_millis().resolve(resolver, out);
    }
}

impl<S> SerializeWith<DateTime<Utc>, S> for TimestampMillis
where
    S: Fallible + ?Sized,
    i64: Serialize<S>,
{
    fn serialize_with(field: &DateTime<Utc>, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        field.timestamp_millis().serialize(serializer)
    }
}

impl<D> DeserializeWith<Archived<i64>, DateTime<Utc>, D> for TimestampMillis
where
    D: Fallible + ?Sized,
    Archived<i64>: Deserialize<i64, D>,
{
    fn deserialize_with(field: &Archived<i64>, deserializer: &mut D) -> Result<DateTime<Utc>, D::Error> {
        let millis: i64 = field.deserialize(deserializer)?;
        // Trusted internal format (self-produced snapshot files): a millis
        // value that round-tripped from a valid DateTime<Utc> is always in
        // range.
        Ok(DateTime::from_timestamp_millis(millis).expect("millis out of DateTime<Utc> range"))
    }
}

// ---------------------------------------------------------------------------
// IpNetAsBits: IpNet <-> { v6: bool, addr: [u8; 16], prefix_len: u8 }
// v4 addresses are stored zero-extended in the low 4 bytes of `addr`.
// ---------------------------------------------------------------------------

#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct IpNetBits {
    pub(crate) v6: bool,
    pub(crate) addr: [u8; 16],
    pub(crate) prefix_len: u8,
}

fn ipnet_to_bits(net: &IpNet) -> IpNetBits {
    match net.addr() {
        IpAddr::V4(v4) => {
            let mut addr = [0u8; 16];
            addr[12..16].copy_from_slice(&v4.octets());
            IpNetBits { v6: false, addr, prefix_len: net.prefix_len() }
        }
        IpAddr::V6(v6) => IpNetBits { v6: true, addr: v6.octets(), prefix_len: net.prefix_len() },
    }
}

fn bits_to_ipnet(bits: &IpNetBits) -> IpNet {
    ipnet_from_parts(bits.v6, bits.addr, bits.prefix_len)
}

/// Shared by both the owned `IpNetBits` (above) and `ArchivedIpNetBits`
/// (used directly by `ArchivedIndexedAttributeValue::as_view` for zero-copy
/// reads) -- `v6`/`addr`/`prefix_len` are all trivially-portable types
/// (bool, [u8; 16], u8), identical on both sides, no endian conversion.
pub(crate) fn ipnet_from_parts(v6: bool, addr: [u8; 16], prefix_len: u8) -> IpNet {
    let ip: IpAddr = if v6 {
        IpAddr::from(addr)
    } else {
        IpAddr::from([addr[12], addr[13], addr[14], addr[15]])
    };
    IpNet::new(ip, prefix_len).expect("valid prefix_len round-tripped from a valid IpNet")
}

pub struct IpNetAsBits;

impl ArchiveWith<IpNet> for IpNetAsBits {
    type Archived = Archived<IpNetBits>;
    type Resolver = Resolver<IpNetBits>;

    fn resolve_with(field: &IpNet, resolver: Self::Resolver, out: Place<Self::Archived>) {
        ipnet_to_bits(field).resolve(resolver, out);
    }
}

impl<S> SerializeWith<IpNet, S> for IpNetAsBits
where
    S: Fallible + ?Sized,
    IpNetBits: Serialize<S>,
{
    fn serialize_with(field: &IpNet, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        ipnet_to_bits(field).serialize(serializer)
    }
}

impl<D> DeserializeWith<Archived<IpNetBits>, IpNet, D> for IpNetAsBits
where
    D: Fallible + ?Sized,
    Archived<IpNetBits>: Deserialize<IpNetBits, D>,
{
    fn deserialize_with(field: &Archived<IpNetBits>, deserializer: &mut D) -> Result<IpNet, D::Error> {
        let bits: IpNetBits = field.deserialize(deserializer)?;
        Ok(bits_to_ipnet(&bits))
    }
}

// ---------------------------------------------------------------------------
// RoaringAsBytes: RoaringBitmap <-> opaque bytes (roaring's own
// serialize_into format -- the same bytes bincode already stores today).
//
// NOT zero-copy: the archived field is just raw bytes, no bitmap semantics
// available without an explicit deserialize step. Per "no lazy loading",
// every RoaringBitmap-backed field is meant to be eagerly decoded once at
// load time via this wrapper's deserialize_with, same allocation cost as
// today's bincode path -- rkyv only removes the doubling/churn overhead
// for the arenas/strings/nodes, not for RoaringBitmap fields.
// ---------------------------------------------------------------------------

pub struct RoaringAsBytes;

impl ArchiveWith<RoaringBitmap> for RoaringAsBytes {
    type Archived = ArchivedVec<u8>;
    type Resolver = VecResolver;

    fn resolve_with(field: &RoaringBitmap, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let mut buf = Vec::new();
        field.serialize_into(&mut buf).expect("roaring serialize");
        ArchivedVec::resolve_from_slice(&buf, resolver, out);
    }
}

impl<S> SerializeWith<RoaringBitmap, S> for RoaringAsBytes
where
    S: Fallible + Allocator + Writer + ?Sized,
{
    fn serialize_with(field: &RoaringBitmap, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let mut buf = Vec::new();
        field.serialize_into(&mut buf).expect("roaring serialize");
        ArchivedVec::serialize_from_slice(&buf, serializer)
    }
}

impl<D> DeserializeWith<ArchivedVec<u8>, RoaringBitmap, D> for RoaringAsBytes
where
    D: Fallible + ?Sized,
{
    fn deserialize_with(field: &ArchivedVec<u8>, _deserializer: &mut D) -> Result<RoaringBitmap, D::Error> {
        Ok(RoaringBitmap::deserialize_from(field.as_slice()).expect("roaring deserialize"))
    }
}

// ---------------------------------------------------------------------------
// OrderedFloatAsF64: OrderedFloat<f64> <-> f64 (identity pass-through)
// ---------------------------------------------------------------------------

pub struct OrderedFloatAsF64;

impl ArchiveWith<OrderedFloat<f64>> for OrderedFloatAsF64 {
    type Archived = Archived<f64>;
    type Resolver = Resolver<f64>;

    fn resolve_with(field: &OrderedFloat<f64>, resolver: Self::Resolver, out: Place<Self::Archived>) {
        field.into_inner().resolve(resolver, out);
    }
}

impl<S> SerializeWith<OrderedFloat<f64>, S> for OrderedFloatAsF64
where
    S: Fallible + ?Sized,
    f64: Serialize<S>,
{
    fn serialize_with(field: &OrderedFloat<f64>, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        field.into_inner().serialize(serializer)
    }
}

impl<D> DeserializeWith<Archived<f64>, OrderedFloat<f64>, D> for OrderedFloatAsF64
where
    D: Fallible + ?Sized,
    Archived<f64>: Deserialize<f64, D>,
{
    fn deserialize_with(field: &Archived<f64>, deserializer: &mut D) -> Result<OrderedFloat<f64>, D::Error> {
        let f: f64 = field.deserialize(deserializer)?;
        Ok(OrderedFloat(f))
    }
}
