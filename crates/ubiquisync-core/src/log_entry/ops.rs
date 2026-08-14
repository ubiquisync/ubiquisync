use std::borrow::Borrow;

use crate::{
    codec::{reader::Reader, writer::Writer},
    crypto::Hash,
    log_entry::{DecodeError, error::EncodeError},
};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct OpBatch<Op: std::fmt::Debug, H: std::fmt::Debug> {
    pub header: H,
    pub ops: Vec<OpOrExpunge<Op>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum OpOrExpunge<Op> {
    Op(Op),
    Expunge(Hash),
}

impl<B: Borrow<[u8]>> OpOrExpunge<B> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), EncodeError> {
        match self {
            OpOrExpunge::Op(op) => {
                let bz = op.borrow();
                if bz.is_empty() {
                    return Err(EncodeError::EmptyOp);
                }
                writer.write_len_prefixed(bz);
            }
            OpOrExpunge::Expunge(hash) => {
                // expunged entries are always prefixed with 0
                // this is safe because empty op bodies are NOT allowed
                writer.write_var_usize(0);
                writer.write_array(hash);
            }
        }
        Ok(())
    }
}

impl<'a, B: From<&'a [u8]>> OpOrExpunge<B> {
    pub fn decode(reader: &mut Reader<'a>) -> Result<Self, DecodeError> {
        let n = reader.read_var_usize()?;
        if n == 0 {
            Ok(Self::Expunge(reader.read_array()?))
        } else {
            Ok(Self::Op(reader.read_slice(n)?.into()))
        }
    }
}
