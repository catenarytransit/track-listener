//! Build the NeMo hotword context graph input from the model's own tokens.txt.
//! The older Parakeet v2 model archive may not ship a SentencePiece bpe.vocab.
//! Sherpa-ONNX 2.0.3 accepts explicit space-separated token pieces starting
//! with U+2581. Do not pass un-tokenized words to its cjkchar fallback.
use std::collections::{HashMap, HashSet};

const MARKER: char = '\u{2581}';

fn spellings(word: &str) -> Vec<String> {
    let word = word.trim();
    if word.eq_ignore_ascii_case("highball") {
        vec!["highball".into(), "Highball".into(), "high ball".into(), "High ball".into()]
    } else if word.eq_ignore_ascii_case("C.P.") {
        vec!["C.P.".into(), "CP".into(), "C P".into(), "C. P.".into(), "C.P".into()]
    } else {
        vec![word.into()]
    }
}

// Select a valid segmentation, preferring fewer tokens, then lower vocabulary
// IDs (usually more frequent BPE pieces). This is a fallback approximation to
// SentencePiece segmentation, not a retrained tokenizer.
fn segment<'a>(word: &str, tokens: &HashMap<&'a str, u32>) -> Option<Vec<&'a str>> {
    let mut text=String::new();
    text.push(MARKER);
    text.push_str(word);
    let n=text.len();
    let mut best:Vec<Option<(Vec<&str>,u64)>>=vec![None;n+1];
    best[n]=Some((Vec::new(),0));
    for start in (0..n).rev().filter(|i|text.is_char_boundary(*i)) {
        for end in (start+1..=n).filter(|i|text.is_char_boundary(*i)) {
            let Some((&token,&rank))=tokens.get_key_value(&text[start..end]) else {continue;};
            let Some((tail,cost))=best[end].as_ref() else {continue;};
            let score=*cost+u64::from(rank);
            if best[start].as_ref().is_none_or(|(old,old_cost)| (tail.len()+1,score)<(old.len(),*old_cost)) {
                let mut path=Vec::with_capacity(tail.len()+1);
                path.push(token);
                path.extend_from_slice(tail);
                best[start]=Some((path,score));
            }
        }
    }
    best[0].take().map(|(pieces,_)|pieces)
}

/// Return one slash-separated pretokenized phrase per spelling, plus words
/// for which none of their spellings could be encoded. Never include unknown
/// symbols or a partially encoded phrase in the context graph.
pub fn encode(words:&[String], vocabulary:&str)->(String,Vec<String>) {
    let tokens:HashMap<&str,u32>=vocabulary.lines().filter_map(|line|{
        let (token,id)=line.rsplit_once(' ')?;
        let id=id.trim().parse().ok()?;
        if token.is_empty() || token.starts_with('<') || token.starts_with(':') || token.contains('/') {
            return None;
        }
        Some((token,id))
    }).collect();
    let mut phrases=Vec::new();
    let mut seen=HashSet::new();
    let mut missing=Vec::new();
    for word in words {
        let mut encoded=false;
        for spelling in spellings(word) {
            let parts:Option<Vec<Vec<&str>>>=spelling.split_whitespace().map(|w|segment(w,&tokens)).collect();
            if let Some(parts)=parts {
                let line=parts.into_iter().flatten().collect::<Vec<_>>().join(" ");
                if !line.starts_with(MARKER) {continue;}
                encoded=true;
                if seen.insert(line.clone()) {phrases.push(line);}
            }
        }
        if !encoded {missing.push(word.clone());}
    }
    (phrases.join("/"),missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    const VOCAB:&str="▁high 1\nball 2\n▁High 3\n▁ball 4\n▁C 5\n. 6\nP 7\n▁P 8\n▁CP 9\n<unk> 0\n<blk> 10\n";
    #[test]
    fn railroad_words_and_acronym_variants_use_real_tokens() {
        let (phrases,missing)=encode(&["highball".into(),"C.P.".into()],VOCAB);
        assert!(missing.is_empty());
        let lines:Vec<_>=phrases.split('/').collect();
        assert!(lines.contains(&"▁high ball"));
        assert!(lines.contains(&"▁High ball"));
        assert!(lines.contains(&"▁high ▁ball"));
        assert!(lines.contains(&"▁C . P ."));
        assert!(lines.contains(&"▁CP"));
        assert!(lines.contains(&"▁C ▁P"));
        assert!(lines.contains(&"▁C . ▁P ."));
    }
    #[test]
    fn unknown_words_are_not_partially_biased_and_duplicates_are_removed() {
        let (phrases,missing)=encode(&["highball".into(),"highball".into(),"unknown".into()],VOCAB);
        assert_eq!(missing,vec!["unknown"]);
        assert_eq!(phrases.split('/').count(),4);
        assert_eq!(encode(&[],VOCAB),(String::new(),Vec::new()));
    }
}
