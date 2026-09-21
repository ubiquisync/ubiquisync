use crate::{
    bytes::{BytesWrapper, ToStatic},
    hlc::Timestamp,
    log::{EntryBody, LogEntry, OpBytesWrapper, OpEntry},
};

impl ToStatic for Timestamp {
    type Static = Self;

    fn to_static(self) -> Self::Static {
        self
    }
}

impl<'a, B: OpBytesWrapper<'a>> ToStatic for OpEntry<'a, B>
where
    B::Static: OpBytesWrapper<'static>,
{
    type Static = OpEntry<'static, B::Static>;

    fn to_static(self) -> Self::Static {
        OpEntry {
            timestamp: self.timestamp.to_static(),
            server_attested_user_id: self.server_attested_user_id.to_static(),
            op: self.op.to_static(),
        }
    }
}

impl<'a, B: OpBytesWrapper<'a>> ToStatic for LogEntry<'a, B>
where
    B::Static: OpBytesWrapper<'static>,
{
    type Static = LogEntry<'static, B::Static>;

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
