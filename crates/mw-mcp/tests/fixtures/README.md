# MCP server test fixtures

Hand-written CIF files for the tests in `../stdio.rs` and `src/access.rs`.
Cell parameters and coordinates are commonly cited literature values typed in
directly; nothing is copied from COD or another database.

| File | What it exercises |
|---|---|
| `rutile.cif` | A copy of `mw-crystal-io/tests/fixtures/rutile.cif`: open, summarize, measure, export |
| `injection.cif` | Instructions to an AI model in a text field; operations from the H-M symbol; Ba/Sr mixed occupancy, which POSCAR export refuses |
