//! Japanese → romaji for lyrics. Uses a morphological analyser (lindera + IPADIC, the same
//! dictionary MeCab uses) for word boundaries and readings, and kakasi for any kanji the
//! dictionary doesn't know. The dictionary is only touched the first time it's needed.
use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use std::borrow::Cow;
use std::sync::OnceLock;
use wana_kana::ConvertJapanese;

fn is_kana(c: char) -> bool {
    matches!(c, '\u{3040}'..='\u{309F}' | '\u{30A0}'..='\u{30FF}' | '\u{31F0}'..='\u{31FF}' | '\u{FF66}'..='\u{FF9F}')
}
fn is_kanji(c: char) -> bool {
    matches!(c, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '々')
}

/// Text contains kana (so it's Japanese, not Chinese).
pub fn has_kana(s: &str) -> bool {
    s.chars().any(is_kana)
}
/// Text contains kana or kanji.
pub fn has_japanese(s: &str) -> bool {
    s.chars().any(|c| is_kana(c) || is_kanji(c))
}

fn segmenter() -> Option<&'static Segmenter> {
    static SEG: OnceLock<Option<Segmenter>> = OnceLock::new();
    SEG.get_or_init(|| load_dictionary("embedded://ipadic").ok().map(|d| Segmenter::new(Mode::Normal, d, None))).as_ref()
}

fn punct(s: &str) -> Option<&'static str> {
    Some(match s {
        "、" | "，" => ",",
        "。" | "．" => ".",
        "！" => "!",
        "？" => "?",
        "「" | "」" | "『" | "』" => "\"",
        "（" => "(",
        "）" => ")",
        "〜" | "～" => "~",
        "・" => " ",
        "…" => "...",
        _ => return None,
    })
}

/// Spellings the dictionary reads wrong (usually old or rare kanji), swapped for their kana
/// reading before analysis. Add to this as they turn up in lyrics.
const READINGS: [(&str, &str); 6] = [
    ("疾うに", "とうに"),   // long since (dictionary: shitsu uni)
    ("淹れ", "いれ"),       // brew tea (kakasi fallback: enre)
    ("所以", "ゆえん"),
    ("刹那", "せつな"),
    ("躱", "かわ"),         // dodge
    ("微睡", "まどろ"),     // doze
];

/// Romanize one line of Japanese text.
pub fn romanize(text: &str) -> String {
    let mut fixed = Cow::Borrowed(text);
    for (k, r) in READINGS {
        if fixed.contains(k) {
            fixed = Cow::Owned(fixed.replace(k, r));
        }
    }
    let text = fixed.as_ref();
    let Some(seg) = segmenter() else { return kakasi::convert(text).romaji };
    let Ok(mut tokens) = seg.segment(Cow::Owned(text.to_string())) else { return kakasi::convert(text).romaji };
    // (piece, attach-to-previous-word). Kana stays kana until whole words are joined, so a
    // trailing っ can double the next consonant across tokens: やっ+て → yatte, not "yate".
    let mut parts: Vec<(Piece, bool)> = vec![];
    for t in tokens.iter_mut() {
        let surface = t.surface.to_string();
        let d: Vec<String> = t.details().iter().map(|s| s.to_string()).collect();
        let pos = d.first().map(String::as_str).unwrap_or("");
        let pos1 = d.get(1).map(String::as_str).unwrap_or("");
        if surface.trim().is_empty() {
            continue;
        }
        if let Some(p) = punct(&surface) {
            parts.push((Piece::Text(p.to_string()), p != " " && p != "("));
            continue;
        }
        if !has_japanese(&surface) {
            parts.push((Piece::Text(surface), false)); // latin words, numbers, symbols stay as written
            continue;
        }
        let reading = d.get(7).filter(|r| r.as_str() != "*" && !r.is_empty()).cloned();
        let roma = match (pos, surface.as_str()) {
            ("助詞", "は") => Piece::Text("wa".into()),
            ("助詞", "へ") => Piece::Text("e".into()),
            ("助詞", "を") => Piece::Text("wo".into()),
            _ => match reading {
                Some(r) => Piece::Kana(r),
                None if surface.chars().any(is_kanji) => Piece::Text(kakasi::convert(&surface).romaji.replace(' ', "")),
                None => Piece::Kana(surface.clone()),
            },
        };
        // glue inflections and suffixes onto their word: 食べ+た → tabeta, かけ+て → kakete
        let base = d.get(6).map(String::as_str).unwrap_or("");
        let copula = matches!(base, "だ" | "です"); // なら / だ / です read better as separate words
        let attach = (pos == "助動詞" && !copula)
            || (pos == "助詞" && pos1 == "接続助詞")
            || pos1 == "接尾"
            || (pos == "動詞" && pos1 == "非自立" && surface.chars().count() <= 1);
        parts.push((roma, attach));
    }
    let mut out = String::new();
    let mut kana = String::new(); // pending kana of the current word
    for (piece, attach) in parts {
        if !attach {
            out.push_str(&kana.to_romaji());
            kana.clear();
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
        }
        match piece {
            Piece::Kana(k) => kana.push_str(&k),
            Piece::Text(t) => {
                out.push_str(&kana.to_romaji());
                kana.clear();
                out.push_str(&t);
            }
        }
    }
    out.push_str(&kana.to_romaji());
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

enum Piece {
    /// a reading, romanized once its whole word is known
    Kana(String),
    /// already final (latin text, punctuation, particles, kakasi output)
    Text(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lines() {
        assert_eq!(romanize("君は僕へ"), "kimi wa boku e");
        assert_eq!(romanize("受け継ぐ夢を 運命と呼ぶなら"), "uketsugu yume wo unmei to yobu nara");
        let r = romanize("お茶を淹れたくて");
        assert!(!has_japanese(&r), "left kanji in {r}");
        assert!(r.contains("iretakute"), "{r}");
        assert!(romanize("魔法をかけてみる").contains("kakete"));
        assert!(romanize("お金が減っていく").ends_with("hette iku"), "{}", romanize("お金が減っていく"));
        assert!(romanize("待って").contains("matte"));
        assert!(romanize("疾うに忘れた").starts_with("touni"), "{}", romanize("疾うに忘れた"));
        assert!(romanize("刹那の光").contains("setsuna"), "{}", romanize("刹那の光"));
        assert!(romanize("攻撃を躱して").contains("kawashite"), "{}", romanize("攻撃を躱して"));
        assert_eq!(romanize("One two"), "One two");
        assert!(has_kana("サヨナラ") && !has_kana("你好") && has_japanese("你好"));
    }
    #[test]
    #[ignore]
    fn show() {
        for l in ["朝の光が 窓を照らす", "雨の日には 傘を忘れた", "お茶を淹れたくて", "夕焼けに、とけてく", "微睡む猫と 静かな午後", "僕らは今 歩き出した"] {
            println!("{l}\n   {}", romanize(l));
        }
    }
}
