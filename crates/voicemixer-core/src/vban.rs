//! VBAN packet format, following VB-Audio's public VBAN specification:
//! a 28-byte header ("VBAN", format bytes, 16-byte stream name, frame
//! counter) followed by up to 1436 bytes of payload.

pub const DEFAULT_PORT: u16 = 6980;
pub const HEADER_SIZE: usize = 28;
pub const MAX_DATA: usize = 1436;
pub const MAX_PACKET: usize = HEADER_SIZE + MAX_DATA;
pub const MAX_SAMPLES: usize = 256;

pub const SAMPLE_RATES: [u32; 21] = [
    6000, 12000, 24000, 48000, 96000, 192000, 384000, 8000, 16000, 32000, 64000, 128000, 256000,
    512000, 11025, 22050, 44100, 88200, 176400, 352800, 705600,
];

pub const SUB_AUDIO: u8 = 0x00;
pub const SUB_SERIAL: u8 = 0x20;
pub const SUB_TEXT: u8 = 0x40;
pub const SUB_SERVICE: u8 = 0x60;

pub const DATA_U8: u8 = 0;
pub const DATA_I16: u8 = 1;
pub const DATA_I24: u8 = 2;
pub const DATA_I32: u8 = 3;
pub const DATA_F32: u8 = 4;
pub const DATA_F64: u8 = 5;
/// Codec nibble value for UTF-8 text in the TEXT sub-protocol.
pub const TEXT_UTF8: u8 = 0x10;

#[derive(Clone, Debug, PartialEq)]
pub struct Header {
    pub sub_protocol: u8,
    /// Index into [`SAMPLE_RATES`] (audio) or the bitrate table (text/serial).
    pub rate_index: u8,
    /// Samples per channel in this packet (1..=256).
    pub samples: u16,
    pub channels: u16,
    pub data_type: u8,
    pub codec: u8,
    pub name: String,
    pub frame: u32,
}

impl Header {
    pub fn sample_rate(&self) -> Option<u32> {
        SAMPLE_RATES.get(self.rate_index as usize).copied()
    }
}

pub fn rate_index(rate: u32) -> Option<u8> {
    SAMPLE_RATES
        .iter()
        .position(|&r| r == rate)
        .map(|i| i as u8)
}

pub fn bytes_per_sample(data_type: u8) -> Option<usize> {
    match data_type {
        DATA_U8 => Some(1),
        DATA_I16 => Some(2),
        DATA_I24 => Some(3),
        DATA_I32 | DATA_F32 => Some(4),
        DATA_F64 => Some(8),
        _ => None,
    }
}

pub fn parse(pkt: &[u8]) -> Option<(Header, &[u8])> {
    if pkt.len() < HEADER_SIZE || &pkt[..4] != b"VBAN" {
        return None;
    }
    let name_raw = &pkt[8..24];
    let end = name_raw.iter().position(|&b| b == 0).unwrap_or(16);
    let h = Header {
        sub_protocol: pkt[4] & 0xE0,
        rate_index: pkt[4] & 0x1F,
        samples: pkt[5] as u16 + 1,
        channels: pkt[6] as u16 + 1,
        data_type: pkt[7] & 0x07,
        codec: pkt[7] & 0xF0,
        name: String::from_utf8_lossy(&name_raw[..end]).into_owned(),
        frame: u32::from_le_bytes([pkt[24], pkt[25], pkt[26], pkt[27]]),
    };
    Some((h, &pkt[HEADER_SIZE..]))
}

pub fn write_header(h: &Header, out: &mut Vec<u8>) {
    out.extend_from_slice(b"VBAN");
    out.push((h.sub_protocol & 0xE0) | (h.rate_index & 0x1F));
    out.push(h.samples.clamp(1, 256) as u8 - 1);
    out.push(h.channels.clamp(1, 256) as u8 - 1);
    out.push((h.data_type & 0x07) | (h.codec & 0xF0));
    let mut name = [0u8; 16];
    let bytes = h.name.as_bytes();
    let k = bytes.len().min(16);
    name[..k].copy_from_slice(&bytes[..k]);
    out.extend_from_slice(&name);
    out.extend_from_slice(&h.frame.to_le_bytes());
}

