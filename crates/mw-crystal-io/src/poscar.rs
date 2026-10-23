//! VASP POSCAR import and export (design §9.3). Export writes VASP 5: the
//! structure expanded to P1, atoms grouped by element with an element line,
//! direct coordinates, the lattice in its orientation. Import reads VASP 5
//! and, with the elements given, VASP 4 ([`read`]).

use mw_crystal::Structure;

pub use crate::poscar_read::{read, read_str, read_with_elements};

use crate::atoms::{self, ExportError, fixed};

/// Whether the structure can be written: every site must hold one fully
/// occupied, known element.
pub fn check(structure: &Structure) -> Result<(), ExportError> {
    atoms::check(structure)
}

/// Writes a POSCAR file. `comment` goes on the first line, which VASP
/// does not read; line breaks in it become spaces.
pub fn write(structure: &Structure, comment: &str) -> Result<String, ExportError> {
    let groups = atoms::by_element(structure)?;
    let mut out = String::new();
    out.push_str(&comment.replace(['\n', '\r'], " "));
    out.push_str("\n1.0\n");
    // The basis vectors in the lattice's orientation (design §4.2), as a
    // POSCAR import gave them.
    for basis in structure.lattice.matrix() {
        for component in basis {
            out.push_str(&fixed(component, 20));
        }
        out.push('\n');
    }
    for (element, _) in &groups {
        out.push_str(&format!("{element:>5}"));
    }
    out.push('\n');
    for (_, positions) in &groups {
        out.push_str(&format!("{:>5}", positions.len()));
    }
    out.push_str("\nDirect\n");
    for (_, positions) in &groups {
        for position in positions {
            for coordinate in position {
                out.push_str(&fixed(*coordinate, 16));
            }
            out.push('\n');
        }
    }
    Ok(out)
}
