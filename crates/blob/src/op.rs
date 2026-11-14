//! The S3 request kinds the system issues, and how each is priced.
//!
//! Pricing follows S3 Standard: PUT, COPY, POST and **LIST** are billed in the
//! same class, about 12.5x a GET. That is why a LIST is never a cheap way to
//! discover state: a key derived from data already held costs nothing, a GET
//! costs one GET-class request, and a LIST costs as much as a PUT.

/// One kind of S3 request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum S3Op {
    /// A whole-object GET.
    Get,
    /// A ranged GET (`Range: bytes=a-b`), one request however many bytes.
    GetRange,
    /// A HEAD, billed as a GET.
    Head,
    /// A PUT, including a conditional create (`If-None-Match: *`).
    Put,
    /// A server-side COPY, billed as a PUT.
    Copy,
    /// A LIST (`ListObjectsV2`), billed as a PUT and capped at 1,000 keys.
    List,
    /// A DELETE, which S3 does not bill.
    Delete,
}

/// The billing class a request falls in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PriceClass {
    /// PUT, COPY, POST and LIST.
    PutClass,
    /// GET, ranged GET and HEAD.
    GetClass,
    /// Not billed per request.
    Free,
}

impl S3Op {
    /// Every request kind, in declaration order.
    pub const ALL: [S3Op; 7] = [
        S3Op::Get,
        S3Op::GetRange,
        S3Op::Head,
        S3Op::Put,
        S3Op::Copy,
        S3Op::List,
        S3Op::Delete,
    ];

    /// The billing class of this request kind.
    pub const fn price_class(self) -> PriceClass {
        match self {
            S3Op::Put | S3Op::Copy | S3Op::List => PriceClass::PutClass,
            S3Op::Get | S3Op::GetRange | S3Op::Head => PriceClass::GetClass,
            S3Op::Delete => PriceClass::Free,
        }
    }

    /// The position of this kind in [`S3Op::ALL`], used to index counters.
    pub const fn index(self) -> usize {
        self as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_priced_like_put() {
        assert_eq!(S3Op::List.price_class(), S3Op::Put.price_class());
        assert_eq!(S3Op::List.price_class(), PriceClass::PutClass);
    }

    #[test]
    fn reads_are_get_class_and_delete_is_free() {
        for op in [S3Op::Get, S3Op::GetRange, S3Op::Head] {
            assert_eq!(op.price_class(), PriceClass::GetClass, "{op:?}");
        }
        assert_eq!(S3Op::Copy.price_class(), PriceClass::PutClass);
        assert_eq!(S3Op::Delete.price_class(), PriceClass::Free);
    }

    #[test]
    fn index_matches_position_in_all() {
        for (i, op) in S3Op::ALL.iter().enumerate() {
            assert_eq!(op.index(), i);
        }
    }
}
