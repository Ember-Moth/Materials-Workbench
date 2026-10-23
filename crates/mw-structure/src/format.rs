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

/// A cell parameter as its field shows it: a length or an angle.
pub(crate) fn cell_param(param: mw_crystal::CellParam, value: f64) -> String {
    use mw_crystal::CellParam;
    match param {
        CellParam::A | CellParam::B | CellParam::C => length(value),
        CellParam::Alpha | CellParam::Beta | CellParam::Gamma => angle(value),
    }
}

pub(crate) fn occupancy(value: f64) -> String {
    format!("{value:.3}")
}

/// Measured distances in Å, e.g. `1.948 Å`.
pub(crate) fn distance(value: f64) -> String {
    format!("{value:.3} Å")
}

/// Measured bond angles, e.g. `90.0°`.
pub(crate) fn bond_angle(value: f64) -> String {
    format!("{value:.1}°")
}

/// A Hermann–Mauguin symbol as CIF writes it (`P 42/m n m`) in its usual
/// display form (`P4₂/mnm`): spaces dropped and screw axes subscripted. A
/// screw axis is recognized only where the symbol's spacing makes it
/// unambiguous, so `P4232` stays as written.
pub(crate) fn space_group(symbol: &str) -> String {
    fn subscript(digit: char) -> char {
        char::from_u32(0x2080 + digit.to_digit(10).unwrap_or(0)).unwrap_or(digit)
    }
    // `nm` with m < n, e.g. `42` or `4_2`, is the screw axis n subscript m.
    fn screw(axis: &str) -> Option<String> {
        let digits: Vec<char> = axis.chars().filter(|c| *c != '_').collect();
        match digits[..] {
            [n @ ('2' | '3' | '4' | '6'), m @ '1'..='5'] if m < n => {
                Some(format!("{n}{}", subscript(m)))
            }
            _ => None,
        }
    }
    let mut text = String::new();
    for (i, token) in symbol.split_whitespace().enumerate() {
        // The lattice letter may be written together with the first axis.
        let (lattice, token) = match token.chars().next() {
            Some(c @ ('P' | 'A' | 'B' | 'C' | 'F' | 'I' | 'R')) if i == 0 && token.len() > 1 => {
                (Some(c), &token[1..])
            }
            _ => (None, token),
        };
        text.extend(lattice);
        let (axis, rest) = token.split_at(token.find('/').unwrap_or(token.len()));
        match screw(axis) {
            Some(axis) => text.push_str(&axis),
            None => text.push_str(axis),
        }
        text.push_str(rest);
    }
    text
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
    fn space_group_symbols() {
        assert_eq!(space_group("P 42/m n m"), "P4₂/mnm");
        assert_eq!(space_group("P42/mnm"), "P4₂/mnm");
        assert_eq!(space_group("P 4_2/m n m"), "P4₂/mnm");
        assert_eq!(space_group("P m -3 m"), "Pm-3m");
        assert_eq!(space_group("P 63 m c"), "P6₃mc");
        assert_eq!(space_group("P 1 21/c 1"), "P12₁/c1");
        assert_eq!(space_group("P 21 21 21"), "P2₁2₁2₁");
        assert_eq!(space_group("R -3 c :H"), "R-3c:H");
        assert_eq!(space_group("P 4 2 2"), "P422");
        assert_eq!(space_group("P4232"), "P4232");
        assert_eq!(space_group("P 1"), "P1");
    }

    #[test]
    fn whole_angles_have_no_decimals() {
        assert_eq!(angle(90.0), "90");
        assert_eq!(angle(104.5), "104.50");
    }
}
