use crate::{
    bytes::{BytesWrapper, ToStatic},
    hlc::Timestamp,
    log::{EntryBody, LogEntry, OpEntry, TimestampRepr},
};

impl<B: BytesWrapper, T: TimestampRepr> LogEntry<B, T> {
    pub(crate) fn transform<B2, T2, F, Err>(&self, transform_op: F) -> Result<LogEntry<B2, T2>, Err>
    where
        B2: BytesWrapper,
        T2: TimestampRepr,
        F: Fn(&OpEntry<B, T>) -> Result<OpEntry<B2, T2>, Err>,
    {
        Ok(match self {
            LogEntry::IndexedEntry(entry_body) => LogEntry::IndexedEntry(match entry_body {
                EntryBody::OpBatch(e) => EntryBody::OpBatch(transform_op(e)?),
                EntryBody::UseKey(cipher_info) => EntryBody::UseKey(*cipher_info),
                EntryBody::Expunged(hash) => EntryBody::Expunged(*hash),
            }),
            LogEntry::Signature(signature) => LogEntry::Signature(*signature),
        })
    }
}

impl ToStatic for Timestamp {
    type Static = Self;

    fn to_static(self) -> Self::Static {
        self
    }
}

impl<B: BytesWrapper, T: TimestampRepr> ToStatic for OpEntry<B, T>
where
    B::Static: BytesWrapper,
    T::Static: TimestampRepr,
{
    type Static = OpEntry<B::Static, T::Static>;

    fn to_static(self) -> Self::Static {
        OpEntry {
            timestamp: self.timestamp.to_static(),
            server_attested_user_id: self.server_attested_user_id.to_static(),
            op: self.op.to_static(),
        }
    }
}

impl<B: BytesWrapper, T: TimestampRepr> ToStatic for LogEntry<B, T>
where
    B::Static: BytesWrapper,
    T::Static: TimestampRepr,
{
    type Static = LogEntry<B::Static, T::Static>;

    fn to_static(self) -> Self::Static {
        match self {
            LogEntry::IndexedEntry(entry) => LogEntry::IndexedEntry(match entry {
                EntryBody::OpBatch(op_batch) => EntryBody::OpBatch(op_batch.to_static()),
                EntryBody::UseKey(cipher_info) => EntryBody::UseKey(cipher_info),
                EntryBody::Expunged(hash) => EntryBody::Expunged(hash),
            }),
            LogEntry::Signature(signature) => LogEntry::Signature(signature),
        }
    }
}
