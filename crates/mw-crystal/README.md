# mw-crystal

The crystal structure model of [Materials Workbench](https://github.com/Ember-Moth/Materials-Workbench),
a desktop workbench for materials science. It has no user interface and can
be used on its own, from a command line tool, a test or a notebook binding.

- **Model**: a structure is the asymmetric unit (sites with their occupants,
  occupancies and displacement parameters) plus a closed set of symmetry
  operations, each an integer matrix with a rational translation. The lattice
  is the cell parameters plus an orientation, so files that give basis
  vectors (such as VASP's POSCAR) keep their axes, left-handed ones too.
- **Derived data**: equivalent atoms, the space group type and setting
  (through [moyo](https://crates.io/crates/moyo)), every site's site symmetry,
  coordinate constraints and Wyckoff position, the cell's constraints, bonds
  across periodic boundaries, the chemical formula, density and coordination
  polyhedra.
- **Editing**: serializable commands on whole sites, each with an exact
  inverse, kept in an undo history (`CrystalDocument`). Under symmetry,
  moved and added sites keep to their special positions; edits that would
  break the symmetry are refused with guidance.
- **Cells**: supercells, changes of cell (P, p) with the primitive,
  conventional, Niggli and other tabulated settings as presets, and
  symmetrizing a P1 structure by the symmetry its atoms have.

Reading and writing CIF, POSCAR and XYZ files is in
[mw-crystal-io](https://crates.io/crates/mw-crystal-io).

## Example

Rutile from its cell, three generators of P4₂/mnm and two sites:

```rust
use mw_crystal::{
    CellParams, Derived, Formula, Lattice, Measured, Occupant, Site, SiteId, Species, Structure,
    SymOp, Symmetry, make_supercell,
};

let generators = ["-y+1/2,x+1/2,z+1/2", "-x+1/2,y+1/2,-z+1/2", "-x,-y,-z"]
    .map(|op| SymOp::parse(op).unwrap());
let site = |id, label: &str, element: &str, position| {
    let occupant = Occupant::new(label, Species::element(element), Measured::exact(1.0));
    Site::new(SiteId(id), position, vec![occupant])
};
let rutile = Structure::new(
    Lattice::new(CellParams::new(4.594, 4.594, 2.959, 90.0, 90.0, 90.0)),
    Symmetry::generate(&generators).unwrap(),
    vec![
        site(1, "Ti1", "Ti", [0.0, 0.0, 0.0]),
        site(2, "O1", "O", [0.3048, 0.3048, 0.0]),
    ],
);

let derived = Derived::compute(&rutile);
let space_group = derived.symmetry.space_group.as_ref().unwrap();
assert_eq!(space_group.number, 136);
assert_eq!(derived.atoms_in_cell(), 6);
let oxygen = derived.site_symmetry(SiteId(2)).unwrap();
assert_eq!(oxygen.wyckoff.as_ref().unwrap().to_string(), "4f");
// Hill order: alphabetical when there is no carbon.
assert_eq!(Formula::of(&rutile, &derived).to_string(), "O2Ti");

// A 2 × 2 × 2 supercell is a new structure in P1, one site per atom.
let supercell = make_supercell(&rutile, [[2, 0, 0], [0, 2, 0], [0, 0, 2]]).unwrap();
assert_eq!(supercell.sites.len(), 48);
```

## Design

The data model and the editing semantics are written up (in Chinese) in
[`docs/design/crystallography-model.md`](https://github.com/Ember-Moth/Materials-Workbench/blob/main/docs/design/crystallography-model.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this crate by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
