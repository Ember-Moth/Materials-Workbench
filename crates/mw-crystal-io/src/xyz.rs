//! Extended XYZ export (design §9.4): the structure expanded to P1,
//! Cartesian coordinates in Å, the cell in `Lattice=` and the columns in
//! `Properties=`, as ASE and OVITO read them.

use mw_crystal::Structure;

use crate::atoms::{self, ExportError, fixed};

/// Whether the structure can be written: every site must hold one fully
/// occupied, known element.
pub fn check(structure: &Structure) -> Result<(), ExportError> {
    atoms::check(structure)
}

/// Writes an extended XYZ file with one frame.
pub fn write(structure: &Structure) -> Result<String, ExportError> {
    let groups = atoms::by_element(structure)?;
    let count: usize = groups.iter().map(|(_, positions)| positions.len()).sum();
    let lattice: Vec<String> = structure
        .lattice
        .params()
        .matrix()
        .iter()
        .flatten()
        .map(|v| fixed(*v, 0))
        .collect();
    let mut out = format!(
        "{count}\nLattice=\"{}\" Properties=species:S:1:pos:R:3 pbc=\"T T T\"\n",
        lattice.join(" ")
    );
    for (element, positions) in &groups {
        for position in positions {
            out.push_str(&format!("{element:<2}"));
            for coordinate in structure.lattice.to_cartesian(*position) {
                out.push_str(&fixed(coordinate, 16));
            }
            out.push('\n');
        }
    }
    Ok(out)
}
