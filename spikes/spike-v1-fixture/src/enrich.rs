//! Turns the diagnostics of one failed load into something a caller can act on without reading the
//! compiler: root causes first, the declaration each error is in, close spellings, the keys a block
//! accepts and the repair from the code registry. The emitting sites only say what is wrong and where.
//! Nothing here changes whether a definition is accepted.

use crate::codes::{self, BLOCK_KEYS};
use crate::diag::Diag;
use crate::Form;
use std::collections::{BTreeMap, BTreeSet};

const TYPES: &[&str] = &["Id", "Text", "Int", "Bool", "Time", "Date", "Url", "Email", "Decimal"];
const OPERATORS: &[&str] = &["eq", "in", "isNull", "gte", "lte", "gt", "lt", "prefix", "contains", "like"];
/// First words of lines that are a declaration on their own (no block), so a diagnostic on them
/// names that declaration even though no brace was opened.
const LINE_DECLS: &[&str] =
    &["predicate", "access", "limit", "enum", "actor", "field", "rows", "unique", "check", "invariant", "select", "filter", "sort"];

/// Errors that are often a consequence of a declaration-level error.
const CASCADING: &[&str] = &["DEF.UNRESOLVED_NAME", "DEF.UNKNOWN_SYMBOL", "DEF.UNKNOWN_FIELD", "DEF.TYPE_MISMATCH", "DEF.UNKNOWN_TYPE"];

/// Declaration-level errors; the errors that follow from them are reported after them.
fn is_root(code: &str) -> bool {
    code.starts_with("PARSE_")
        || code.starts_with("LEX_")
        || matches!(code, "DEF.DUPLICATE" | "DEF.UNKNOWN_TYPE" | "DEF.UNKNOWN_RESOURCE" | "DEF.AMBIGUOUS" | "DEF.BAD_DECIMAL_TYPE")
}

pub fn enrich(mut diags: Vec<Diag>, src: &str, form: Form) -> Vec<Diag> {
    // Stable: the order inside the two groups stays the order of discovery.
    diags.sort_by_key(|d| !is_root(d.code));
    let scopes = matches!(form, Form::A | Form::ETs | Form::EPy).then(|| line_scopes(src));
    let universe = universe(src);
    let first_root: Option<(String, u32)> = diags.iter().find(|d| is_root(d.code)).map(|d| (d.code.to_string(), d.span.line));
    let mut told: BTreeSet<&'static str> = BTreeSet::new();
    // A PoC program saved with the product extension `.aip` fails with errors that look like typos. Say so once,
    // on the first error. (A file with the extension `.aipoc` is refused before it is read: `wrong_language`.)
    if let Some(first) = diags.first_mut().filter(|_| looks_like_poc_program(src)) {
        let note = "language: this file looks like a PoC program (top-level entity/command/query/use), whose checker is `aip check`; rename it to `.aipoc`. \
                    `aip service` reads the product definition language (resource, expose, transition; see spikes/spike-v1-fixture/fixture/recruitment.aip)."
            .to_string();
        first.set_help(Some(note));
    }
    for d in &mut diags {
        if let Some(scopes) = &scopes {
            let scope = scope_of(scopes, src, d);
            d.set_scope(scope);
        }
        let mut parts: Vec<String> = Vec::new();
        if let Some(h) = d.help() {
            parts.push(h.to_string());
        }
        parts.extend(suggestions(d, &universe));
        let cascade = match &first_root {
            Some((code, line)) if !is_root(d.code) && CASCADING.contains(&d.code) => {
                parts.push(format!("cascade: this may follow from {code} at line {line}; fix that first and check again."));
                true
            }
            _ => false,
        };
        // The repair of a code is stated once per answer; the same code later in the list only repeats it.
        if let Some(info) = codes::lookup(d.code).filter(|_| !cascade && told.insert(d.code)) {
            parts.push(format!("fix: {}", info.fix));
        }
        if !parts.is_empty() {
            d.set_help(Some(parts.join(" ")));
        }
    }
    diags
}

