# POSCAR test fixtures

All files here are written by hand for `../../poscar.rs`. Lattice constants and coordinates are commonly cited literature values typed in directly; nothing is copied from a database or produced by VASP, so there is no third-party license to track. The first line of each file, which POSCAR leaves free, says what it is.

| File | What it exercises |
|---|---|
| `si_diamond.vasp` | VASP 5 with an element line, a scaling factor applied to unit vectors, direct coordinates |
| `gaas_primitive_cartesian.vasp` | Cartesian coordinates scaled with the lattice; fcc primitive vectors, which are not in the standard orientation (explicit orientation, 60° angles kept exact) |
| `rutile_volume.vasp` | A negative scaling factor: the cell volume in Å³ |
| `CONTCAR` | The layout VASP writes after a run: POTCAR names with suffixes (`Ti_pv/…`), selective dynamics flags, and a block of velocities after the coordinates |
| `gaas_vasp4.vasp` | VASP 4: no element line, so the elements must be given |
| `zno_left_handed.vasp` | Lattice vectors that form a left-handed set (a and b of wurtzite swapped) |
