//! How a query matches text: accents and case don't count, so `genie` finds
//! Génie and `rep` finds Répertoire.

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

pub fn normalize(s: &str) -> String {
    s.nfd().filter(|c| !is_combining_mark(*c)).flat_map(char::to_lowercase).collect()
}

/// Exact, then start of the text, start of a word, then inside. `None` when it
/// doesn't match. Both sides are already normalized.
fn score_word(text: &str, word: &str) -> Option<f32> {
    if text == word {
        return Some(1.0);
    }
    if text.starts_with(word) {
        return Some(0.85 + 0.1 * word.len() as f32 / text.len() as f32);
    }
    let mut best = None;
    for (at, _) in text.match_indices(word) {
        let word_start = text[..at].chars().next_back().is_some_and(|c| !c.is_alphanumeric());
        if word_start {
            return Some(0.7);
        }
        best = Some(0.5);
    }
    best
}

/// Every word of the query must match one of the fields, each given with a
/// weight. The score is the average over the words, between 0 and 1.
pub fn score(query: &str, fields: &[(&str, f32)]) -> Option<f32> {
    let query = normalize(query);
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return None;
    }
    let fields: Vec<(String, f32)> = fields.iter().map(|(t, w)| (normalize(t), *w)).collect();
    let mut total = 0.0;
    for word in &words {
        total += fields
            .iter()
            .filter_map(|(text, weight)| score_word(text, word).map(|s| s * weight))
            .fold(None, |best: Option<f32>, s| Some(best.map_or(s, |b| b.max(s))))?;
    }
    Some(total / words.len() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(query: &str, text: &str) -> Option<f32> {
        score(query, &[(text, 1.0)])
    }

    #[test]
    fn accents_and_case_are_ignored() {
        assert_eq!(one("GENIE", "Génie"), Some(1.0));
        assert!(one("rep", "Répertoire").is_some());
    }

    #[test]
    fn better_matches_score_higher() {
        let exact = one("fire", "fire").unwrap();
        let prefix = one("fire", "firefox").unwrap();
        let word = one("fox", "fire fox").unwrap();
        let inside = one("efo", "firefox").unwrap();
        assert!(exact > prefix && prefix > word && word > inside);
    }

    #[test]
    fn every_word_has_to_match() {
        assert!(score("epic games", &[("Heroic", 1.0), ("epic games", 0.8)]).is_some());
        assert!(one("fire chrome", "firefox").is_none());
        assert!(one("  ", "firefox").is_none());
    }

    #[test]
    fn letters_with_gaps_dont_match() {
        assert!(one("fire", "LibreOffice Impress").is_none());
    }
}
