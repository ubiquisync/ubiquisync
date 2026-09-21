use std::borrow::Borrow;

use crate::{
    bytes::{BytesWrapper, OpaqueBytes, PlaintextBytes, ToStatic},
    codec::{Reader, Writer},
    hlc::Timestamp,
    log::{LogDecodeError, LogEncodeError, LogValidationError},
};

/// An entry wrapping a single operation with a timestamp and optional
/// server-attested user ID (only used in server mode).
///
/// The timestamp, server-attested user ID, and operation are all encrypted
/// separately so that the cipher-text of each step can feed into key-derivation
/// of the next step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpEntry<B: BytesWrapper, T: TimestampRepr> {
    /// HLC timestamp — monotonically non-decreasing within a peer's stream.
    /// Entries written in one atomic transaction share a tick, so they are
    /// treated as one logical write by LWW comparisons.
    pub timestamp: T,
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

pub trait TimestampRepr: Sized + std::fmt::Debug + Clone + PartialEq + Eq + ToStatic {
    fn encode(&self, w: &mut Writer);
}

pub trait DecodeTimestamp<'a>: TimestampRepr {
    fn decode(r: &mut Reader<'a>) -> Result<Self, LogDecodeError>;
}

impl TimestampRepr for Timestamp {
    fn encode(&self, w: &mut Writer) {
        w.write_le_u64(self.raw());
    }
}

impl<'a> DecodeTimestamp<'a> for Timestamp {
    fn decode(r: &mut Reader<'a>) -> Result<Self, LogDecodeError> {
        Ok(Self::from_raw(r.read_le_u64()?))
    }
}

impl<'a> TimestampRepr for OpaqueBytes<'a> {
    fn encode(&self, w: &mut Writer) {
        w.write_len_prefixed(self.borrow());
    }
}

impl<'a> DecodeTimestamp<'a> for OpaqueBytes<'a> {
    fn decode(r: &mut Reader<'a>) -> Result<Self, LogDecodeError> {
        let b = r.read_len_prefixed()?;
        Ok(b.into())
    }
}

impl<'a> OpEntry<PlaintextBytes<'a>, Timestamp> {
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

impl<B: BytesWrapper, T: TimestampRepr> OpEntry<B, T> {
    pub fn encode(&self, writer: &mut Writer) -> Result<(), LogEncodeError> {
        self.timestamp.encode(writer);
        writer.write_len_prefixed(self.server_attested_user_id.borrow());
        writer.write_len_prefixed(self.op.borrow());
        Ok(())
    }

    pub fn decode<'a>(reader: &mut Reader<'a>) -> Result<Self, LogDecodeError>
    where
        B: From<&'a [u8]>,
        T: DecodeTimestamp<'a>,
    {
        let timestamp = T::decode(reader)?;
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
    use proptest::prelude::*;
    use test_strategy::proptest;

    use crate::{
        bytes::{BytesWrapper, PlaintextBytes},
        codec::{Reader, Writer},
        hlc::Timestamp,
        log::{OpEntry, TimestampRepr},
    };

    impl<B: BytesWrapper + Arbitrary + 'static, T: TimestampRepr + Arbitrary + 'static> Arbitrary
        for OpEntry<B, T>
    {
        type Parameters = ();
        type Strategy = BoxedStrategy<Self>;

        fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
            (
                any::<T>(),
                prop_oneof![Just(vec![]), any::<[u8; 16]>().prop_map(|uid| uid.to_vec())],
                any::<B>(),
            )
                .prop_map(|(ts, uid, op)| OpEntry {
                    timestamp: ts,
                    server_attested_user_id: uid.into(),
                    op,
                })
                .boxed()
        }
    }

    #[proptest]
    fn test_roundtrip(op: OpEntry<PlaintextBytes<'static>, Timestamp>) {
        // TODO we only test for opaque and that should be equivalent to plaintext otherwise,
        // but if we wanted we could use a macro to duplicate - the generic lifetimes make it really hard with just generics
        let mut w = Writer::new();
        op.encode(&mut w).unwrap();
        let res = w.finalize();
        let mut r = Reader::new(&res);
        let decoded = OpEntry::<PlaintextBytes, Timestamp>::decode(&mut r).unwrap();
        assert_eq!(op, decoded);
    }
}
