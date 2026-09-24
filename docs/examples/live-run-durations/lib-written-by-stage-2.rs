/// Parses durations like "90s", "5m", "2h", "1h30m" into seconds.
/// Returns None for empty or malformed input.
pub fn parse_duration(s: &str) -> Option<u64> {
    const UNITS: [(u8, u64, i32); 3] = [(b'h', 3600, 0), (b'm', 60, 1), (b's', 1, 2)];

    let bytes = s.as_bytes();
    let mut i = 0;
    let mut total: u64 = 0;
    let mut last_unit: i32 = -1;

    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
        let value: u64 = s[start..i].parse().ok()?;

        let unit_byte = *bytes.get(i)?;
        let (_, multiplier, unit_index) = UNITS.iter().find(|(u, _, _)| *u == unit_byte)?;
        if *unit_index <= last_unit {
            return None;
        }
        last_unit = *unit_index;
        i += 1;

        total = total.checked_add(value.checked_mul(*multiplier)?)?;
    }

    if last_unit == -1 {
        None
    } else {
        Some(total)
    }
}
