pub(crate) fn encode_hex(hash: &[u8; 32]) -> String {
    let mut s = String::new();
    for b in hash {
        write!(s, "{b:02x}").unwrap()
    }
}
