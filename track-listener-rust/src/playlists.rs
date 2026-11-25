use anyhow::{Context, Result, ensure};
use reqwest::{Client, Url};
use std::collections::BTreeMap;

pub fn parse_pls(body: &str, base: &Url) -> Vec<Url> {
    let mut entries = BTreeMap::new();
    for line in body.trim_start_matches('\u{feff}').lines() {
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_ascii_lowercase();
            if let Some(n) = key.strip_prefix("file").and_then(|n| n.parse::<u32>().ok()) {
                if let Ok(url) = base.join(value.trim().trim_matches('"')) {
                    if matches!(url.scheme(), "http" | "https") { entries.insert(n,url); }
                }
            }
        }
    }
    entries.into_values().collect()
}
pub async fn resolve(client: &Client, source: &str) -> Result<Vec<Url>> {
    let url = Url::parse(source)?;
    if !url.path().to_ascii_lowercase().ends_with(".pls") { return Ok(vec![url]); }
    let mut response = client.get(url).send().await?.error_for_status()?;
    let base = response.url().clone();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(body.len() + chunk.len() <= 65536, "PLS exceeds 64 KiB");
        body.extend_from_slice(&chunk);
    }
    let urls = parse_pls(std::str::from_utf8(&body).context("PLS not UTF-8")?, &base);
    ensure!(!urls.is_empty(), "no playable HTTP(S) entries in PLS");
    Ok(urls)
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn ordered_and_relative() {
        let base = Url::parse("http://example.org/content/a.pls").unwrap();
        let urls = parse_pls("[playlist]\nFile2=http://radio.example/live\nfile1=../stream\nFile3=file:///etc/passwd", &base);
        assert_eq!(urls.iter().map(Url::as_str).collect::<Vec<_>>(), ["http://example.org/stream", "http://radio.example/live"]);
    }
}
