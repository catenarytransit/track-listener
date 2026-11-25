use std::time::{SystemTime, UNIX_EPOCH};
pub const RATE: usize = 16_000;
// Eight signed 16-bit PCM counts (approximately -72 dBFS peak).
// Require the ENTIRE clip to stay below this floor, not just its average.
pub const SILENCE_PEAK: i32 = 8;
pub fn is_silent(samples: &[f32]) -> bool {
    !samples.is_empty() && samples.iter().all(|s| s.abs() <= SILENCE_PEAK as f32 / 32768.0)
}
pub fn pcm_is_silent(data: &[u8]) -> bool {
    !data.is_empty() && data.len() % 2 == 0 && data.chunks_exact(2).all(|p|
        i32::from(i16::from_le_bytes([p[0],p[1]])).abs() <= SILENCE_PEAK)
}
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
    #[test] fn silence_requires_every_sample_to_be_below_floor() {
        assert!(is_silent(&[0.0; 512]));
        assert!(is_silent(&[8.0/32768.0, -8.0/32768.0]));
        assert!(!is_silent(&[]));
        assert!(!is_silent(&[f32::NAN]));
        let mut quiet = vec![0.0; 16000];
        quiet[15999] = -9.0 / 32768.0;
        assert!(!is_silent(&quiet));
        assert!(!pcm_is_silent(&wav(&quiet)[44..]));
        assert!(pcm_is_silent(&wav(&[0.0;512])[44..]));
        assert!(!pcm_is_silent(&[]));
        assert!(!pcm_is_silent(&[0]));
        assert!(!pcm_is_silent(&i16::MIN.to_le_bytes()));
    }
    #[test] fn wav_header() {
        let w = wav(&[-1.0, 0.0, 1.0]);
        assert_eq!(&w[..4], b"RIFF"); assert_eq!(&w[36..40], b"data");
        assert_eq!(&w[44..], &[0,128,0,0,255,127]); assert_eq!(w.len(), 50);
    }
}
