pub enum Signature {
    Ed25519([u8; 64]),
    P256([u8; 64]),
}
