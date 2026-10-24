//! The Python module `materials_workbench` (`docs/design/scripting.md` §5):
//! structures read from CIF and POSCAR files, queried, made into
//! supercells, described in another cell, symmetrized and written out,
//! through the same capability registry as the application, the command
//! line and MCP clients (`mw-scripting`).
//!
//! The bindings (`python.rs`) are built only with the `python` feature, so
//! that the workspace builds and tests without Python. What they decide
//! about their arguments is here, in plain Rust, and tested without an
//! interpreter.

#[cfg(feature = "python")]
mod python;

use std::path::Path;

use mw_scripting::{Format, NewCell};

/// What `Structure.supercell` takes: one repeat for every axis, three
/// repeats along a, b and c, or the matrix by rows.
#[derive(Clone, Debug, PartialEq)]
pub enum Size {
    All(i64),
    Repeats(Vec<i64>),
    Matrix(Vec<Vec<i64>>),
}

/// The supercell matrix a [`Size`] stands for.
pub fn supercell_matrix(size: &Size) -> Result<[[i32; 3]; 3], String> {
    let entry =
        |n: i64| i32::try_from(n).map_err(|_| format!("{n} is too large for a supercell matrix"));
    let repeat = |n: i64| {
        if n < 1 {
            Err(format!("a repeat is a whole number of at least 1, not {n}"))
        } else {
            entry(n)
        }
    };
    match size {
        Size::All(n) => {
            let n = repeat(*n)?;
            Ok([[n, 0, 0], [0, n, 0], [0, 0, n]])
        }
        Size::Repeats(repeats) => match repeats.as_slice() {
            &[a, b, c] => Ok([[repeat(a)?, 0, 0], [0, repeat(b)?, 0], [0, 0, repeat(c)?]]),
            _ => Err(format!(
                "three repeats are needed, one for each of a, b and c; got {}",
                repeats.len()
            )),
        },
        Size::Matrix(rows) => {
            let wrong = || "the supercell matrix has three rows of three integers".to_string();
            if rows.len() != 3 || rows.iter().any(|row| row.len() != 3) {
                return Err(wrong());
            }
            let mut matrix = [[0; 3]; 3];
            for (i, row) in rows.iter().enumerate() {
                for (j, &value) in row.iter().enumerate() {
                    matrix[i][j] = entry(value)?;
                }
            }
            Ok(matrix)
        }
    }
}

/// A number in a matrix or origin of `Structure.transform_cell`.
#[derive(Clone, Debug, PartialEq)]
pub enum Number {
    Integer(i64),
    Float(f64),
    /// A fraction such as `1/2`, as text or as `str()` of a
    /// `fractions.Fraction`.
    Text(String),
}

/// The text the registry reads as a fraction: integers and fractions as
/// they are; a float in its shortest exact form, which the registry reads
/// as the fraction it abbreviates (`0.5` is 1/2, `0.3333333333333333` is
/// 1/3) or refuses.
pub fn fraction_text(number: &Number) -> String {
    match number {
        Number::Integer(n) => n.to_string(),
        Number::Float(x) => x.to_string(),
        Number::Text(text) => text.trim().to_string(),
    }
}

/// The new cell `Structure.transform_cell` asks for: exactly one of a
/// preset `cell`, a `setting`, or a `matrix` P by rows with an optional
/// `origin` p.
pub fn new_cell(
    cell: Option<&str>,
    setting: Option<&str>,
    matrix: Option<Vec<Vec<String>>>,
    origin: Option<Vec<String>>,
) -> Result<NewCell, String> {
    let given = [cell.is_some(), setting.is_some(), matrix.is_some()]
        .iter()
        .filter(|&&given| given)
        .count();
    if given != 1 {
        return Err(
            "give one of cell (\"primitive\", \"conventional\" or \"niggli\"), setting or matrix"
                .into(),
        );
    }
    if origin.is_some() && matrix.is_none() {
        return Err("origin goes with matrix".into());
    }
    if let Some(cell) = cell {
        return NewCell::preset(cell).ok_or_else(|| {
            format!("cell is \"primitive\", \"conventional\" or \"niggli\", not {cell:?}")
        });
    }
    if let Some(setting) = setting {
        return Ok(NewCell::Setting(setting.to_string()));
    }
    let rows = matrix.expect("one was given");
    let wrong = || "matrix has three rows of three numbers".to_string();
    let rows: Vec<[String; 3]> = rows
        .into_iter()
        .map(|row| <[String; 3]>::try_from(row).map_err(|_| wrong()))
        .collect::<Result<_, _>>()?;
    let matrix = <[[String; 3]; 3]>::try_from(rows).map_err(|_| wrong())?;
    let origin = origin
        .map(|origin| <[String; 3]>::try_from(origin).map_err(|_| "origin has three numbers"))
        .transpose()?;
    Ok(NewCell::Matrix {
        matrix: Box::new(matrix),
        origin,
    })
}