/// The PoC language declares `entity`, `command`, `query` and `use` at the top level; the product language has
/// none of these words as declarations, so one such line at column 0 is enough.
fn looks_like_poc_program(src: &str) -> bool {
    src.lines().any(|l| ["entity ", "command ", "query ", "use "].iter().any(|k| l.starts_with(k)))
}

/// The words a spelling can be compared with: every identifier of the definition, the keys of every
/// block, the built-in types and the filter operators.
fn universe(src: &str) -> BTreeMap<String, usize> {
    let mut set: BTreeMap<String, usize> = BTreeMap::new();
    let mut word = String::new();
    for c in src.chars().chain(std::iter::once(' ')) {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
        } else {
            if word.len() >= 2 && !word.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                *set.entry(std::mem::take(&mut word)).or_default() += 1;
            }
            word.clear();
        }
    }
    // Words the language itself defines count as used often, so they are never taken for a typo.
    for (_, keys) in BLOCK_KEYS {
        for k in *keys {
            *set.entry(k.to_string()).or_default() += 2;
        }
    }
    for w in TYPES.iter().chain(OPERATORS) {
        *set.entry(w.to_string()).or_default() += 2;
    }
    set
}

fn backticked(msg: &str) -> Vec<&str> {
    msg.split('`').skip(1).step_by(2).collect()
}

fn suggestions(d: &Diag, universe: &BTreeMap<String, usize>) -> Vec<String> {
    let mut out = Vec::new();
    match d.code {
        "DEF.PARSE_UNKNOWN_KEY" => {
            let block = d.msg.split(" 안 ").next().unwrap_or("");
            if let Some(keys) = codes::keys_of(block) {
                let bad = backticked(&d.msg).first().copied().unwrap_or("");
                let near = close(bad, keys.iter().map(|k| k.to_string()));
                if !near.is_empty() {
                    out.push(format!("did you mean {}?", ticks(&near)));
                }
                out.push(format!("`{block}` accepts: {}.", keys.join(", ")));
            }
        }
        "DEF.PARSE_EXPECTED" => {
            // A syntax error often sits at a key of the block being read: name the keys that block has.
            let inner = d.scope().and_then(|s| s.rsplit(" > ").next()).unwrap_or("");
            let block = BLOCK_KEYS.iter().filter(|(b, _)| inner == *b || inner.starts_with(&format!("{b} "))).max_by_key(|(b, _)| b.len());
            if let Some((block, keys)) = block {
                out.push(format!("`{block}` accepts: {}.", keys.join(", ")));
            }
        }
        "DEF.PARSE_UNKNOWN_DECL" => {
            if let Some(keys) = codes::keys_of("선언") {
                let bad = backticked(&d.msg).first().copied().unwrap_or("");
                let near = close(bad, keys.iter().map(|k| k.to_string()));
                if !near.is_empty() {
                    out.push(format!("did you mean {}?", ticks(&near)));
                }
                out.push(format!("a declaration starts with one of: {}.", keys.join(", ")));
            }
        }
        "DEF.UNKNOWN_OPERATOR" => {
            let bad = backticked(&d.msg).first().copied().unwrap_or("");
            let near = close(bad, OPERATORS.iter().map(|s| s.to_string()));
            if !near.is_empty() {
                out.push(format!("did you mean {}?", ticks(&near)));
            }
            out.push(format!("operators: {}.", OPERATORS.join(", ")));
        }
        "DEF.UNKNOWN_FIELD" | "DEF.UNKNOWN_SYMBOL" | "DEF.UNRESOLVED_NAME" | "DEF.UNKNOWN_TYPE" | "DEF.UNKNOWN_RESOURCE" | "DEF.UNKNOWN_KEY" => {
            // The misspelled word is the quoted word the definition uses least: a real name occurs
            // at its declaration and at its uses, a typo once or twice.
            let bad = backticked(&d.msg)
                .into_iter()
                .flat_map(|t| t.split('.'))
                .filter(|t| !t.is_empty())
                .min_by_key(|t| universe.get(*t).copied().unwrap_or(0));
            if let Some(bad) = bad {
                let upper = d.code == "DEF.UNKNOWN_TYPE";
                let pool = universe.iter().filter(|(w, _)| !upper || w.chars().next().is_some_and(char::is_uppercase)).map(|(w, _)| w.clone());
                let near = close(bad, pool);
                if !near.is_empty() {
                    out.push(format!("did you mean {}?", ticks(&near)));
                }
            }
        }
        _ => {}
    }
    out
}

