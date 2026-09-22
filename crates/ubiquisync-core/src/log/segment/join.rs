use thiserror::Error;

use crate::{
    crypto::CipherKeyResolver,
    log::{
        ChainHash, ChainHashError, LogEntry, LogHashContext, SegmentCipherError,
        entries_to_plaintext,
        segment::{
            DecodedEntries, SegmentDecodeError, SegmentEncodeError, SegmentReader,
            encode_segment_plaintext,
        },
    },
};

#[derive(Error, Debug)]
pub enum JoinSegmentsError {
    #[error("segment decode error {0}")]
    Decode(#[from] SegmentDecodeError),
    #[error("segment decode error {0}")]
    Encode(#[from] SegmentEncodeError),
    #[error("segment cipher error {0}")]
    Cipher(#[from] SegmentCipherError),
    #[error("chain hash error {0}")]
    Hash(#[from] ChainHashError),
    #[error("empty bodies")]
    Empty,
    #[error("out of order bodies, hashes don't chain")]
    OutOfOrder,
}

pub async fn join_segments<'a, B: AsRef<[u8]> + 'a>(
    key_resolver: &dyn CipherKeyResolver,
    hash_ctx: &LogHashContext,
    bodies: &'a [B],
) -> Result<JoinedSegmentData, JoinSegmentsError> {
    let mut prev_chain = None;
    let mut chain_hash = None;
    let mut start_cipher = None;
    let mut active_cipher = None;
    let mut all_entries = vec![];
    let mut sig = None;
    //     let seed = ChainSeed::new(&log_id);
    // let entry_cipher = if let Some(head)
    for body in bodies {
        let reader = SegmentReader::start(body.as_ref())?;
        let segment_header = reader.header().clone();
        let cur_hash = if let Some(chain_hash) = chain_hash {
            if chain_hash != segment_header.prev_chain {
                return Err(JoinSegmentsError::OutOfOrder);
            }
            chain_hash
        } else {
            // this is the first segment so capture prev_chain and start_cipher
            prev_chain = Some(segment_header.prev_chain);
            chain_hash = Some(segment_header.prev_chain);
            start_cipher = segment_header.start_cipher;
            // note that we DO NOT set the active cipher to segment_header.start_cipher every time
            // if we did this, we could silently accept whatever cipher the segment claims without
            // a UseKey entry. in reality, this should never occur because replicas should check
            // start_cipher validity at ommission time and thus this is really a decode-time optimization
            active_cipher = start_cipher;
            segment_header.prev_chain
        };
        let decoded = reader.read(key_resolver, hash_ctx.log_id()).await?;
        let entries = match decoded.entries {
            DecodedEntries::Opaque(items) => {
                let entries = entries_to_plaintext(
                    hash_ctx,
                    &mut active_cipher,
                    &cur_hash,
                    key_resolver,
                    items.iter(),
                )
                .await?;
                entries.into_iter().map(|e| e.0).collect()
            }
            DecodedEntries::Plaintext(items) => {
                chain_hash = Some(
                    cur_hash
                        .compute_next_plaintext(
                            hash_ctx,
                            &mut active_cipher,
                            key_resolver,
                            items.iter(),
                        )
                        .await?,
                );
                items
            }
        };
        sig = Some(segment_header.signature);

        for e in entries {
            match e {
                LogEntry::IndexedEntry(_) => all_entries.push(e),
                // we discard intermediate signatures when joining
                // in the future retain behavior could be configurable
                LogEntry::Signature(_) => {}
            }
        }
    }

    if let Some(prev_chain) = prev_chain
        && let Some(chain_hash) = chain_hash
        && let Some(sig) = sig
    {
        let body = encode_segment_plaintext(
            &sig,
            &prev_chain,
            &start_cipher,
            hash_ctx.log_id(),
            key_resolver,
            &all_entries,
        )
        .await?;
        Ok(JoinedSegmentData {
            prev_chain,
            chain_hash,
            body,
        })
    } else {
        Err(JoinSegmentsError::Empty)
    }
}

pub struct JoinedSegmentData {
    pub prev_chain: ChainHash,
    pub chain_hash: ChainHash,
    pub body: Vec<u8>,
}

#[cfg(test)]
mod tests {

    use proptest::sample::size_range;
    use secrecy::{ExposeSecret, SecretBox};
    use test_strategy::proptest;

    use crate::crypto::RootKey256;
    use crate::log::LogEntry;
    use crate::log::segment::join::join_segments;
    use crate::log::segment::tests::TestKeyResolver;
    use crate::log::segment::{DecodedEntries, SegmentReader, encode_segment_plaintext};
    use crate::log::segment::{encode_segment_opaque, tests::TestCaseData};
    use crate::log::{entries_to_opaque, segment::tests::TestCase};

    #[proptest(async = "tokio", cases = 10)]
    async fn test_join_segments(#[any(size_range(1..16).lift())] cases: Vec<(TestCase, bool)>) {
        let mut last_data: Option<TestCaseData> = None;
        let mut key_resolver = TestKeyResolver::new();
        let mut bodies = vec![];
        let mut all_entries = vec![];
        let mut start_chain = None;
        let mut end_chain = None;
        for (mut case, opaque) in cases {
            if let Some(last_data) = last_data {
                // in order to actually make our segments chain, we patch the generated
                // case data to always use data from the first segment for the key & log_id
                // and from the previous segment for start_key and prev_chain
                case.signing_key = last_data.case.signing_key;
                case.log_id = last_data.case.log_id;
                case.prev_chain = last_data.head_chain;
                case.start_key = last_data.end_cipher.and_then(|c| {
                    key_resolver
                        .get(&c.fingerprint)
                        .map(|k| *k.key.expose_secret())
                });
            }

            let data = case.data().await;
            for e in data.entries.iter() {
                if let LogEntry::IndexedEntry(_) = e {
                    all_entries.push(e.to_owned());
                }
            }

            for (k, v) in data.key_resolver.iter() {
                key_resolver.insert(
                    *k,
                    // we have to do some gymnastics to copy our key resolvers over from one
                    // segment to the next
                    RootKey256::new(SecretBox::new(Box::new(*v.key.expose_secret()))),
                );
            }

            let body = if opaque {
                let mut head_cipher = data.start_cipher;
                let opaque = entries_to_opaque(
                    &data.seed,
                    &mut head_cipher,
                    &data.prev_chain,
                    &data.key_resolver,
                    data.entries.iter(),
                )
                .await
                .unwrap();
                encode_segment_opaque(
                    &data.signature,
                    &data.prev_chain,
                    &data.start_cipher,
                    opaque.iter().map(|(e, _)| e),
                )
                .unwrap()
            } else {
                encode_segment_plaintext(
                    &data.signature,
                    &data.prev_chain,
                    &data.start_cipher,
                    &data.case.log_id,
                    &data.key_resolver,
                    &data.entries,
                )
                .await
                .unwrap()
            };
            bodies.push(body);
            if start_chain.is_none() {
                start_chain = Some(data.prev_chain);
            }
            end_chain = Some(data.head_chain);
            last_data = Some(data);
        }
        let last_data = last_data.unwrap();
        let joined = join_segments(&key_resolver, &last_data.seed, &bodies)
            .await
            .unwrap();
        assert_eq!(start_chain.unwrap(), joined.prev_chain);
        assert_eq!(end_chain.unwrap(), joined.chain_hash);

        let reader = SegmentReader::start(&joined.body).unwrap();
        let decoded = reader
            .read(&key_resolver, last_data.seed.log_id())
            .await
            .unwrap();
        let verified = decoded
            .verify(&last_data.verifying_key, &key_resolver)
            .await
            .unwrap();
        match verified.decoded.entries {
            DecodedEntries::Opaque(_) => unreachable!(),
            DecodedEntries::Plaintext(items) => {
                assert_eq!(items, all_entries);
            }
        }
        assert_eq!(verified.head_chain, last_data.head_chain);
        assert_eq!(verified.head_cipher, last_data.end_cipher);
    }
}
