//! Keep recognition retry policy separate from native models for unit tests.
use crate::{audio::RATE, sentences::Sentence};
use anyhow::Result;

const RETRY_MIN_SAMPLES: usize = RATE; // One second of 16 kHz audio.

fn has_text(sentences: &[Sentence]) -> bool {
    sentences.iter().any(|s| s.text.chars().any(char::is_alphanumeric))
}

pub fn recognize_with_retry(
    samples: &[f32],
    mut decode: impl FnMut(&[f32]) -> Result<Vec<Sentence>>,
    denoise: impl FnOnce(&[f32]) -> Result<Vec<f32>>,
) -> Result<Vec<Sentence>> {
    let original = decode(samples);
    if original.as_ref().is_ok_and(|s| has_text(s)) || samples.len() <= RETRY_MIN_SAMPLES { return original; }
    // Exactly one retry, on a separate buffer. A failed retry cannot replace
    // the original error/result, and never deletes the original recording.
    match denoise(samples).and_then(|clean| decode(&clean)) {
        Ok(sentences) if has_text(&sentences) => Ok(sentences),
        Ok(_) => original,
        Err(error) => {
            tracing::warn!(%error, "denoised recognition retry failed");
            original
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    fn words(text: &str) -> Vec<Sentence> {
        vec![Sentence { text: text.into(), start_ms: Some(10), end_ms: Some(100) }]
    }
    #[test]
    fn no_retry_for_success_or_at_most_one_second() {
        for n in [RATE - 1, RATE] {
            let r = recognize_with_retry(&vec![0.1; n], |_| Ok(vec![]), |_| panic!("unexpected denoise")).unwrap();
            assert!(r.is_empty());
        }
        let r = recognize_with_retry(&vec![0.1; 4 * RATE], |_| Ok(words("Copy")), |_| panic!("unexpected denoise")).unwrap();
        assert_eq!(r[0].text, "Copy");
    }
    #[test]
    fn retries_empty_punctuation_or_error_once_without_mutating_input() {
        for initial in [Ok(vec![]), Ok(words("...")), Err(anyhow!("no result"))] {
            let original = vec![0.1; RATE + 1];
            let mut initial = Some(initial);
            let mut calls = 0;
            let result = recognize_with_retry(&original, |samples| {
                calls += 1;
                if let Some(result) = initial.take() {
                    assert_eq!(samples, original.as_slice());
                    result
                } else {
                    assert_eq!(samples.len(), original.len());
                    assert_eq!(samples[0], 0.2);
                    Ok(words("Highball"))
                }
            }, |samples| Ok(samples.iter().map(|v| v * 2.0).collect())).unwrap();
            assert_eq!(calls, 2);
            assert_eq!(result[0].text, "Highball");
            assert_eq!(original[0], 0.1);
        }
    }
    #[test]
    fn unsuccessful_retry_preserves_original_result() {
        let audio = vec![0.1; 4 * RATE];
        let error = recognize_with_retry(&audio, |_| Err(anyhow!("original")), |s| Ok(s.to_vec())).unwrap_err();
        assert_eq!(error.to_string(), "original");
        let result = recognize_with_retry(&audio, |_| Ok(vec![]), |_| Err(anyhow!("denoise failed"))).unwrap();
        assert!(result.is_empty());
    }
}
