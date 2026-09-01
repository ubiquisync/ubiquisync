use alloc::borrow::{Borrow, Cow};

/// Converts a value to a fully-owned version of itself with 'static lifetime.
/// This is useful for converting types which contain Cow's into
/// a fully owned version with no borrowed data.
pub trait ToStatic {
    /// The fully-owned version of self.
    type Static: 'static;
    /// Converts self to its fully-owned representation.
    fn to_static(self) -> Self::Static;
}

pub trait BytesWrapper: Borrow<[u8]> + std::fmt::Debug + From<Vec<u8>> + Default {
    fn is_empty(&self) -> bool;
}

/// A bytes wrapper that indicates that the underlying bytes are either plaintext or encrypted.
/// Opaque bytes are canonical for hashing!
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OpaqueBytes<'a>(pub Cow<'a, [u8]>);

impl<'a> From<&'a [u8]> for OpaqueBytes<'a> {
    fn from(value: &'a [u8]) -> Self {
        OpaqueBytes(Cow::Borrowed(value))
    }
}

impl From<Vec<u8>> for OpaqueBytes<'_> {
    fn from(value: Vec<u8>) -> Self {
        OpaqueBytes(Cow::Owned(value))
    }
}

impl<'a> Borrow<[u8]> for OpaqueBytes<'a> {
    fn borrow(&self) -> &[u8] {
        self.0.borrow()
    }
}

impl<'a> ToStatic for OpaqueBytes<'a> {
    type Static = OpaqueBytes<'static>;
    fn to_static(self) -> Self::Static {
        OpaqueBytes(match self.0 {
            Cow::Borrowed(b) => Cow::Owned(b.into()),
            Cow::Owned(v) => Cow::Owned(v),
        })
    }
}

impl<'a> BytesWrapper for OpaqueBytes<'a> {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
impl proptest::arbitrary::Arbitrary for OpaqueBytes<'static> {
    type Parameters = ();
    type Strategy = proptest::strategy::BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        use proptest::strategy::Strategy;

        // NOTE: empty bytes are almost never valid in this protocol so we don't generate them
        proptest::collection::vec(proptest::arbitrary::any::<u8>(), 1..=256)
            .prop_map(|v| OpaqueBytes(Cow::Owned(v)))
            .boxed()
    }
}

/// A bytes wrapper that indicates that the underlying bytes are plaintext and not encrypted.
/// Plaintext bytes are ONLY canonical for hashing if the container is NOT encrypted.
/// If the container is encrypted ONLY encrypted bytes are canonical for hashing.
/// [OpaqueBytes] MUST always be used for hashing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlaintextBytes<'a>(pub Cow<'a, [u8]>);

impl<'a> From<&'a [u8]> for PlaintextBytes<'a> {
    fn from(value: &'a [u8]) -> Self {
        PlaintextBytes(Cow::Borrowed(value))
    }
}

impl From<Vec<u8>> for PlaintextBytes<'_> {
    fn from(value: Vec<u8>) -> Self {
        PlaintextBytes(Cow::Owned(value))
    }
}

impl<'a> Borrow<[u8]> for PlaintextBytes<'a> {
    fn borrow(&self) -> &[u8] {
        self.0.borrow()
    }
}

impl<'a> ToStatic for PlaintextBytes<'a> {
    type Static = PlaintextBytes<'static>;
    fn to_static(self) -> Self::Static {
        PlaintextBytes(match self.0 {
            Cow::Borrowed(b) => Cow::Owned(b.into()),
            Cow::Owned(v) => Cow::Owned(v),
        })
    }
}

impl<'a> BytesWrapper for PlaintextBytes<'a> {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
impl proptest::arbitrary::Arbitrary for PlaintextBytes<'static> {
    type Parameters = ();
    type Strategy = proptest::strategy::BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        use proptest::strategy::Strategy;

        // NOTE: empty bytes are almost never valid in this protocol so we don't generate them
        proptest::collection::vec(proptest::arbitrary::any::<u8>(), 1..=256)
            .prop_map(|v| PlaintextBytes(Cow::Owned(v)))
            .boxed()
    }
}
