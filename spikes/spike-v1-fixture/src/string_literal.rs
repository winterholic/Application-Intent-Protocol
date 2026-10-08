#[derive(Clone, Copy)]
pub enum Flavor {
    Aip,
    Host,
}

fn four_hex(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<u32, &'static str> {
    let mut value = 0;
    for _ in 0..4 {
        let digit = chars.next().and_then(|ch| ch.to_digit(16)).ok_or("불완전한 Unicode escape")?;
        value = value * 16 + digit;
    }
    Ok(value)
}

pub fn decode_body(body: &str, flavor: Flavor, allow_newline: bool) -> Result<String, &'static str> {
    let mut chars = body.chars().peekable();
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        if ch == '\0' {
            return Err("문자열에 NUL은 허용되지 않음");
        }
        if ch == '\n' && !allow_newline {
            return Err("문자열 안 실제 줄바꿈은 허용되지 않음");
        }
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        let escaped = chars.next().ok_or("끝나지 않은 문자열 escape")?;
        match escaped {
            '"' | '\\' => out.push(escaped),
            '\'' if matches!(flavor, Flavor::Host) => out.push('\''),
            '/' if matches!(flavor, Flavor::Aip) => out.push('/'),
            'b' => out.push('\u{0008}'),
            'f' => out.push('\u{000C}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'u' => {
                let first = four_hex(&mut chars)?;
                if (0xD800..=0xDBFF).contains(&first) {
                    if matches!(flavor, Flavor::Host) {
                        return Err("호스트 Unicode escape에 surrogate는 허용되지 않음");
                    }
                    if chars.next() != Some('\\') || chars.next() != Some('u') {
                        return Err("상위 surrogate 뒤에 하위 surrogate가 필요");
                    }
                    let low = four_hex(&mut chars)?;
                    if !(0xDC00..=0xDFFF).contains(&low) {
                        return Err("유효하지 않은 하위 surrogate");
                    }
                    let scalar = 0x10000 + ((first - 0xD800) << 10) + low - 0xDC00;
                    out.push(char::from_u32(scalar).ok_or("유효하지 않은 Unicode scalar")?);
                } else if (0xDC00..=0xDFFF).contains(&first) {
                    return Err("단독 하위 surrogate는 허용되지 않음");
                } else if first == 0 {
                    return Err("문자열에 NUL은 허용되지 않음");
                } else {
                    out.push(char::from_u32(first).ok_or("유효하지 않은 Unicode scalar")?);
                }
            }
            _ => return Err("지원하지 않는 문자열 escape"),
        }
    }
    Ok(out)
}