/// Decodes a PCM audio payload into interleaved f32. Returns false for
/// unsupported formats or truncated packets.
pub fn decode_audio(h: &Header, data: &[u8], out: &mut Vec<f32>) -> bool {
    if h.sub_protocol != SUB_AUDIO || h.codec != 0 {
        return false;
    }
    let Some(bps) = bytes_per_sample(h.data_type) else {
        return false;
    };
    let count = h.samples as usize * h.channels as usize;
    if data.len() < count * bps {
        return false;
    }
    out.clear();
    out.reserve(count);
    for s in data[..count * bps].chunks_exact(bps) {
        let v = match h.data_type {
            DATA_U8 => (s[0] as f32 - 128.0) / 128.0,
            DATA_I16 => i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0,
            DATA_I24 => (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0,
            DATA_I32 => i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0,
            DATA_F32 => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
            _ => f64::from_le_bytes(s.try_into().unwrap()) as f32,
        };
        out.push(v);
    }
    true
}

/// Appends interleaved samples encoded as `data_type` (I16, I24 or F32).
pub fn encode_audio(samples: &[f32], data_type: u8, out: &mut Vec<u8>) {
    for &v in samples {
        let v = v.clamp(-1.0, 1.0);
        match data_type {
            DATA_I16 => out.extend_from_slice(&((v * 32767.0) as i16).to_le_bytes()),
            DATA_I24 => out.extend_from_slice(&((v * 8_388_607.0) as i32).to_le_bytes()[..3]),
            _ => out.extend_from_slice(&v.to_le_bytes()),
        }
    }
}

/// Frames that fit in one packet for a channel count and sample size.
pub fn frames_per_packet(channels: usize, bytes_per_sample: usize) -> usize {
    (MAX_DATA / (channels.max(1) * bytes_per_sample.max(1))).clamp(1, MAX_SAMPLES)
}

pub fn text_packet(stream: &str, frame: u32, text: &str) -> Vec<u8> {
    let h = Header {
        sub_protocol: SUB_TEXT,
        rate_index: 0,
        samples: 1,
        channels: 1,
        data_type: DATA_U8,
        codec: TEXT_UTF8,
        name: stream.to_string(),
        frame,
    };
    let mut out = Vec::with_capacity(HEADER_SIZE + text.len());
    write_header(&h, &mut out);
    out.extend_from_slice(&text.as_bytes()[..text.len().min(MAX_DATA)]);
    out
}

/// Text payload of a TEXT packet (trailing NULs removed).
pub fn text_payload(h: &Header, data: &[u8]) -> Option<String> {
    if h.sub_protocol != SUB_TEXT {
        return None;
    }
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    Some(String::from_utf8_lossy(&data[..end]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_round_trip() {
        let h = Header {
            sub_protocol: SUB_AUDIO,
            rate_index: rate_index(48000).unwrap(),
            samples: 3,
            channels: 2,
            data_type: DATA_I16,
            codec: 0,
            name: "Stream1".into(),
            frame: 42,
        };
        let mut pkt = Vec::new();
        write_header(&h, &mut pkt);
        let samples = [0.0, 0.5, -0.5, 0.25, 1.0, -1.0];
        encode_audio(&samples, DATA_I16, &mut pkt);
        let (h2, data) = parse(&pkt).unwrap();
        assert_eq!(h2, h);
        assert_eq!(h2.sample_rate(), Some(48000));
        let mut out = Vec::new();
        assert!(decode_audio(&h2, data, &mut out));
        for (a, b) in samples.iter().zip(&out) {
            assert!((a - b).abs() < 1e-3);
        }
    }

    #[test]
    fn int24_round_trip() {
        let mut bytes = Vec::new();
        encode_audio(&[-0.75, 0.3], DATA_I24, &mut bytes);
        let h = Header {
            sub_protocol: 0,
            rate_index: 3,
            samples: 1,
            channels: 2,
            data_type: DATA_I24,
            codec: 0,
            name: String::new(),
            frame: 0,
        };
        let mut out = Vec::new();
        assert!(decode_audio(&h, &bytes, &mut out));
        assert!((out[0] + 0.75).abs() < 1e-5 && (out[1] - 0.3).abs() < 1e-5);
    }

    #[test]
    fn text_round_trip() {
        let pkt = text_packet("Command1", 7, "Strip[0].Mute = 1;");
        let (h, data) = parse(&pkt).unwrap();
        assert_eq!(h.name, "Command1");
        assert_eq!(
            text_payload(&h, data).as_deref(),
            Some("Strip[0].Mute = 1;")
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse(b"HELLO").is_none());
        assert!(parse(&[0u8; 40]).is_none());
    }

    #[test]
    fn packet_sizes_fit() {
        assert_eq!(frames_per_packet(2, 2), 256);
        assert_eq!(frames_per_packet(8, 4), 44);
    }
}
