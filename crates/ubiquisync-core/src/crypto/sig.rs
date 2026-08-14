use crate::{
    codec::{reader::Reader, writer::Writer},
    log_entry::DecodeError,
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
                writer.write_byte(SIG_ED25519);
                s
            }
            Signature::P256(s) => {
                writer.write_byte(SIG_ED25519);
                s
            }
        };
        writer.write_array(s);
    }

    pub fn decode(reader: &mut Reader) -> Result<Self, DecodeError> {
        let algo = reader.read_byte()?;
        Ok(match algo {
            SIG_ED25519 => Signature::Ed25519(reader.read_array()?),
            SIG_P256 => Signature::P256(reader.read_array()?),
            _ => return Err(DecodeError::UknownSignatureAlgorithm(algo)),
        })
    }
}

pub const SIG_ED25519: u8 = 0x0;
pub const SIG_P256: u8 = 0x1;
