//! Verifies AttributeValue's `omit_bounds` fix round-trips correctly with
//! genuinely recursive data (a Set containing an Object containing a
//! nested Set), not just that it compiles.

use arbor_types::{ArchivedAttributeValue, AttributeValue, Attributes};
use rkyv::rancor::Error;

fn main() {
    let mut inner_attrs = Attributes::new();
    inner_attrs.set(
        arbor_types::AttributeNameId::new(1),
        AttributeValue::Set(vec![
            AttributeValue::Integer(1),
            AttributeValue::Integer(2),
        ]),
    );

    let value = AttributeValue::Set(vec![
        AttributeValue::String("hello".to_string()),
        AttributeValue::Object(inner_attrs),
        AttributeValue::Set(vec![AttributeValue::Bool(true)]),
    ]);

    let bytes = rkyv::to_bytes::<Error>(&value).expect("serialize");
    let archived = rkyv::access::<ArchivedAttributeValue, Error>(&bytes).expect("access/validate");

    let ArchivedAttributeValue::Set(items) = archived else { panic!("expected Set") };
    assert_eq!(items.len(), 3);

    let deserialized: AttributeValue = rkyv::deserialize::<AttributeValue, Error>(archived).expect("deserialize");
    assert_eq!(deserialized, value);

    println!("AttributeValue recursive round-trip OK: {value:?}");
}
