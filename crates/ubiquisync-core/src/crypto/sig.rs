use crate::{
    codec::{reader::Reader, writer::Writer},
    crypto::CryptoDecodeError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum Signature {
    Ed25519([u8; 64]),
    P256([u8; 64]),
}

impl Signature {
    pub fn encode(&self, writer: &mut Writer) {
        let s = match self {
            Signature::Ed25519(s) => {
                writer.write_byte(SIG_ALGO_ED25519);
                s
            }
            Signature::P256(s) => {
                writer.write_byte(SIG_ALGO_P256);
                s
            }
        };
        writer.write_array(s);
    }

    pub fn decode(reader: &mut Reader) -> Result<Self, CryptoDecodeError> {
        let algo = reader.read_byte()?;
        Ok(match algo {
            SIG_ALGO_ED25519 => Signature::Ed25519(reader.read_array()?),
            SIG_ALGO_P256 => Signature::P256(reader.read_array()?),
            _ => return Err(CryptoDecodeError::UnknownAlgorithm(algo).into()),
        })
    }
}

pub const SIG_ALGO_ED25519: u8 = 0x0;
pub const SIG_ALGO_P256: u8 = 0x1;

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use test_strategy::proptest;

    use crate::codec::{reader::Reader, writer::Writer};
    use crate::crypto::{CryptoDecodeError, Signature};

    #[proptest]
    fn test_roundtrip(signature: Signature) {
        let mut w = Writer::new();
        signature.encode(&mut w);
        let res = w.finalize();
        let mut r = Reader::new(&res);
        let decoded = Signature::decode(&mut r).unwrap();
        assert_eq!(signature, decoded);
    }

    #[test]
    fn test_unknown() {
        let mut w = Writer::new();
        w.write_byte(46);
        w.write_array(&[0, 1, 3, 4, 5]);
        let res = w.finalize();
        let mut r = Reader::new(&res);
        let decoded = Signature::decode(&mut r);
        assert_matches!(decoded, Err(CryptoDecodeError::UnknownAlgorithm(46)))
    }
}
