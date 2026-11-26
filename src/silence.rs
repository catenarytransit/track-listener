//! Per-stream activity filtering after Silero VAD, before archive writes.
//! The threshold is based on recent VAD-idle frames, not on transcript text.
use crate::audio::RATE;
use std::{collections::VecDeque, ops::Range};

const FRAME: usize = 512; // same window as Silero, 32 ms at 16 kHz
const HISTORY: usize = 512; // about 16 seconds of idle audio
const MIN_HISTORY: usize = 32;
const MIN_RMS: f32 = 0.0005; // about -66 dBFS
const MAX_RMS: f32 = 0.008; // cap adaptive gating at about -42 dBFS
const PADDING: usize = RATE / 5; // keep 200 ms around the detected activity

#[derive(Default)]
pub struct NoiseFloor {
    idle: VecDeque<f32>,
}

// AC energy ignores a constant DC offset in a silent decoder output.
fn rms(samples: &[f32]) -> f32 {
    let n = samples.len() as f32;
    let (sum, squares) = samples.iter().fold((0.0, 0.0), |(sum, squares), &s| {
        (sum + s, squares + s * s)
    });
    (squares / n - (sum / n).powi(2)).max(0.0).sqrt()
}

fn lower_quintile(mut values: Vec<f32>) -> f32 {
    let index = values.len() / 5;
    *values.select_nth_unstable_by(index, f32::total_cmp).1
}

impl NoiseFloor {
    /// Only feed windows when the VAD is idle both before and after accepting
    /// them. The low percentile is resistant to speech onset contamination.
    pub fn observe(&mut self, samples: &[f32]) {
        if samples.len() != FRAME { return; }
        if self.idle.len() == HISTORY { self.idle.pop_front(); }
        self.idle.push_back(rms(samples));
    }

    pub fn speech_bounds(&self, samples: &[f32]) -> Option<Range<usize>> {
        if samples.is_empty() { return None; }
        let energies: Vec<f32> = samples.chunks(FRAME).map(rms).collect();
        let floor = if self.idle.len() >= MIN_HISTORY {
            (lower_quintile(self.idle.iter().copied().collect()) * 1.8).clamp(MIN_RMS, MAX_RMS)
        } else if energies.len() >= MIN_HISTORY {
            // Until idle history is available, reject only low, relatively
            // stationary background. Keep louder uncalibrated signals.
            (lower_quintile(energies.clone()) * 1.8).clamp(MIN_RMS, 0.003)
        } else {
            MIN_RMS
        };
        let mut first = None;
        let mut last = 0;
        let mut active = 0;
        for (i, energy) in energies.iter().enumerate() {
            if *energy > floor {
                first.get_or_insert(i * FRAME);
                last = ((i + 1) * FRAME).min(samples.len());
                active += 1;
            }
        }
        // A single click or compression spike is not a transmission. Preserve
        // short tails of capped segments if all their windows have activity.
        if active < 3.min(energies.len()) { return None; }
        let first = first?;
        Some(first.saturating_sub(PADDING)..(last + PADDING).min(samples.len()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn noise(amplitude: f32, frames: usize) -> Vec<f32> {
        let mut seed = 11u32;
        (0..frames * FRAME).map(|_| {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * amplitude
        }).collect()
    }
    fn trained(amplitude: f32) -> NoiseFloor {
        let mut floor = NoiseFloor::default();
        for frame in noise(amplitude, 80).chunks_exact(FRAME) { floor.observe(frame); }
        floor
    }
    #[test]
    fn rejects_digital_silence_dc_and_isolated_spikes() {
        let floor = NoiseFloor::default();
        assert!(floor.speech_bounds(&[0.0; FRAME * 10]).is_none());
        assert!(floor.speech_bounds(&[0.01; FRAME * 10]).is_none());
        let mut samples = vec![0.0; FRAME * 20];
        samples[FRAME * 8] = 0.9;
        assert!(floor.speech_bounds(&samples).is_none());
    }
    #[test]
    fn independently_adapts_to_different_stream_noise_floors() {
        let quiet = trained(0.0001);
        let noisy = trained(0.006);
        let hiss = noise(0.006, 80);
        assert!(noisy.speech_bounds(&hiss).is_none());
        assert!(quiet.speech_bounds(&hiss).is_some());
        let mut speech = hiss;
        speech[FRAME * 30..FRAME * 40].copy_from_slice(&noise(0.04, 10));
        let bounds = noisy.speech_bounds(&speech).unwrap();
        assert_eq!(bounds, FRAME * 30 - PADDING..FRAME * 40 + PADDING);
        let weak = noise(0.0018, 8);
        assert!(quiet.speech_bounds(&weak).is_some());
    }
    #[test]
    fn uncalibrated_low_level_hiss_is_rejected_but_louder_audio_survives() {
        let floor = NoiseFloor::default();
        assert!(floor.speech_bounds(&noise(0.002, 100)).is_none());
        assert!(floor.speech_bounds(&noise(0.02, 100)).is_some());
    }
    #[test]
    fn interior_pauses_are_not_cut_and_edges_are_padded() {
        let floor = NoiseFloor::default();
        let mut samples = vec![0.0; FRAME * 60];
        samples[FRAME * 10..FRAME * 15].copy_from_slice(&noise(0.02, 5));
        samples[FRAME * 40..FRAME * 45].copy_from_slice(&noise(0.02, 5));
        assert_eq!(floor.speech_bounds(&samples), Some(FRAME * 10 - PADDING..FRAME * 45 + PADDING));
    }
}
