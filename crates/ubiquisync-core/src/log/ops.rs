use std::borrow::Borrow;

use crate::{
    bytes::{BytesWrapper, OpaqueBytes, PlaintextBytes, ToStatic},
    codec::{Reader, Writer},
    hlc::Timestamp,
    log::{LogDecodeError, LogEncodeError, LogValidationError},
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
pub struct OpEntry<'a, B: OpBytesWrapper<'a>> {
    /// HLC timestamp — monotonically non-decreasing within a peer's stream.
    /// Entries written in one atomic transaction share a tick, so they are
    /// treated as one logical write by LWW comparisons.
    pub timestamp: B::Timestamp,
    /// The **server-attested** user id for this entry. Every entry originates
    /// from *some* user, but this field specifically carries the identity a
    /// server vouched for — it is populated only in server-mode segments, where
    /// the server asserts attribution. `None` in device mode, where attribution
    /// is implicit from the peer directory and no server assertion exists.
    ///
    /// Do not read this as "the author"; read it as "who the server said this
    /// was." It is distinct from a stream's `peer_id` (which stream the entry
    /// came from).
    ///
    /// This _can_ be empty in server logs if and only if none of the ops are user attributable.
    pub server_attested_user_id: B,
    /// The operation in the entry.
    pub op: B,
}

pub trait OpBytesWrapper<'a>: BytesWrapper {
    type Timestamp: std::fmt::Debug + Clone + PartialEq + Eq + ToStatic;

    fn encode_ts(ts: &Self::Timestamp, w: &mut Writer);
    fn decode_ts(r: &mut Reader<'a>) -> Result<Self::Timestamp, LogDecodeError>;
}

impl<'a> OpBytesWrapper<'a> for PlaintextBytes<'a> {
    type Timestamp = Timestamp;

    fn encode_ts(ts: &Self::Timestamp, w: &mut Writer) {
        w.write_le_u64(ts.raw());
    }

    fn decode_ts(r: &mut Reader<'a>) -> Result<Self::Timestamp, LogDecodeError> {
        Ok(Timestamp::from_raw(r.read_le_u64()?))
    }
}

impl<'a> OpBytesWrapper<'a> for OpaqueBytes<'a> {
    type Timestamp = Self;

    fn encode_ts(ts: &Self::Timestamp, w: &mut Writer) {
        w.write_len_prefixed(ts.borrow());
    }

    fn decode_ts(r: &mut Reader<'a>) -> Result<Self::Timestamp, LogDecodeError> {
        let b = r.read_len_prefixed()?;
        Ok(b.into())
    }
}

impl<'a> OpEntry<'a, PlaintextBytes<'a>> {
    pub fn new(
        timestamp: Timestamp,
        server_attested_user_id: PlaintextBytes<'a>,
        op: PlaintextBytes<'a>,
    ) -> Self {
        Self {
            timestamp,
            server_attested_user_id,
            op,
        }
    }
}

impl<'a, B: OpBytesWrapper<'a>> OpEntry<'a, B> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), LogEncodeError>
    where
        B: Borrow<[u8]>,
    {
        B::encode_ts(&self.timestamp, writer);
        writer.write_len_prefixed(self.server_attested_user_id.borrow());
        writer.write_len_prefixed(self.op.borrow());
        Ok(())
    }

    pub fn decode(reader: &mut Reader<'a>) -> Result<Self, LogDecodeError>
    where
        B: From<&'a [u8]>,
    {
        let timestamp = B::decode_ts(reader)?;
        let server_attested_user_id = reader.read_len_prefixed()?.into();
        let op = reader.read_len_prefixed()?.into();
        Ok(Self {
            timestamp,
            server_attested_user_id,
            op,
        })
    }

    pub fn validate(&self) -> Result<(), LogValidationError> {
        // TODO we could validate
        // - the length range of the server attested user id
        // - the length range of the op
        // we don't have defined ranges yet, so for now this is a no-op
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::borrow::Cow;

    use proptest::prelude::*;
    use test_strategy::proptest;

    use crate::{
        bytes::{BytesWrapper, PlaintextBytes},
        codec::{Reader, Writer},
        log::{LogEncodeError, OpEntry, OpOrExpunge},
    };

    impl<B: BytesWrapper + Arbitrary + 'static> Arbitrary for OpEntry<B> {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            (
                any::<[u8; 8]>(),
                prop_oneof![Just(vec![]), any::<[u8; 16]>().prop_map(|uid| uid.to_vec())],
                proptest::collection::vec(any::<OpOrExpunge<B>>(), 1..16),
            )
                .prop_map(|(ts, uid, ops)| OpEntry {
                    timestamp: ts.to_vec().into(),
                    server_attested_user_id: uid.into(),
                    ops,
                })
                .boxed()
        }
    }

    #[proptest]
    fn test_roundtrip(op_batch: OpEntry<PlaintextBytes<'static>>) {
        // TODO we only test for opaque and that should be equivalent to plaintext otherwise,
        // but if we wanted we could use a macro to duplicate - the generic lifetimes make it really hard with just generics
        let mut w = Writer::new();
        op_batch.encode(&mut w).unwrap();
        let res = w.finalize();
        let mut r = Reader::new(&res);
        let decoded = OpEntry::<PlaintextBytes>::decode(&mut r).unwrap();
        assert_eq!(op_batch, decoded);
    }

    #[test]
    fn test_empty_op() {
        let op = OpOrExpunge::Op(PlaintextBytes(Cow::Owned(vec![])));
        let mut w = Writer::new();
        let res = op.encode(&mut w);
        assert_matches!(res, Err(LogEncodeError::EmptyOps))
    }
}
