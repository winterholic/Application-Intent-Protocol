/// `ClubMember` -> `club_member`, `createdAt` -> `created_at`.
pub fn snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() {
            let prev_lower = i > 0 && (chars[i - 1].is_ascii_lowercase() || chars[i - 1].is_ascii_digit());
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            let prev_upper = i > 0 && chars[i - 1].is_ascii_uppercase();
            if i > 0 && (prev_lower || (prev_upper && next_lower)) {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(*c);
        }
    }
    out
}

/// Double-quoted SQL identifier.
pub fn q(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

pub fn lit(s: &str) -> String {
    // Trigger and function bodies are wrapped in `$aip$ ... $aip$`. A literal containing `$aip$` would end that quote early
    // and let the rest of the text run as DDL, so a `$` is emitted as a Unicode escape that stays one literal token.
    if s.contains('$') {
        return format!("U&'{}'", s.replace('\\', "\\\\").replace('\'', "''").replace('$', "\\0024"));
    }
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::snake;

    #[test]
    fn dollar_signs_cannot_close_a_dollar_quoted_body() {
        let literal = super::lit("T-$aip$; CREATE TABLE pwned(x int); SELECT $aip$-{n}");
        assert!(!literal.contains('$'), "{literal}");
        assert!(literal.starts_with("U&'"), "{literal}");
        assert_eq!(super::lit("it's"), "'it''s'");
        assert_eq!(super::lit("a\\b$"), "U&'a\\\\b\\0024'");
    }

    #[test]
    fn snake_case() {
        assert_eq!(snake("ClubMember"), "club_member");
        assert_eq!(snake("createdAt"), "created_at");
        assert_eq!(snake("URLValue"), "url_value");
        assert_eq!(snake("school"), "school");
    }
}
