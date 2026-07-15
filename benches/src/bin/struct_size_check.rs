//! Checks whether `IndexedNode`'s enum layout is dominated by its largest
//! variant (`IndexedPolicy`, which embeds several `RoaringBitmap`s inline),
//! forcing every `Entity` variant -- millions of them -- to pay for space
//! it doesn't use. If so, `Policy(Box<IndexedPolicy>)` would shrink the
//! whole `Vec<IndexedNode>` at near-zero cost (one extra pointer indirection
//! only when actually reading a policy, which is rare relative to entities).

use arbor_types::{IndexedAttributeValue, IndexedEntity, IndexedNode, IndexedPolicy};

fn main() {
    println!("size_of::<IndexedEntity>()          = {}", std::mem::size_of::<IndexedEntity>());
    println!("size_of::<IndexedPolicy>()          = {}", std::mem::size_of::<IndexedPolicy>());
    println!("size_of::<IndexedNode>()            = {}", std::mem::size_of::<IndexedNode>());
    println!("size_of::<IndexedAttributeValue>()  = {}", std::mem::size_of::<IndexedAttributeValue>());
    println!("size_of::<(u32, IndexedAttributeValue)>() = {}", std::mem::size_of::<(u32, IndexedAttributeValue)>());
}
