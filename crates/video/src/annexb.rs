//! H.264 access units in Annex B form, always with 4-byte start codes
//! (`00 00 00 01`).
//!
//! The frame cryptor that encrypts our video leaves an H.264 frame clear up
//! to its first slice NAL unit. A browser rewrites NAL units to 4-byte
//! start codes before it checks the tag, and our native cryptor does not,
//! so a sender that writes 3-byte start codes fails to decrypt everywhere
//! (plan, Element Call interop; client-sdk-js#2140). Encoders hand us Annex
//! B with either length (Vulkan Video, NVENC) or length-prefixed NAL units
//! (VideoToolbox, Media Foundation); everything goes through here first.
//!
//! Only start codes are touched: the bytes of each NAL unit, emulation
//! prevention included, are copied as they are.

use crate::VideoError;

/// The start code we write.
pub const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// The NAL units of an Annex B stream, without their start codes, in order.
/// Start codes of 3 or 4 bytes are both found. Bytes before the first start
/// code are ignored; trailing zeros of a unit (they belong to the next start
/// code or are padding) are dropped.
pub fn nal_units(stream: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= stream.len() {
        if stream[i] == 0 && stream[i + 1] == 0 && stream[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let ends: Vec<usize> = starts
        .iter()
        .skip(1)
        .map(|s| s - 3)
        .chain(std::iter::once(stream.len()))
        .collect();
    starts.into_iter().zip(ends).filter_map(move |(s, e)| {
        let mut end = e;
        while end > s && stream[end - 1] == 0 {
            end -= 1;
        }
        (end > s).then(|| &stream[s..end])
    })
}

/// Appends `nal` to `out` behind a 4-byte start code.
pub fn push_nal(out: &mut Vec<u8>, nal: &[u8]) {
    out.extend_from_slice(&START_CODE);
    out.extend_from_slice(nal);
}

/// `stream` rewritten with a 4-byte start code before every NAL unit.
pub fn normalize(stream: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(stream.len() + 16);
    for nal in nal_units(stream) {
        push_nal(&mut out, nal);
    }
    out
}

/// Annex B from length-prefixed NAL units (AVCC: each unit after its
/// big-endian length of `length_size` bytes, 1, 2 or 4).
///
/// # Errors
/// [`VideoError::InvalidBitstream`] for another length size, or a length
/// that runs past the end.
pub fn from_length_prefixed(data: &[u8], length_size: usize) -> Result<Vec<u8>, VideoError> {
    if !matches!(length_size, 1 | 2 | 4) {
        return Err(VideoError::InvalidBitstream("unsupported NAL length size"));
    }
    let mut out = Vec::with_capacity(data.len() + 16);
    let mut rest = data;
    while !rest.is_empty() {
        if rest.len() < length_size {
            return Err(VideoError::InvalidBitstream("truncated NAL length"));
        }
        let (len, tail) = rest.split_at(length_size);
        let len = len.iter().fold(0usize, |n, b| (n << 8) | usize::from(*b));
        if len > tail.len() {
            return Err(VideoError::InvalidBitstream("NAL unit runs past the end"));
        }
        let (nal, next) = tail.split_at(len);
        if !nal.is_empty() {
            push_nal(&mut out, nal);
        }
        rest = next;
    }
    Ok(out)
}

/// H.264 NAL unit types we look at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NalType {
    /// A slice of a non-IDR picture.
    Slice,
    /// A slice of an IDR (key) picture.
    Idr,
    /// Supplemental enhancement information.
    Sei,
    /// Sequence parameter set.
    Sps,
    /// Picture parameter set.
    Pps,
    /// Access unit delimiter.
    Aud,
    /// Any other type (its number).
    Other(u8),
}

/// The type of an H.264 NAL unit (from its first byte).
pub fn nal_type(nal: &[u8]) -> Option<NalType> {
    let t = nal.first()? & 0x1F;
    Some(match t {
        1 => NalType::Slice,
        5 => NalType::Idr,
        6 => NalType::Sei,
        7 => NalType::Sps,
        8 => NalType::Pps,
        9 => NalType::Aud,
        other => NalType::Other(other),
    })
}

/// Whether an access unit is a keyframe: it has an IDR slice, and the SPS
/// and PPS a receiver needs to start from it.
pub fn is_keyframe(access_unit: &[u8]) -> bool {
    let (mut idr, mut sps, mut pps) = (false, false, false);
    for nal in nal_units(access_unit) {
        match nal_type(nal) {
            Some(NalType::Idr) => idr = true,
            Some(NalType::Sps) => sps = true,
            Some(NalType::Pps) => pps = true,
            _ => {}
        }
    }
    idr && sps && pps
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: &[u8] = &[0x67, 0x42, 0xE0, 0x1F, 0x00, 0x00, 0x03, 0x00, 0x01];
    const PPS: &[u8] = &[0x68, 0xCE, 0x3C, 0x80];
    const IDR: &[u8] = &[0x65, 0x88, 0x84, 0x00, 0x00, 0x03, 0x01, 0xFF];
    const SLICE: &[u8] = &[0x41, 0x9A, 0x00, 0x00, 0x03, 0x00, 0x42];

    fn three_byte(units: &[&[u8]]) -> Vec<u8> {
        units
            .iter()
            .flat_map(|u| [&[0, 0, 1][..], u].concat())
            .collect()
    }

    #[test]
    fn three_byte_start_codes_become_four() {
        let stream = three_byte(&[SPS, PPS, IDR]);
        let out = normalize(&stream);
        let expected: Vec<u8> = [SPS, PPS, IDR]
            .iter()
            .flat_map(|u| [&START_CODE[..], u].concat())
            .collect();
        assert_eq!(out, expected);
        // Already normal: unchanged.
        assert_eq!(normalize(&out), out);
    }

    #[test]
    fn mixed_start_codes_and_padding_are_handled() {
        let mut stream = vec![0xAA];
        stream.extend_from_slice(&[0, 0, 0, 1]);
        stream.extend_from_slice(SLICE);
        stream.extend_from_slice(&[0, 0]);
        stream.extend_from_slice(&[0, 0, 1]);
        stream.extend_from_slice(PPS);
        stream.extend_from_slice(&[0, 0, 0]);
        let units: Vec<&[u8]> = nal_units(&stream).collect();
        assert_eq!(units, [SLICE, PPS]);
    }

    #[test]
    fn emulation_prevention_bytes_are_kept() {
        // 00 00 03 inside a unit is not a start code and stays.
        let out = normalize(&three_byte(&[SLICE]));
        assert_eq!(&out[4..], SLICE);
    }

    #[test]
    fn length_prefixed_units_become_annex_b() {
        let mut avcc = Vec::new();
        for u in [SPS, PPS, IDR] {
            avcc.extend_from_slice(&(u.len() as u32).to_be_bytes());
            avcc.extend_from_slice(u);
        }
        let out = from_length_prefixed(&avcc, 4).unwrap();
        assert_eq!(out, normalize(&three_byte(&[SPS, PPS, IDR])));
        let two: Vec<u8> = [&[0, 4][..], PPS].concat();
        assert_eq!(
            from_length_prefixed(&two, 2).unwrap(),
            [&START_CODE[..], PPS].concat()
        );
        assert!(from_length_prefixed(&avcc[..avcc.len() - 1], 4).is_err());
        assert!(from_length_prefixed(&avcc, 3).is_err());
        assert!(from_length_prefixed(&[0, 0], 4).is_err());
    }

    #[test]
    fn keyframes_need_idr_sps_and_pps() {
        assert!(is_keyframe(&normalize(&three_byte(&[SPS, PPS, IDR]))));
        assert!(!is_keyframe(&normalize(&three_byte(&[IDR]))));
        assert!(!is_keyframe(&normalize(&three_byte(&[SPS, PPS, SLICE]))));
        assert_eq!(nal_type(SPS), Some(NalType::Sps));
        assert_eq!(nal_type(&[0x09, 0xF0]), Some(NalType::Aud));
        assert_eq!(nal_type(&[0x0C]), Some(NalType::Other(12)));
        assert_eq!(nal_type(&[]), None);
    }
}
