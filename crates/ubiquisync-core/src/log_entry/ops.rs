use std::borrow::Borrow;

use crate::{
    codec::{reader::Reader, writer::Writer},
    crypto::Hash,
    log_entry::{DecodeError, error::EncodeError},
};

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct OpBatch<Op: alloc::fmt::Debug, H: alloc::fmt::Debug> {
    pub header: H,
    pub ops: Vec<OpOrExpunge<Op>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum OpOrExpunge<Op> {
    Op(Op),
    Expunge(Hash),
}

impl<B: alloc::fmt::Debug, H: alloc::fmt::Debug> OpBatch<B, H> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), EncodeError>
    where
        B: Borrow<[u8]>,
        H: Borrow<[u8]>,
    {
        writer.write_len_prefixed(self.header.borrow());
        writer.write_var_usize(self.ops.len());
        for o in self.ops.iter() {
            o.encode(writer)?
        }
        Ok(())
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, DecodeError>
    where
        B: From<&'a [u8]>,
        H: From<&'a [u8]>,
    {
        let header: H = reader.read_len_prefixed()?.into();
        let n = reader.read_var_usize()?;
        // NOTE: we intentionally DO NOT reserve size in the vec to prevent out-of-memory attacks!
        let mut ops = vec![];
        for _ in 0..n {
            let op = OpOrExpunge::decode(reader)?;
            ops.push(op);
        }
        Ok(Self { header, ops })
    }
}

impl<B> OpOrExpunge<B> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), EncodeError>
    where
        B: Borrow<[u8]>,
    {
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

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, DecodeError>
    where
        B: From<&'a [u8]>,
    {
        let n = reader.read_var_usize()?;
        if n == 0 {
            Ok(Self::Expunge(reader.read_array()?))
        } else {
            Ok(Self::Op(reader.read_slice(n)?.into()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::borrow::Cow;

    use test_strategy::proptest;

    use crate::{
        codec::{reader::Reader, writer::Writer},
        log_entry::{EncodeError, OpBatch, OpOrExpunge, OpaqueBytes, PlaintextBytes},
    };

    #[proptest]
    fn test_roundtrip(op_batch: OpBatch<OpaqueBytes<'static>, OpaqueBytes<'static>>) {
        // TODO we only test for opaque and that should be equivalent to plaintext otherwise,
        // but if we wanted we could use a macro to duplicate - the generic lifetimes make it really hard with just generics
        let mut w = Writer::new();
        op_batch.encode(&mut w).unwrap();
        let res = w.finalize();
        let mut r = Reader::new(&res);
        let decoded = OpBatch::<OpaqueBytes, OpaqueBytes>::decode(&mut r).unwrap();
        assert_eq!(op_batch, decoded);
    }

    #[test]
    fn test_empty_op() {
        let op = OpOrExpunge::Op(PlaintextBytes(Cow::Owned(vec![])));
        let mut w = Writer::new();
        let res = op.encode(&mut w);
        assert_matches!(res, Err(EncodeError::EmptyOp))
    }
}
