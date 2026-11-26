//! RNNoise operates on 48 kHz mono; Parakeet and the archive use 16 kHz.
use anyhow::{Result, anyhow, ensure};
use flexaudio_denoise::{Denoiser, FRAME_SIZE};
use std::f32::consts::PI;

const RATIO: usize = 3;
const RADIUS: i32 = 24;

// Symmetric Hann-windowed sinc at 48 kHz. The low-pass suppresses interpolation
// images before RNNoise and aliasing when converting its output back to 16 kHz.
fn kernel() -> Vec<f32> {
    let cutoff = 7500.0 / 48000.0;
    let mut weights: Vec<f32> = (-RADIUS..=RADIUS).map(|i| {
        let x = i as f32;
        let sinc = if i == 0 { 2.0 * cutoff } else { (2.0 * PI * cutoff * x).sin() / (PI * x) };
        sinc * (0.5 + 0.5 * (PI * x / RADIUS as f32).cos())
    }).collect();
    let sum: f32 = weights.iter().sum();
    for w in &mut weights { *w /= sum; }
    weights
}

fn upsample(samples: &[f32], weights: &[f32]) -> Vec<f32> {
    let phases: Vec<Vec<(isize, f32)>> = (0..RATIO).map(|phase| {
        weights.iter().enumerate().filter_map(|(i, &w)| {
            let offset = phase as isize + i as isize - RADIUS as isize;
            (offset % RATIO as isize == 0).then_some((offset / RATIO as isize, w * RATIO as f32))
        }).collect()
    }).collect();
    let mut output = Vec::with_capacity(samples.len() * RATIO);
    for i in 0..samples.len() {
        for phase in &phases {
            let sample: f32 = phase.iter().map(|&(offset, w)| {
                samples[(i as isize + offset).clamp(0, samples.len() as isize - 1) as usize] * w
            }).sum();
            output.push(sample.clamp(-1.0, 1.0));
        }
    }
    output
}

fn downsample(samples: &[f32], weights: &[f32]) -> Vec<f32> {
    (0..samples.len()).step_by(RATIO).map(|i| {
        let sample: f32 = weights.iter().enumerate().map(|(j, &w)| {
            let index = (i as isize + j as isize - RADIUS as isize).clamp(0, samples.len() as isize - 1);
            samples[index as usize] * w
        }).sum();
        sample.clamp(-1.0, 1.0)
    }).collect()
}

pub fn denoise(samples: &[f32]) -> Result<Vec<f32>> {
    if samples.is_empty() { return Ok(Vec::new()); }
    ensure!(samples.iter().all(|s| s.is_finite()), "invalid audio samples");
    let weights = kernel();
    let mut audio = upsample(samples, &weights);
    let length = audio.len();
    // Process whole RNNoise frames, including the final partial frame padded
    // with zeros, then remove the documented 480-sample delay and padding.
    audio.resize(length.div_ceil(FRAME_SIZE) * FRAME_SIZE, 0.0);
    let mut denoiser = Denoiser::new(1).map_err(|e| anyhow!("create denoiser: {e:?}"))?;
    denoiser.process(&mut audio).map_err(|e| anyhow!("denoise: {e:?}"))?;
    audio.extend(denoiser.flush());
    ensure!(audio.len() >= FRAME_SIZE + length, "short denoiser output");
    let cleaned = downsample(&audio[FRAME_SIZE..FRAME_SIZE + length], &weights);
    ensure!(cleaned.len() == samples.len() && cleaned.iter().all(|s| s.is_finite()),
        "invalid denoised audio");
    Ok(cleaned)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_length_and_original_pcm_for_partial_frames() {
        let input: Vec<f32> = (0..48_001).map(|i| (i as f32 * 0.09).sin() * 0.1).collect();
        let original = input.clone();
        let cleaned = denoise(&input).unwrap();
        assert_eq!(cleaned.len(), input.len());
        assert!(cleaned.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        assert_eq!(input, original);
        assert!(denoise(&[]).unwrap().is_empty());
    }
    #[test]
    fn resampling_preserves_voice_band_and_alignment() {
        let input: Vec<f32> = (0..16_001).map(|i| (i as f32 * 2.0 * PI * 1000.0 / 16000.0).sin() * 0.1).collect();
        let weights = kernel();
        let up = upsample(&input, &weights);
        assert_eq!(up.len(), 3 * input.len());
        let down = downsample(&up, &weights);
        assert_eq!(down.len(), input.len());
        assert!(input[32..input.len()-32].iter().zip(&down[32..down.len()-32]).all(|(a,b)| (a-b).abs() < 0.002));
    }
}
