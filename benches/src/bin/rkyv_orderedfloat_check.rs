//! Verifies the `OrderedFloat<f64>` with-wrapper. `ordered-float`'s own
//! "rkyv" feature targets rkyv 0.7 (confirmed via a separate scratch
//! build -- `OrderedFloat<f64>: Archive` fails under our rkyv 0.8 with a
//! "multiple different versions of crate `rkyv`" error), so this needs the
//! same hand-written with-module treatment as DateTime<Utc>/IpNet. Simplest
//! of the four: OrderedFloat<f64> is a bare newtype around f64, so this is
//! an identity pass-through, no transformation logic.

use ordered_float::OrderedFloat;
use rkyv::rancor::Error;
use rkyv::with::{ArchiveWith, DeserializeWith, SerializeWith};
use rkyv::{Place, Resolver, Serialize, Archive as ArchiveTrait, Deserialize as DeserializeTrait, Archived};

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
    S: rkyv::rancor::Fallible + ?Sized,
    f64: Serialize<S>,
{
    fn serialize_with(field: &OrderedFloat<f64>, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        field.into_inner().serialize(serializer)
    }
}

impl<D> DeserializeWith<Archived<f64>, OrderedFloat<f64>, D> for OrderedFloatAsF64
where
    D: rkyv::rancor::Fallible + ?Sized,
    Archived<f64>: DeserializeTrait<f64, D>,
{
    fn deserialize_with(field: &Archived<f64>, deserializer: &mut D) -> Result<OrderedFloat<f64>, D::Error> {
        let f: f64 = field.deserialize(deserializer)?;
        Ok(OrderedFloat(f))
    }
}

#[derive(ArchiveTrait, Serialize, DeserializeTrait, Debug, PartialEq)]
struct Example {
    #[rkyv(with = OrderedFloatAsF64)]
    score: OrderedFloat<f64>,
}

fn main() {
    let value = Example { score: OrderedFloat(3.14159) };

    let bytes = rkyv::to_bytes::<Error>(&value).expect("serialize");
    let archived = rkyv::access::<ArchivedExample, Error>(&bytes).expect("access/validate");
    assert_eq!(f64::from(archived.score), 3.14159);

    let deserialized: Example = rkyv::deserialize::<Example, Error>(archived).expect("deserialize");
    assert_eq!(deserialized, value);

    println!("OrderedFloat<f64> wrapper round-trip OK: {value:?}");
}
