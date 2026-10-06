use spike_v1_fixture::lexer::{lex, Tok};

#[test]
fn an_out_of_range_integer_is_a_positioned_diagnostic() {
    for number in ["9223372036854775808", "999999999999999999999999999999999999"] {
        let error = lex(number, 17, 9).expect_err("integer overflow is a definition error");
        assert_eq!(error.code, "LEX_NUMBER_RANGE");
        assert_eq!((error.span.line, error.span.col), (17, 9));
    }
}

#[test]
fn duration_conversion_checks_millisecond_overflow() {
    for literal in ["18446744073709552s", "307445734561826m", "9223372036854775807s", "9223372036854775807m"] {
        let error = lex(literal, 23, 4).expect_err("duration multiplication cannot wrap");
        assert_eq!(error.code, "LEX_DURATION_RANGE", "{literal}");
        assert_eq!((error.span.line, error.span.col), (23, 4));
    }
}

#[test]
fn the_existing_maximum_values_units_and_following_spans_are_preserved() {
    let tokens = lex("9223372036854775807 9223372036854775807ms 18446744073709551s 307445734561825m\n1s", 3, 5).unwrap();
    assert_eq!(tokens[0].tok, Tok::Int(i64::MAX));
    assert_eq!(tokens[1].tok, Tok::Dur(i64::MAX as u64));
    assert_eq!(tokens[2].tok, Tok::Dur(18_446_744_073_709_551_000));
    assert_eq!(tokens[3].tok, Tok::Dur(18_446_744_073_709_500_000));
    assert_eq!(tokens[4].tok, Tok::Dur(1000));
    assert_eq!((tokens[4].span.line, tokens[4].span.col), (4, 1));
    let error = lex("1h", 2, 8).unwrap_err();
    assert_eq!(error.code, "LEX_BAD_DURATION");
}
