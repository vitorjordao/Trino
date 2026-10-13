//! Command palette (Ctrl+K): every menu action, searchable by typing —
//! the same commands the menus, the shortcuts and (as files) an AI agent
//! use. This module is the matcher; the app builds the item list.

/// One palette entry.
pub struct Item<T> {
    pub label: String,
    /// Shortcut or context shown on the right.
    pub hint: String,
    pub payload: T,
}

/// Fuzzy subsequence score of `query` in `label` (case-insensitive):
/// `None` if the characters do not appear in order; higher is better.
/// Rewards matches at word starts and consecutive runs, so "al x" finds
/// "Align X — min" and "dup" finds "Duplicate".
pub fn score(query: &str, label: &str) -> Option<i32> {
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if q.is_empty() {
        return Some(0);
    }
    let l: Vec<char> = label.chars().flat_map(char::to_lowercase).collect();
    let mut qi = 0;
    let mut total = 0;
    let mut run = 0;
    let mut prev_matched = false;
    for (i, &c) in l.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            let word_start = i == 0 || !l[i - 1].is_alphanumeric();
            run = if prev_matched { run + 1 } else { 1 };
            total += 1 + run * 2 + if word_start { 6 } else { 0 };
            qi += 1;
            prev_matched = true;
        } else {
            prev_matched = false;
            if qi > 0 && qi < q.len() {
                total -= 1; // gap inside the match
            }
        }
    }
    (qi == q.len()).then_some(total - (l.len() as i32) / 8)
}

/// Indices of `items` matching `query`, best first (stable for ties).
pub fn filter<T>(items: &[Item<T>], query: &str) -> Vec<usize> {
    let mut hits: Vec<(i32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| score(query, &it.label).map(|s| (s, i)))
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(labels: &[&str]) -> Vec<Item<()>> {
        labels
            .iter()
            .map(|l| Item {
                label: l.to_string(),
                hint: String::new(),
                payload: (),
            })
            .collect()
    }

    #[test]
    fn subsequence_word_starts_win() {
        let it = items(&[
            "Delete",
            "Duplicate",
            "Drop to surface",
            "Align X — min",
            "Align Y — max",
        ]);
        assert_eq!(filter(&it, "dup")[0], 1);
        assert_eq!(filter(&it, "al y")[0], 4);
        assert_eq!(filter(&it, "dts")[0], 2, "word starts");
        assert!(filter(&it, "zzz").is_empty());
        assert_eq!(filter(&it, "").len(), 5, "empty query lists everything");
    }

    #[test]
    fn every_align_variant_matches() {
        for l in ["Align Y — min", "Align Y — center", "Align Y — max"] {
            assert!(score("al y", l).is_some(), "{l}");
        }
    }

    #[test]
    fn order_matters_and_case_does_not() {
        assert!(score("SAVE", "Save").is_some());
        assert!(score("evas", "Save").is_none());
    }
}
