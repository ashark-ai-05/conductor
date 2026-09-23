use durations::parse_duration;

// --- Normal, single-unit cases ---

#[test]
fn seconds_only() {
    assert_eq!(parse_duration("90s"), Some(90));
}

#[test]
fn minutes_only() {
    assert_eq!(parse_duration("5m"), Some(5 * 60));
}

#[test]
fn hours_only() {
    assert_eq!(parse_duration("2h"), Some(2 * 3600));
}

#[test]
fn single_digit_seconds() {
    assert_eq!(parse_duration("1s"), Some(1));
}

#[test]
fn leading_zeros_in_number() {
    assert_eq!(parse_duration("005s"), Some(5));
}

// --- Combinations (must appear in h, m, s order) ---

#[test]
fn hours_minutes() {
    assert_eq!(parse_duration("1h30m"), Some(3600 + 30 * 60));
}

#[test]
fn hours_seconds() {
    assert_eq!(parse_duration("1h10s"), Some(3600 + 10));
}

#[test]
fn minutes_seconds() {
    assert_eq!(parse_duration("30m10s"), Some(30 * 60 + 10));
}

#[test]
fn hours_minutes_seconds() {
    assert_eq!(parse_duration("1h30m10s"), Some(3600 + 30 * 60 + 10));
}

#[test]
fn all_units_larger_values() {
    assert_eq!(
        parse_duration("12h45m30s"),
        Some(12 * 3600 + 45 * 60 + 30)
    );
}

// --- Boundaries ---

#[test]
fn zero_seconds() {
    assert_eq!(parse_duration("0s"), Some(0));
}

#[test]
fn zero_all_units() {
    assert_eq!(parse_duration("0h0m0s"), Some(0));
}

#[test]
fn zero_hours_nonzero_rest() {
    assert_eq!(parse_duration("0h0m1s"), Some(1));
}

#[test]
fn large_but_non_overflowing_value() {
    assert_eq!(parse_duration("1000h"), Some(1000 * 3600));
}

#[test]
fn overflow_returns_none() {
    // u64::MAX hours, multiplied by 3600, overflows u64.
    assert_eq!(parse_duration("18446744073709551615h"), None);
}

#[test]
fn number_too_large_to_fit_returns_none() {
    // Far beyond u64::MAX digits, can't even parse as an integer.
    assert_eq!(
        parse_duration("999999999999999999999999999999s"),
        None
    );
}

// --- Malformed input ---

#[test]
fn empty_string_is_none() {
    assert_eq!(parse_duration(""), None);
}

#[test]
fn whitespace_only_is_none() {
    assert_eq!(parse_duration(" "), None);
}

#[test]
fn missing_unit_is_none() {
    assert_eq!(parse_duration("10"), None);
}

#[test]
fn unknown_unit_is_none() {
    assert_eq!(parse_duration("10x"), None);
}

#[test]
fn unit_without_number_is_none() {
    assert_eq!(parse_duration("h"), None);
}

#[test]
fn unit_before_number_is_none() {
    assert_eq!(parse_duration("h5"), None);
}

#[test]
fn negative_value_is_none() {
    assert_eq!(parse_duration("-5s"), None);
}

#[test]
fn plus_signed_value_is_none() {
    assert_eq!(parse_duration("+5s"), None);
}

#[test]
fn fractional_value_is_none() {
    assert_eq!(parse_duration("1.5h"), None);
}

#[test]
fn duplicate_unit_is_none() {
    assert_eq!(parse_duration("1h1h"), None);
}

#[test]
fn duplicate_minutes_is_none() {
    assert_eq!(parse_duration("5m5m"), None);
}

#[test]
fn wrong_order_minutes_before_hours_is_none() {
    assert_eq!(parse_duration("30m1h"), None);
}

#[test]
fn wrong_order_seconds_before_hours_is_none() {
    assert_eq!(parse_duration("10s1h"), None);
}

#[test]
fn wrong_order_seconds_before_minutes_is_none() {
    assert_eq!(parse_duration("10s30m"), None);
}

#[test]
fn trailing_number_without_unit_is_none() {
    assert_eq!(parse_duration("5h5"), None);
}

#[test]
fn trailing_garbage_is_none() {
    assert_eq!(parse_duration("5s abc"), None);
}

#[test]
fn leading_whitespace_is_none() {
    assert_eq!(parse_duration(" 5s"), None);
}

#[test]
fn trailing_whitespace_is_none() {
    assert_eq!(parse_duration("5s "), None);
}

#[test]
fn uppercase_unit_is_none() {
    assert_eq!(parse_duration("5S"), None);
}

#[test]
fn bare_unit_letters_only_is_none() {
    assert_eq!(parse_duration("hms"), None);
}
