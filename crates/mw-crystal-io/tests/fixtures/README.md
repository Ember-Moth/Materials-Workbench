# CIF test fixtures

All files here are written by hand for the tests in `../fixtures.rs`. Cell
parameters and coordinates are commonly cited literature values or made-up
values typed in directly; no file is copied from COD or another database, so
there is no third-party license to track. The files modeled on problems the
COD corpus statistics found (`tests/cod_corpus.rs`) name the COD entries
whose layout they imitate; COD data is in the public domain.

| File | What it exercises |
|---|---|
| `rutile.cif` | Current field names, 16 listed operations; must match the built-in rutile sample |
| `SrTiO3.cif` | Pm-3m with 48 quoted operations |
| `zincite_legacy.cif` | Legacy `_symmetry_*` names, decimal translations (`z+0.5`), Biso, elements given only by labels |
| `bst_su_aniso.cif` | Standard uncertainties, an anisotropic ADP loop, Ba/Sr mixed occupancy on one position |
| `malformed.cif` | Syntax and content errors at known lines |
| `multi_block.cif` | A publication block, a P1 structure without operations, and a block whose operations come from its H-M symbol |
| `halite_p1.cif` | NaCl written out in P1; the symmetry search must find F m -3 m |
| `rutile_near_special.cif` | Operations from a Hall symbol, sites a few mÅ off their special positions, mixed occupancy as two rows, a repeated row |
| `shelxl_multiplicity.cif` | Multiplicities as SHELXL-97 (site-symmetry order under the multiplicity tag), SHELXL-2014 (`_atom_site_site_symmetry_order`) and International Tables write them |
| `placeholder_entry.cif` | Entries without coordinates: an atom site row of `?` only, with and without a cell |
| `dummy_row.cif` | A dummy row whose label and coordinates are all `.` |
| `duplicate_labels.cif` | Labels used twice for different atoms; repeated anisotropic rows |
| `zero_occupancy.cif` | A disorder component refined to occupancy 0.00 |
| `corundum_hall.cif` | Operations only from a Hall symbol with a double prime, `-R 3 2"c` |
| `alias_spellings.cif` | A field under its current and legacy name: spelled differently but the same (no conflict), and two origin choices (a conflict) |
| `truncated_su.cif` | Damaged ADP values: `0.0029(18` without the closing parenthesis at a cut-off line end, and a typo `0..0042(15)` |

The operation lists were generated from group generators and are checked
for closure by the importer itself.
