use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Sentence {
    pub text: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
}
fn normalize(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace() && *c != '\u{2581}').flat_map(char::to_lowercase).collect()
}
fn parts(text: &str) -> Vec<&str> {
    let mut out = Vec::new(); let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !matches!(c, '.' | '?' | '!') { continue; }
        let mut end = i + c.len_utf8();
        while let Some(&(j, ch)) = chars.peek() {
            if !matches!(ch, '.' | '?' | '!' | '"' | '\'' | '\u{201d}' | '\u{2019}' | ')') { break; }
            chars.next(); end = j + ch.len_utf8();
        }
        if chars.peek().is_some_and(|(_, ch)| !ch.is_whitespace()) { continue; }
        let p = text[start..end].trim();
        let last = p.split_whitespace().last().unwrap_or("");
        if matches!(last, "Mr." | "Mrs." | "Ms." | "Dr." | "St." | "Jr." | "Sr." | "No." | "U.S.") || (last.len() == 2 && last.ends_with('.') && last.as_bytes()[0].is_ascii_uppercase()) { continue; }
        if !p.is_empty() { out.push(p); }
        start = end;
    }
    if !text[start..].trim().is_empty() { out.push(text[start..].trim()); }
    out
}
/// Align only when the normalized token sequence matches the recognized text.
/// Never invent sentence offsets when the recognizer omits or misaligns timing.
pub fn split(text: &str, tokens: &[String], times: &[f32], durations: &[f32], length: i64) -> Vec<Sentence> {
    let mut out: Vec<_> = parts(text).into_iter().map(|p| Sentence { text: p.into(), start_ms: None, end_ms: None }).collect();
    let normalized: Vec<_> = tokens.iter().map(|t| normalize(t)).collect();
    if tokens.is_empty() || tokens.len() != times.len() || normalized.concat() != normalize(text)
        || times.iter().any(|t| !t.is_finite() || *t < 0.0 || *t * 1000.0 > length as f32 + 100.0)
        || times.windows(2).any(|w| w[0] > w[1]) { return out; }
    let mut pos = 0;
    let offsets: Vec<_> = normalized.iter().map(|t| { let a = pos; pos += t.len(); (a, pos) }).collect();
    pos = 0;
    for s in &mut out {
        let end = pos + normalize(&s.text).len();
        let mut hits = offsets.iter().enumerate().filter(|(_, (a,b))| *a < end && *b > pos && a < b).map(|(i,_)| i);
        if let Some(first) = hits.next() {
            let last = hits.last().unwrap_or(first);
            let lo = (times[first] * 1000.0).round() as i64;
            let hi = if durations.len() == times.len() && durations[last].is_finite() && durations[last] > 0.0 {
                ((times[last] + durations[last]) * 1000.0).round() as i64
            } else { times.get(last+1).map(|t| (t * 1000.0).round() as i64).unwrap_or(length) };
            s.start_ms = Some(lo.clamp(0,length)); s.end_ms = Some(hi.clamp(lo.min(length),length));
        }
        pos = end;
    }
    out
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn timings() {
        let tokens = ["\u{2581}Copy", ".", "\u{2581}Thanks", "."].map(String::from);
        let r = split("Copy. Thanks.", &tokens, &[0.1,0.3,0.8,1.1], &[], 1400);
        assert_eq!(r.len(),2); assert_eq!(r[0].start_ms,Some(100)); assert_eq!(r[0].end_ms,Some(800));
        assert_eq!(r[1].start_ms,Some(800));
    }
    #[test] fn mismatched() { let r = split("No. 4 is clear. Copy!", &["bad".into()], &[0.1], &[], 1000); assert_eq!(r.len(),2); assert_eq!(r[0].start_ms,None); }
}
