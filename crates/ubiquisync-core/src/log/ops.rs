use std::borrow::Borrow;

use crate::{
    codec::{ReadError, Reader, Writer},
    crypto::Hash256,
    log::LogEncodeError,
};

/// A batch of one or more operations in an op vocabulary.
///
/// This structure allows batching multiple operations into a single log entry
/// which should be executed atomically by the log processor, but multiple
/// operations should ONLY be used when the log processor supports this.
/// When a processor does not support this explicitly only a single operation
/// should be included.
/// Note that op backends may supporting batching multiple operations at the
/// op vocabularly level so the usefulness of the structure provided here
/// is mostly to allow for expunging a single operation in an atomic batch
/// if such functionality is desired by the op vocabularly.
/// This is a rare edge case, but is supported nevertheless because the
/// real immediate value we get from separating ops from headers here
/// is to enable encryption keys to be used with coordinate derived subkeys
/// with key reuse only occuring in the rare case that an honest peer forks,
/// in which case only a single header would reuse the same sub-key.
/// Essentially, the hash of the encrypted timestamp (and then optional server attested user id)
/// serve as an encryption nonce for each successive operation. While each entry and operation
/// should have a unique coordinate derived nonce already (based on peer id, container id,
/// entry index and slot index) if a log writer inadvertantly forked (wrote the same entry
/// at the same index - which could happen due to restore from backup scenarioes).
/// To prevent this we use the last chain hash for nonce randomness for each successive
/// entry. But even this strategy could leak the first forked entry.
/// By encrypted the timestamp first and then using the hash of its ciphertext
/// for nonce randomness, forking would only leak the timestamp in non-malicious fork scenarios
/// Of course, in a malicious scenario, the same timestamp could be used intentionally,
/// but if the writer is really malicious it would just leak the whole cipher -
/// this behavior is to prevent honest writers from leaking encrypted material in edge cases.
/// (You could argue there are rare edge cases where a timestamp collision could occur on
/// a non-malicious device, but given the way HLC's work this would require a very specific
/// sequence events on a device with a non-functional clock. In that case, the leakage would
/// extend to the server attested user id or first divergent op slot, and then its hash
/// would be input to key derivation for future entrying keeping their contents protected.)
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub struct OpBatch<B: alloc::fmt::Debug> {
    pub timestamp: B,
    pub server_attested_user_id: B,
    /// The operations in the batch, usually only 1.
    pub ops: Vec<OpOrExpunge<B>>,
}

/// Represents an operation or that operation's hash in the case it has been expunged on a per-slot basis.
/// Generally per-slot expunge shouldn't be used, there should only be one operation in a batch and if
/// we want to expunge that entry, we can just use [LogEntry::Expunged].
/// A few scenarios where per-slot expungement _could_ be useful where conceived of when this
/// structure was created, but not implemented. Now it mainly serves to separate op header and body
/// for hashing and then encryption, but the possibility of multiple ops with per-slot expungement
/// was retained because it is otherwise cheap and would be expensive to add back later.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(test, derive(test_strategy::Arbitrary))]
pub enum OpOrExpunge<Op> {
    Op(Op),
    Expunge(Hash256),
}

impl<B: alloc::fmt::Debug> OpBatch<B> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), LogEncodeError>
    where
        B: Borrow<[u8]>,
    {
        writer.write_len_prefixed(self.timestamp.borrow());
        writer.write_len_prefixed(self.server_attested_user_id.borrow());
        writer.write_var_usize(self.ops.len());
        for o in self.ops.iter() {
            o.encode(writer)?
        }
        Ok(())
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, ReadError>
    where
        B: From<&'a [u8]>,
    {
        let timestamp = reader.read_len_prefixed()?.into();
        let server_attested_user_id = reader.read_len_prefixed()?.into();
        let n = reader.read_var_usize()?;
        // NOTE: we intentionally DO NOT reserve size in the vec to prevent out-of-memory attacks!
        let mut ops = vec![];
        for _ in 0..n {
            let op = OpOrExpunge::decode(reader)?;
            ops.push(op);
        }
        Ok(Self {
            timestamp,
            server_attested_user_id,
            ops,
        })
    }
}

impl<B> OpOrExpunge<B> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), LogEncodeError>
    where
        B: Borrow<[u8]>,
    {
        match self {
            OpOrExpunge::Op(op) => {
                let bz = op.borrow();
                if bz.is_empty() {
                    return Err(LogEncodeError::EmptyOp);
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

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, ReadError>
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

    #[cfg(test)]
    use crate::bytes::OpaqueBytes;
    use crate::{
        bytes::PlaintextBytes,
        codec::{Reader, Writer},
        log::{LogEncodeError, OpBatch, OpOrExpunge},
    };

    #[proptest]
    fn test_roundtrip(op_batch: OpBatch<OpaqueBytes<'static>>) {
        // TODO we only test for opaque and that should be equivalent to plaintext otherwise,
        // but if we wanted we could use a macro to duplicate - the generic lifetimes make it really hard with just generics
        let mut w = Writer::new();
        op_batch.encode(&mut w).unwrap();
        let res = w.finalize();
        let mut r = Reader::new(&res);
        let decoded = OpBatch::<OpaqueBytes>::decode(&mut r).unwrap();
        assert_eq!(op_batch, decoded);
    }

    #[test]
    fn test_empty_op() {
        let op = OpOrExpunge::Op(PlaintextBytes(Cow::Owned(vec![])));
        let mut w = Writer::new();
        let res = op.encode(&mut w);
        assert_matches!(res, Err(LogEncodeError::EmptyOp))
    }
}