fn ticks(names: &[String]) -> String {
    names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(" or ")
}

/// At most three words within a small edit distance of `bad`, nearest first. Short words allow one
/// edit so that unrelated short names are not offered.
fn close(bad: &str, pool: impl Iterator<Item = String>) -> Vec<String> {
    if bad.len() < 3 {
        return Vec::new();
    }
    let max = if bad.len() >= 5 { 2 } else { 1 };
    let mut scored: Vec<(usize, String)> = pool.filter(|w| w != bad).map(|w| (distance(bad, &w), w)).filter(|(d, _)| *d <= max).collect();
    scored.sort();
    scored.dedup();
    scored.into_iter().take(3).map(|(_, w)| w).collect()
}

/// Edit distance with a transposition of two neighbours counted as one edit, the usual slip of a typist
/// or a language model (`titel` for `title`).
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

/// Applies the braces of `code` to `stack`: a brace opens a block labelled by the words since the
/// previous brace or `;`, and closes the innermost one. Braces inside strings do not count.
fn advance(stack: &mut Vec<String>, code: &str) {
    let (mut in_str, mut seg_start) = (false, 0usize);
    for (i, c) in code.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '{' if !in_str => {
                stack.push(label(&code[seg_start..i]));
                seg_start = i + 1;
            }
            '}' if !in_str => {
                stack.pop();
                seg_start = i + 1;
            }
            ';' if !in_str => seg_start = i + 1,
            _ => {}
        }
    }
}

fn strip_comment(line: &str) -> &str {
    line.split("//").next().unwrap_or("")
}

/// For each line (index 0 is line 1): the labels of the blocks that are open when the line starts.
fn line_scopes(src: &str) -> Vec<Vec<String>> {
    let mut stack: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for line in src.lines() {
        out.push(stack.clone());
        advance(&mut stack, strip_comment(line));
    }
    out
}

/// `resource Recruitment`, `expose read`, `aggregate bookmarkCount`: the words before the brace up to
/// the first character that starts a type, a parameter list or a condition.
fn label(header: &str) -> String {
    let head = header.split(['(', ':', '=']).next().unwrap_or("");
    let mut words: Vec<&str> = Vec::new();
    for w in head.split_whitespace() {
        if w == "when" {
            break;
        }
        words.push(w);
        if words.len() == 3 {
            break;
        }
    }
    words.join(" ")
}

fn scope_of(scopes: &[Vec<String>], src: &str, d: &Diag) -> Option<String> {
    let idx = (d.span.line as usize).checked_sub(1)?;
    let mut stack = scopes.get(idx)?.clone();
    if let Some(line) = src.lines().nth(idx) {
        let code = strip_comment(line);
        let col = d.span.col.saturating_sub(1) as usize;
        let at = code.char_indices().nth(col).map_or(code.len(), |(b, _)| b);
        advance(&mut stack, &code[..at]);
        let trimmed = code.trim_start();
        let indent = code.len() - trimmed.len();
        let first = trimmed.split(|c: char| !c.is_alphanumeric() && c != '_').next().unwrap_or("");
        match code.find('{') {
            // On the header of a block, left of its brace: the block itself is part of the path.
            Some(b) if at < b && at >= indent => stack.push(label(&code[indent..b])),
            // A declaration without a block: its own first two words.
            None if LINE_DECLS.contains(&first) && at >= indent => {
                // `select a, b` and its kin are lists: the keyword alone names them.
                let words = if matches!(first, "select" | "filter" | "sort") { 1 } else { 2 };
                stack.push(label(trimmed).split_whitespace().take(words).collect::<Vec<_>>().join(" "));
            }
            _ => {}
        }
    }
    let path: Vec<String> = stack.into_iter().filter(|l| !l.is_empty()).collect();
    (!path.is_empty()).then(|| path.join(" > "))
}
