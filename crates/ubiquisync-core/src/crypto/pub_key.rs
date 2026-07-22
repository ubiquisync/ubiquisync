pub enum PubKey {
    Ed25519([u8; 32]),
    P256([u8; 33]),
}
