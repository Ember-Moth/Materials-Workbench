//! Number formatting for displayed values. Values are shown with a fixed
//! precision per quantity; locale-specific separators are not needed for the
//! decimal notation crystallography uses.

/// Cell lengths in Å, e.g. `4.594`.
pub(crate) fn length(value: f64) -> String {
    format!("{value:.3}")
}

/// Angles in degrees: whole angles without decimals, e.g. `90`, `104.50`.
pub(crate) fn angle(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// Fractional coordinates with up to four decimals and no trailing zeros,
/// e.g. `0`, `0.5`, `0.3048`.
pub(crate) fn coordinate(value: f64) -> String {
    let text = format!("{value:.4}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

pub(crate) fn occupancy(value: f64) -> String {
    format!("{value:.3}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_drop_trailing_zeros() {
        assert_eq!(coordinate(0.0), "0");
        assert_eq!(coordinate(0.5), "0.5");
        assert_eq!(coordinate(0.3048), "0.3048");
        assert_eq!(coordinate(-0.00001), "0");
    }

    #[test]
    fn whole_angles_have_no_decimals() {
        assert_eq!(angle(90.0), "90");
        assert_eq!(angle(104.5), "104.50");
    }
}
