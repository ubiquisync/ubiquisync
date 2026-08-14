use alloc::borrow::{Borrow, Cow};

/// A bytes wrapper that indicates that the underlying bytes are either plaintext or encrypted.
/// Opaque bytes are canonical for hashing!
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueBytes<'a>(pub Cow<'a, [u8]>);

impl<'a> From<&'a [u8]> for OpaqueBytes<'a> {
    fn from(value: &'a [u8]) -> Self {
        OpaqueBytes(Cow::Borrowed(value))
    }
}

impl<'a> Borrow<[u8]> for OpaqueBytes<'a> {
    fn borrow(&self) -> &[u8] {
        self.0.borrow()
    }
}

#[cfg(test)]
impl proptest::arbitrary::Arbitrary for OpaqueBytes<'static> {
    type Parameters = ();
    type Strategy = proptest::strategy::BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        use proptest::strategy::Strategy;

        proptest::collection::vec(proptest::arbitrary::any::<u8>(), 0..=256)
            .prop_map(|v| OpaqueBytes(Cow::Owned(v)))
            .boxed()
    }
}

/// A bytes wrapper that indicates that the underlying bytes are plaintext and not encrypted.
/// Plaintext bytes are ONLY canonical for hashing if the container is NOT encrypted.
/// If the container is encrypted ONLY encrypted bytes are canonical for hashing.
/// [OpaqueBytes] MUST always be used for hasing.
#[derive(Debug, Clone, PartialEq, Eq)]

pub struct PlaintextBytes<'a>(pub Cow<'a, [u8]>);

impl<'a> From<&'a [u8]> for PlaintextBytes<'a> {
    fn from(value: &'a [u8]) -> Self {
        PlaintextBytes(Cow::Borrowed(value))
    }
}

impl<'a> Borrow<[u8]> for PlaintextBytes<'a> {
    fn borrow(&self) -> &[u8] {
        self.0.borrow()
    }
}

#[cfg(test)]
impl proptest::arbitrary::Arbitrary for PlaintextBytes<'static> {
    type Parameters = ();
    type Strategy = proptest::strategy::BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        use proptest::strategy::Strategy;

        proptest::collection::vec(proptest::arbitrary::any::<u8>(), 0..=256)
            .prop_map(|v| PlaintextBytes(Cow::Owned(v)))
            .boxed()
    }
}
