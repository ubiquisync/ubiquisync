#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum Signature {
    Ed25519([u8; 64]),
    P256([u8; 64]),
}
