use std::time::{SystemTime, UNIX_EPOCH};
pub const RATE: usize = 16_000;
pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}
pub fn duration_ms(n: usize) -> i64 { ((n as u64 * 1000 + RATE as u64 / 2) / RATE as u64) as i64 }
pub fn wav(samples: &[f32]) -> Vec<u8> {
    let len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(len as usize + 44);
    out.extend(b"RIFF"); out.extend((36 + len).to_le_bytes()); out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes()); out.extend(1u16.to_le_bytes()); out.extend(1u16.to_le_bytes());
    out.extend((RATE as u32).to_le_bytes()); out.extend((RATE as u32 * 2).to_le_bytes());
    out.extend(2u16.to_le_bytes()); out.extend(16u16.to_le_bytes()); out.extend(b"data"); out.extend(len.to_le_bytes());
    for s in samples { out.extend(((s * 32768.0).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes()); }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn wav_header() {
        let w = wav(&[-1.0, 0.0, 1.0]);
        assert_eq!(&w[..4], b"RIFF"); assert_eq!(&w[36..40], b"data");
        assert_eq!(&w[44..], &[0,128,0,0,255,127]); assert_eq!(w.len(), 50);
    }
}
