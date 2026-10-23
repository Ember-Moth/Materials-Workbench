//! CIF numbers with an optional standard uncertainty in parentheses, e.g.
//! `3.905(2)` = 3.905 ± 0.002.

use mw_crystal::Measured;

/// Parses a CIF number. Returns `None` for anything that is not one,
/// including the special values `?` and `.`. The closing parenthesis may be
/// missing: files whose lines were cut at a fixed width end rows in
/// `0.0029(18`.
pub(crate) fn parse(text: &str) -> Option<Measured> {
    let (number, su) = match text.split_once('(') {
        Some((number, rest)) => (number, Some(rest.strip_suffix(')').unwrap_or(rest))),
        None => (text, None),
    };
    // f64's parser also accepts `inf` and `NaN`; CIF numbers are digits.
    if number.is_empty()
        || !number
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E'))
        || !number.chars().any(|c| c.is_ascii_digit())
    {
        return None;
    }
    let value: f64 = number.parse().ok()?;
    let Some(su) = su else {
        return Some(Measured::exact(value));
    };
    if su.is_empty() || !su.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // The uncertainty applies to the last digits written: `3.905(2)` has
    // three decimals and so ±0.002; an exponent scales it like the value.
    let (mantissa, exponent) = match number.find(['e', 'E']) {
        Some(i) => (&number[..i], number[i + 1..].parse::<i32>().ok()?),
        None => (number, 0),
    };
    let decimals = mantissa
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len() as i32);
    let digits: f64 = su.parse().ok()?;
    Some(Measured::with_su(
        value,
        digits * 10f64.powi(exponent - decimals),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn su(text: &str) -> Option<f64> {
        parse(text).and_then(|m| m.su())
    }

    #[test]
    fn values_and_uncertainties() {
        assert_eq!(parse("3.905").map(|m| m.value()), Some(3.905));
        assert_eq!(parse("3.905").and_then(|m| m.su()), None);
        assert_eq!(parse("3.905(2)").map(|m| m.value()), Some(3.905));
        assert!((su("3.905(2)").unwrap() - 0.002).abs() < 1e-15);
        assert!((su("0.30478(12)").unwrap() - 0.00012).abs() < 1e-15);
        assert_eq!(su("120(30)"), Some(30.0));
        assert!((su("1.23e-4(5)").unwrap() - 5e-6).abs() < 1e-18);
        assert_eq!(parse("-.5").map(|m| m.value()), Some(-0.5));
        assert_eq!(parse("+2.").map(|m| m.value()), Some(2.0));
        assert!((su("0.0029(18").unwrap() - 0.0018).abs() < 1e-15);
    }

    #[test]
    fn rejects_non_numbers() {
        for text in [
            "?",
            ".",
            "",
            "inf",
            "NaN",
            "1.0(",
            "1.0()",
            "1.0(x)",
            "1.0(2)3",
            "0..049(4)",
            "x1",
            "1,5",
            "-",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
