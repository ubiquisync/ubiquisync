use crate::{
    bytes::ToStatic,
    hlc::Timestamp,
    log::{EntryBody, LogEntry, OpBytesWrapper, OpEntry},
};

impl<'a, B: OpBytesWrapper<'a>> LogEntry<'a, B> {
    pub(crate) fn transform<C, F, Err>(&self, transform_op: F) -> Result<LogEntry<'a, C>, Err>
    where
        C: OpBytesWrapper<'a>,
        F: Fn(&OpEntry<'a, B>) -> Result<OpEntry<'a, C>, Err>,
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

impl<'a, B: OpBytesWrapper<'a>> ToStatic for OpEntry<'a, B>
where
    B::Static: OpBytesWrapper<'static>,
    B::Timestamp: ToStatic<Static = <B::Static as OpBytesWrapper<'static>>::Timestamp>,
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
    B::Timestamp: ToStatic<Static = <B::Static as OpBytesWrapper<'static>>::Timestamp>,
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
