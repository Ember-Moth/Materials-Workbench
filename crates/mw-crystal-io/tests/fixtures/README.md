# CIF test fixtures

All files here are written by hand for the tests in `../fixtures.rs`. Cell
parameters and coordinates are commonly cited literature values typed in
directly; no file is copied from COD or another database, so there is no
third-party license to track.

| File | What it exercises |
|---|---|
| `rutile.cif` | Current field names, 16 listed operations; must match the built-in rutile sample |
| `SrTiO3.cif` | Pm-3m with 48 quoted operations |
| `zincite_legacy.cif` | Legacy `_symmetry_*` names, decimal translations (`z+0.5`), Biso, elements given only by labels |
| `bst_su_aniso.cif` | Standard uncertainties, an anisotropic ADP loop, Ba/Sr mixed occupancy on one position |
| `malformed.cif` | Syntax and content errors at known lines |
| `multi_block.cif` | A publication block, a P1 structure without operations, and a block that lists no operations |

The operation lists were generated from group generators and are checked
for closure by the importer itself.
