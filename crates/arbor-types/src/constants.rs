use roaring::RoaringBitmap;
use std::sync::LazyLock;

pub static EMPTY_BITMAP: LazyLock<RoaringBitmap> = LazyLock::new(RoaringBitmap::new);