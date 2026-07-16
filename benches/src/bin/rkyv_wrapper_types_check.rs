//! Verifies the `rkyv` `with`-wrapper types needed for the port: `Uuid` is
//! free via rkyv's native `uuid-1` feature (checked directly, no wrapper
//! code); `DateTime<Utc>` and `IpNet` have no native rkyv support (checked
//! in conversation: neither `rkyv` nor `chrono`/`jiff`/`ipnet` provide a
//! compatible impl), so both need hand-written `ArchiveWith`/
//! `SerializeWith`/`DeserializeWith` impls on a local marker type -- the
//! only legal path around the orphan rule (`Archive` is rkyv's trait,
//! `DateTime<Utc>`/`IpNet` are foreign types; neither is local to this
//! crate, so a direct `impl Archive for DateTime<Utc>` would be
//! impl-foreign-trait-for-foreign-type, rejected by the compiler).
//!
//! Round-trips a struct containing all three types through `to_bytes` +
//! `access` and asserts equality, before this pattern gets used for real in
//! the Snapshot port.

use chrono::{DateTime, Utc};
use ipnet::IpNet;
use rkyv::rancor::Error;
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Place, Resolver, Serialize, Archive as ArchiveTrait, Deserialize as DeserializeTrait, Archived};
use std::net::IpAddr;
use uuid::Uuid;

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
    S: rkyv::rancor::Fallible + ?Sized,
    i64: Serialize<S>,
{
    fn serialize_with(field: &DateTime<Utc>, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        field.timestamp_millis().serialize(serializer)
    }
}

impl<D> DeserializeWith<Archived<i64>, DateTime<Utc>, D> for TimestampMillis
where
    D: rkyv::rancor::Fallible + ?Sized,
    Archived<i64>: DeserializeTrait<i64, D>,
{
    fn deserialize_with(field: &Archived<i64>, deserializer: &mut D) -> Result<DateTime<Utc>, D::Error> {
        let millis: i64 = field.deserialize(deserializer)?;
        // Trusted internal format (self-produced snapshot files), same
        // tradeoff already made for reserved_vec_serde/rapid_hash_map_serde:
        // a millis value that round-tripped from a valid DateTime<Utc> is
        // always in range.
        Ok(DateTime::from_timestamp_millis(millis).expect("millis out of DateTime<Utc> range"))
    }
}

// ---------------------------------------------------------------------------
// IpNetBits: IpNet <-> { v6: bool, addr: [u8; 16], prefix_len: u8 }
// v4 addresses are stored zero-extended in the low 4 bytes of `addr`.
// ---------------------------------------------------------------------------

#[derive(ArchiveTrait, Serialize, DeserializeTrait)]
pub struct IpNetBits {
    v6: bool,
    addr: [u8; 16],
    prefix_len: u8,
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
    let ip: IpAddr = if bits.v6 {
        IpAddr::from(bits.addr)
    } else {
        IpAddr::from([bits.addr[12], bits.addr[13], bits.addr[14], bits.addr[15]])
    };
    IpNet::new(ip, bits.prefix_len).expect("valid prefix_len round-tripped from a valid IpNet")
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
    S: rkyv::rancor::Fallible + ?Sized,
    IpNetBits: Serialize<S>,
{
    fn serialize_with(field: &IpNet, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        ipnet_to_bits(field).serialize(serializer)
    }
}

impl<D> DeserializeWith<Archived<IpNetBits>, IpNet, D> for IpNetAsBits
where
    D: rkyv::rancor::Fallible + ?Sized,
    Archived<IpNetBits>: DeserializeTrait<IpNetBits, D>,
{
    fn deserialize_with(field: &Archived<IpNetBits>, deserializer: &mut D) -> Result<IpNet, D::Error> {
        let bits: IpNetBits = field.deserialize(deserializer)?;
        Ok(bits_to_ipnet(&bits))
    }
}

// ---------------------------------------------------------------------------
// Round-trip check
// ---------------------------------------------------------------------------

#[derive(ArchiveTrait, Serialize, DeserializeTrait, Debug, PartialEq)]
struct Example {
    id: Uuid, // free via rkyv's native uuid-1 feature, no wrapper needed
    #[rkyv(with = TimestampMillis)]
    created_at: DateTime<Utc>,
    #[rkyv(with = IpNetAsBits)]
    network_v4: IpNet,
    #[rkyv(with = IpNetAsBits)]
    network_v6: IpNet,
}

fn main() {
    let value = Example {
        id: Uuid::new_v4(),
        created_at: DateTime::from_timestamp_millis(1_752_500_000_123).unwrap(),
        network_v4: "10.0.0.0/8".parse().unwrap(),
        network_v6: "fe80::/10".parse().unwrap(),
    };

    let bytes = rkyv::to_bytes::<Error>(&value).expect("serialize");
    let archived = rkyv::access::<ArchivedExample, Error>(&bytes).expect("access/validate");

    // Uuid is zero-copy: archived.id is usable directly, no conversion.
    assert_eq!(archived.id, value.id);

    let deserialized: Example =
        rkyv::deserialize::<Example, Error>(archived).expect("deserialize");
    assert_eq!(deserialized, value);

    println!("all wrapper round-trips OK: {value:?}");
}
