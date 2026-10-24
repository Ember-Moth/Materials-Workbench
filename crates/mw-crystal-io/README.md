# mw-crystal-io

Reading and writing crystal structure files for
[mw-crystal](https://crates.io/crates/mw-crystal), the structure model of
[Materials Workbench](https://github.com/Ember-Moth/Materials-Workbench).
It has no user interface and can be used on its own.

- **CIF 1.1** (a subset: cell, symmetry operations or space group name,
  atom sites with occupancies, displacement parameters and standard
  uncertainties): one structure per data block, each with diagnostics tied
  to source lines. Structures come out normalized: sites snapped onto
  special positions, mixed occupancy merged into one site, repeated rows
  dropped. Fields the model does not hold are kept and written back on
  export. On the 504,373 files of the Crystallography Open Database up to
  1 MiB, 0.031 % fail to import (mostly modulated structures given only a
  superspace group), and no file makes the reader panic.
- **VASP POSCAR and CONTCAR** (VASP 5, and VASP 4 with the elements given):
  a P1 structure whose lattice keeps the orientation of the file's vectors,
  left-handed ones too.
- **Export** to CIF, POSCAR and extended XYZ. POSCAR and XYZ list atoms one
  by one, so they refuse structures with partially occupied sites.

## Example

```rust
use mw_crystal_io::{cif, poscar};

let text = "\
data_rutile
_space_group_name_H-M_alt 'P 42/m n m'
_cell_length_a 4.594
_cell_length_b 4.594
_cell_length_c 2.959
_cell_angle_alpha 90
_cell_angle_beta 90
_cell_angle_gamma 90
loop_
_atom_site_label
_atom_site_type_symbol
_atom_site_fract_x
_atom_site_fract_y
_atom_site_fract_z
Ti1 Ti 0 0 0
O1 O 0.3048 0.3048 0
";
let report = cif::read_str(text);
for diagnostic in report.all_diagnostics() {
    println!("line {:?}: {:?}", diagnostic.line(), diagnostic.kind());
}
let (block, rutile) = report.structures().next().unwrap();
assert_eq!(block.name, "rutile");
assert_eq!(rutile.symmetry.operations().len(), 16);

// POSCAR lists the 6 atoms of the cell, grouped by element.
let vasp = poscar::write(rutile, "rutile").unwrap();
let back = poscar::read_str(&vasp);
let (_, p1) = back.structures().next().unwrap();
assert_eq!(p1.sites.len(), 6);
```

## Design

The supported subset, the normalization and the diagnostics are written up
(in Chinese) in §9 of
[`docs/design/crystallography-model.md`](https://github.com/Ember-Moth/Materials-Workbench/blob/main/docs/design/crystallography-model.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in this crate by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
