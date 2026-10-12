//! Help for diagnostics whose emitting site said only what is wrong.
//!
//! The registry (`aip_ir::codes`) knows how every code is repaired and the source knows which names exist, so
//! the checker can answer a typo with the close spelling and an error with its repair in the same output,
//! instead of asking the reader to run `aip explain-code` first. A help text a site wrote is never replaced.
//! The product definition checker has the same step (`spikes/spike-v1-fixture/src/enrich.rs`); the two cannot
//! share code because the spike crates are built apart from this workspace.

use aip_ir::codes;
use aip_syntax::Diagnostic;
use std::collections::{BTreeMap, BTreeSet};

/// Codes whose message names a thing that does not exist.
///
/// Written as strings, not as constants of the registry: `aip-ir/tests/code_sites.snap` pins where a code is
/// emitted, and this file only refers to codes.
const UNKNOWN_NAME_CODES: &[&str] = &["AIP-E102", "AIP-E103", "AIP-E104"];

pub fn add(diags: &mut [Diagnostic], src: &str) {
    let words = words(src);
    let mut told: BTreeSet<String> = BTreeSet::new();
    for d in diags.iter_mut() {
        if d.help.is_some() {
            continue;
        }
        let mut parts: Vec<String> = Vec::new();
        let near = close_to_quoted(&d.message, &words).filter(|n| !n.is_empty() && UNKNOWN_NAME_CODES.contains(&d.code.as_str()));
        if let Some(near) = near {
            parts.push(format!("did you mean {}?", near.iter().map(|n| format!("'{n}'")).collect::<Vec<_>>().join(" or ")));
        }
        // The repair of a code is stated once per answer.
        if let Some(info) = codes::lookup(&d.code).filter(|_| told.insert(d.code.clone())) {
            parts.push(format!("fix: {}", info.fix));
        }
        if !parts.is_empty() {
            d.help = Some(parts.join(" "));
        }
    }
}

/// Every identifier of the source with the number of times it occurs.
fn words(src: &str) -> BTreeMap<String, usize> {
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    let mut word = String::new();
    for c in src.chars().chain(std::iter::once(' ')) {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            if word.len() >= 2 && !word.starts_with(|c: char| c.is_ascii_digit()) {
                *out.entry(std::mem::take(&mut word)).or_default() += 1;
            }
            word.clear();
        }
    }
    out
}

/// The quoted word of the message that the source uses least is the misspelled one: a real name occurs at its
/// declaration and at its uses, a typo once or twice. Returns the words within a small edit distance of it.
fn close_to_quoted(msg: &str, words: &BTreeMap<String, usize>) -> Option<Vec<String>> {
    let bad = msg.split('\'').skip(1).step_by(2).filter(|t| !t.is_empty()).min_by_key(|t| words.get(*t).copied().unwrap_or(0))?;
    if bad.len() < 3 {
        return None;
    }
    let max = if bad.len() >= 5 { 2 } else { 1 };
    let mut scored: Vec<(usize, &String)> =
        words.keys().filter(|w| w.as_str() != bad).map(|w| (distance(bad, w), w)).filter(|(d, _)| *d <= max).collect();
    scored.sort();
    Some(scored.into_iter().take(3).map(|(_, w)| w.clone()).collect())
}

/// Edit distance in which swapping two neighbours is one edit: the usual slip of a typist or a language model.
fn distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}
