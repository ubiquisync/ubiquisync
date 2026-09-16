// use thiserror::Error;

// use crate::{
//     crypto::{
//         CipherInfo, ContainerKey256, EntryCipher, RootKey256Fingerprint, SegmentCipher,
//         VerifyingKey,
//     },
//     ids::{ContainerId, LogId},
//     log::{
//         ChainHash, ChainSeed, SegmentCipherError,
//         segment::{SegmentDecodeError, SegmentReader, SegmentVerifyError},
//     },
// };

// #[derive(Error, Debug)]
// pub enum JoinSegmentsError {
//     #[error("segment decode error {0}")]
//     Decode(#[from] SegmentDecodeError),
//     #[error("segment verify error {0}")]
//     Verify(#[from] SegmentVerifyError),
//     #[error("segment cipher error {0}")]
//     Cipher(#[from] SegmentCipherError),
// }

// #[derive(Error, Debug)]
// #[error("key resolve error")]
// pub enum KeyResolveError {
//     // key not resolved
//     // unknown cipher suite
// }

// pub async fn join_segments<'a>(
//     verifying_key: &VerifyingKey,
//     key_resolver: &dyn CipherKeyResolver,
//     log_id: &LogId,
//     mut head_cipher: &Option<CipherInfo>,
//     bodies: impl Iterator<Item = &'a [u8]>,
// ) -> Result<(), JoinSegmentsError> {
//     //     let seed = ChainSeed::new(&log_id);
//     let mut prev_chain = None;
//     let mut chain_hash = None;
//     let mut all_entries = vec![];
//     let mut sig = None;
//     let entry_cipher = if let Some(head)
//     for body in bodies {
//         let reader = SegmentReader::start(body)?;
//         let segment_header = reader.header();
//         if let Some(ch) = chain_hash {
//             if segment_header.prev_chain != ch {
//                 todo!("error")
//             }
//         } else {
//             prev_chain = Some(segment_header.prev_chain);
//         }
//         let verified = reader.verify(verifying_key, entry_cipher, segment_cipher, seed)?;
//         chain_hash = Some(verified.chain_hash);
//         sig = Some(verified.signature);
//         let entries = verified.to_plaintext(entry_cipher)?;
//         for e in entries {
//             match e {
//                 crate::log::LogEntry::IndexedEntry(_) => all_entries.push(e),
//                 crate::log::LogEntry::Signature(_) => {
//                     // we discard intermediate signatures when joining
//                     // in the future retain behavior could be configurable
//                 }
//             }
//         }
//     }

//     todo!()
// }

// pub struct JoinedSegmentData {
//     pub prev_chain: ChainHash,
//     pub chain_hash: ChainHash,
//     pub body: Vec<u8>,
// }

// // pub fn join_segments<'a>(
// //     &self,
// //     peer_info: &PeerInfo,
// //     container_id: &ContainerId,
// //     bodies: impl Iterator<Item = &'a [u8]>,
// // ) -> Result<SegmentData, JoinSegmentsError> {
// //     let log_id = LogId {
// //         peer_id: peer_info.peer,
// //         container_id: *container_id,
// //     };
// //     let seed = ChainSeed::new(&log_id);
// //     let mut prev_chain = None;
// //     let mut chain_hash = None;
// //     let mut all_entries = vec![];
// //     let mut sig = None;
// //     for body in bodies {
// //         let reader = SegmentReader::start(body)?;
// //         let segment_header = reader.header();
// //         if let Some(ch) = chain_hash {
// //             if segment_header.prev_chain != ch {
// //                 todo!("error")
// //             }
// //         } else {
// //             prev_chain = Some(segment_header.prev_chain);
// //         }
// //         let verified = reader.verify(&peer_info.commitment.sig_verify_key, &None, &None, &seed)?;
// //         chain_hash = Some(verified.chain_hash);
// //         sig = Some(verified.signature);
// //         let entries = verified.to_plaintext(&None)?;
// //         all_entries.extend(entries);
// //     }

// //     // TODO get rid of unwraps
// //     let sig = sig.unwrap();
// //     let prev_chain = prev_chain.unwrap();
// //     let chain_hash = chain_hash.unwrap();

// //     let new_body = encode_segment_plaintext(&sig, &prev_chain, &None, &all_entries)?;
// //     Ok(SegmentData {
// //         body: new_body,
// //         prev_chain,
// //         chain_hash,
// //     })
// // }