/// A format by name.
pub fn format(name: &str) -> Result<Format, String> {
    Format::parse(name).ok_or_else(|| format!("{name:?} is no format; use cif, poscar or xyz"))
}

/// The format to read a file as: `format`, or else told from its name.
pub fn input_format(format: Option<&str>) -> Result<Option<Format>, String> {
    let Some(name) = format else {
        return Ok(None);
    };
    let format = self::format(name)?;
    if !format.can_read() {
        return Err(format!(
            "{} files are written, not read; use cif or poscar",
            format.title()
        ));
    }
    Ok(Some(format))
}

/// The format to write a file as: `format`, or else told from its name.
pub fn output_format(path: &Path, format: Option<&str>) -> Result<Format, String> {
    match format {
        Some(name) => self::format(name),
        None => Format::of_output(path).ok_or_else(|| {
            format!(
                "cannot tell the format from the name {}; give format=\"cif\", \"poscar\" or \
                 \"xyz\"",
                path.display()
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(
            supercell_matrix(&Size::All(2)),
            Ok([[2, 0, 0], [0, 2, 0], [0, 0, 2]])
        );
        assert_eq!(
            supercell_matrix(&Size::Repeats(vec![2, 3, 1])),
            Ok([[2, 0, 0], [0, 3, 0], [0, 0, 1]])
        );
        assert_eq!(
            supercell_matrix(&Size::Matrix(vec![
                vec![1, 1, 0],
                vec![-1, 1, 0],
                vec![0, 0, 1]
            ])),
            Ok([[1, 1, 0], [-1, 1, 0], [0, 0, 1]])
        );
        assert!(supercell_matrix(&Size::All(0)).is_err());
        assert!(supercell_matrix(&Size::Repeats(vec![2, 2])).is_err());
        assert!(supercell_matrix(&Size::Repeats(vec![2, -1, 1])).is_err());
        assert!(supercell_matrix(&Size::Matrix(vec![vec![1, 0, 0]])).is_err());
        assert!(supercell_matrix(&Size::All(1 << 40)).is_err());
    }

    #[test]
    fn fractions_are_text_the_registry_reads() {
        assert_eq!(fraction_text(&Number::Integer(-1)), "-1");
        assert_eq!(fraction_text(&Number::Float(0.5)), "0.5");
        assert_eq!(
            fraction_text(&Number::Float(1.0 / 3.0)),
            "0.3333333333333333"
        );
        assert_eq!(fraction_text(&Number::Text(" 1/2 ".into())), "1/2");
    }

    #[test]
    fn new_cells() {
        assert_eq!(
            new_cell(Some("Primitive"), None, None, None),
            Ok(NewCell::Primitive)
        );
        assert_eq!(
            new_cell(None, Some("P 1 1 21/a"), None, None),
            Ok(NewCell::Setting("P 1 1 21/a".into()))
        );
        let rows = |r: [&str; 9]| -> Vec<Vec<String>> {
            r.chunks(3)
                .map(|row| row.iter().map(|s| s.to_string()).collect())
                .collect()
        };
        let identity = rows(["1", "0", "0", "0", "1", "0", "0", "0", "1"]);
        let origin = Some(vec!["0".to_string(), "0".to_string(), "1/4".to_string()]);
        match new_cell(None, None, Some(identity.clone()), origin.clone()) {
            Ok(NewCell::Matrix { matrix, origin }) => {
                assert_eq!(matrix[1], ["0", "1", "0"]);
                assert_eq!(origin.unwrap()[2], "1/4");
            }
            other => panic!("{other:?}"),
        }
        assert!(new_cell(None, None, None, None).is_err());
        assert!(new_cell(Some("primitive"), Some("P 1"), None, None).is_err());
        assert!(new_cell(Some("primitive"), None, None, origin).is_err());
        assert!(new_cell(Some("hexagonal"), None, None, None).is_err());
        assert!(new_cell(None, None, Some(identity[..2].to_vec()), None).is_err());
    }

    #[test]
    fn formats() {
        assert_eq!(output_format(Path::new("a.cif"), None), Ok(Format::Cif));
        assert_eq!(output_format(Path::new("POSCAR"), None), Ok(Format::Poscar));
        assert_eq!(
            output_format(Path::new("a.txt"), Some("xyz")),
            Ok(Format::Xyz)
        );
        assert!(output_format(Path::new("a.txt"), None).is_err());
        assert_eq!(input_format(None), Ok(None));
        assert_eq!(input_format(Some("vasp")), Ok(Some(Format::Poscar)));
        assert!(input_format(Some("xyz")).is_err());
        assert!(format("pdb").is_err());
    }
}
