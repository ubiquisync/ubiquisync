use thiserror::Error;

pub const MAX_VAR_U64_SIZE: usize = 9;

pub fn encode_var_u64(v: u64, buf: &mut [u8; 9]) -> &[u8] {
    let mut v = v;
    for i in 0..9 {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            buf[i] = byte;
            return &buf[..=i];
        } else {
            // set the 8th bit to indicate that we have another byte in the varint
            // in the case where this is the last byte, this actually sets the high bit 63
            buf[i] = byte | 0x80;
        }
    }
    return buf;
}

pub fn encode_zigzag_i64(v: i64, buf: &mut [u8; 9]) -> &[u8] {
    let x = ((v << 1) ^ (v >> 63)) as u64;
    encode_var_u64(x, buf)
}

#[derive(Error, Debug)]
pub enum VarintDecodeError {
    #[error("non-minimal")]
    NonMinimal,
    #[error("unexpected EOF")]
    UnexpectedEof,
}

pub fn decode_var_u64(buf: &[u8]) -> Result<(u64, &[u8]), VarintDecodeError> {
    let mut result = 0u64;
    let mut shift = 0;
    for i in 0..buf.len() {
        let byte = buf[i];
        // if we're at the last possible byte for a u64 varint (index 8 for max length of 9)
        // we must return to not keep consuming input and overflow
        if i == 8 {
            if byte == 0 {
                return Err(VarintDecodeError::NonMinimal);
            }
            result |= (byte as u64) << shift;
            return Ok((result, &buf[i + 1..]));
        }

        // mask out the 8th bit and add the value to the result based on the current shift
        result |= ((byte & 0x7F) as u64) << shift;

        // check if the 8th bit is 0 meaning we don't have a continuation and this is the end of our varint
        if byte & 0x80 == 0 {
            // if we're past the first byte and we have a zero
            if i >= 1 && byte == 0 {
                return Err(VarintDecodeError::NonMinimal);
            }
            return Ok((result, &buf[i + 1..]));
        }
        shift += 7;
    }
    // we might end up here if the 8th bit of the last byte was 1 and we didn't have any more input
    Err(VarintDecodeError::UnexpectedEof)
}

pub fn decode_zigzag_i64(buf: &[u8]) -> Result<(i64, &[u8]), VarintDecodeError> {
    let (decoded, rest) = decode_var_u64(buf)?;
    let res = ((decoded >> 1) as i64) ^ -((decoded & 1) as i64);
    Ok((res, rest))
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use test_case::test_case;
    use test_strategy::proptest;

    use crate::codec::varint::{
        VarintDecodeError, decode_var_u64, decode_zigzag_i64, encode_var_u64, encode_zigzag_i64,
    };

    #[proptest]
    fn varints_round_trip(x: u64, tail: Vec<u8>) {
        let mut buf = [0; 9];
        let encoded = encode_var_u64(x, &mut buf);
        let mut data = vec![];
        data.extend_from_slice(&encoded);
        data.extend_from_slice(tail.as_slice());
        let res = decode_var_u64(data.as_slice());
        assert_matches!(res, Ok(_), "encoded: {encoded:?}");
        let (decoded, rest) = res.unwrap();
        assert_eq!(x, decoded, "encoded: {encoded:?}, rest: {rest:?}");
        assert_eq!(rest, tail.as_slice())
    }

    #[test_case(0, &[0])]
    #[test_case(0x7F, &[0x7F])]
    #[test_case(0x80, &[0x80, 1])]
    #[test_case(1 << 63, &[0x80; 9]; "bit 63 set has the high bit of all 9 bytes set")]
    #[test_case(u64::MAX, &[0xFF; 9]; "u64 max fills 9 bytes")]
    fn sanity_check_varints(x: u64, expected: &[u8]) {
        let mut buf = [0; 9];
        let encoded = encode_var_u64(x, &mut buf);
        assert_eq!(encoded, expected);
    }

    #[test_case(&[128, 0]; "0")]
    #[test_case(&[129, 0]; "1")]
    #[test_case(&[0xFF, 0]; "0x7F")]
    #[test_case(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0]; "0 expressed in 8 bytes")]
    fn non_minimal_varints_fail(buf: &[u8]) {
        assert_matches!(decode_var_u64(&buf), Err(VarintDecodeError::NonMinimal))
    }

    #[test_case(&[0x80])]
    #[test_case(&[0xFF, 0x80])]
    #[test_case(&[]; "empty")]
    fn truncated_varints_eof(buf: &[u8]) {
        assert_matches!(decode_var_u64(&buf), Err(VarintDecodeError::UnexpectedEof))
    }

    #[proptest]
    fn zizzags_round_trip(x: i64) {
        let mut buf = [0; 9];
        let encoded = encode_zigzag_i64(x, &mut buf);
        let res = decode_zigzag_i64(&encoded);
        assert_matches!(res, Ok(_), "encoded: {encoded:?}");
        let (decoded, rest) = res.unwrap();
        assert_eq!(x, decoded, "encoded: {encoded:?}, rest: {rest:?}");
        assert_eq!(rest.len(), 0)
    }

    #[test_case(0, &[0])]
    #[test_case(-1, &[1])]
    #[test_case(1, &[2])]
    #[test_case(i64::MIN, &[0xFF; 9])]
    #[test_case(i64::MAX, &[0xFE, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF])]
    fn sanity_check_zigzags(x: i64, expected: &[u8]) {
        let mut buf = [0; 9];
        let encoded = encode_zigzag_i64(x, &mut buf);
        assert_eq!(encoded, expected);
    }
}
