//! Matching catalog songs against what's already in the Navidrome library.
use crate::model::Track;
use crate::subsonic::Client;
use std::collections::HashSet;

const IGNORABLE: [&str; 7] = ["feat", "ft.", "with", "remaster", "version", "edit", "mix"];

/// Normalise a title for comparison: drop "(feat. …)", "[Remastered]", " - 2011 Remaster",
/// punctuation and case.
pub fn norm(s: &str) -> String {
    let lower = s.to_lowercase();
    // remove bracketed groups that only describe versions/features
    let mut out = String::new();
    let mut chars = lower.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if c == '(' || c == '[' {
            let close = if c == '(' { ')' } else { ']' };
            if let Some(end) = lower[i + 1..].find(close) {
                let inner = &lower[i + 1..i + 1 + end];
                if IGNORABLE.iter().any(|k| inner.contains(k)) {
                    while let Some(&(j, _)) = chars.peek() {
                        if j > i + end {
                            break;
                        }
                        chars.next();
                    }
                    continue;
                }
            }
        }
        out.push(c);
    }
    if let Some(p) = out.find(" - ") {
        let tail = &out[p..];
        if ["remaster", "version", "edit"].iter().any(|k| tail.contains(k)) {
            out.truncate(p);
        }
    }
    out.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn words(s: &str) -> HashSet<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

pub fn artist_overlap(a: &str, b: &str) -> bool {
    let (na, nb) = (norm(a), norm(b));
    !words(a).is_disjoint(&words(b)) || (!na.is_empty() && !nb.is_empty() && (na.contains(&nb) || nb.contains(&na)))
}

fn strip_brackets(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Find a library song matching a title/artist/duration, if any.
pub fn find_song(client: &Client, title: &str, artist: &str, duration: f64) -> Option<Track> {
    let nt = norm(title);
    if nt.is_empty() {
        return None;
    }
    let q = strip_brackets(title);
    let q = if q.is_empty() { title.to_string() } else { q };
    let songs = client.search_songs(&q, 40).ok()?;
    let mut best: Option<(f64, Track)> = None;
    for s in songs {
        if norm(&s.title) != nt {
            continue;
        }
        let dd = (s.duration - duration).abs();
        if duration > 0.0 && dd > 8.0 {
            continue;
        }
        if artist_overlap(artist, &s.artist) || (duration > 0.0 && dd <= 2.0) {
            if best.as_ref().map(|(b, _)| dd < *b).unwrap_or(true) {
                best = Some((dd, s));
            }
        }
    }
    best.map(|(_, t)| t)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalises() {
        assert_eq!(norm("Some Song (feat. Someone)"), norm("Some Song"));
        assert_eq!(norm("Song - 2011 Remaster"), norm("song"));
        assert_eq!(norm("Show OP: Title Track"), "showoptitletrack");
        assert_eq!(norm("ひかりのなか"), "ひかりのなか");
        assert_ne!(norm("Heaven (Live)"), norm("Heaven")); // "live" is a real difference
        assert!(artist_overlap("Artist One;Artist Two", "Artist One & Artist Two"));
        assert!(!artist_overlap("Artist One", "Someone Else"));
    }
}
