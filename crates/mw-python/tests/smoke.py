"""Smoke test of the Python module materials_workbench (docs/design/scripting.md §5.6).

Usage: python -I crates/mw-python/tests/smoke.py TARGET_DIR [MW]
       python -I crates/mw-python/tests/smoke.py --installed

TARGET_DIR is the Cargo profile directory holding the library that
`cargo build -p mw-python --features extension-module` built (target/debug).
The script copies it into a temporary directory under the name Python imports
(materials_workbench.so, or .pyd on Windows), imports it from there and checks
it on the test files of mw-crystal-io. Given the `mw` executable, it also checks
that the command line writes the same text. With --installed it checks the
module installed in the running Python instead, as CI does with the wheels.

Nothing needs installing for the first form: `python -I` ignores PYTHONPATH
and the user's site packages, so the directory is put on sys.path here.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from fractions import Fraction
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
FIXTURES = ROOT / "crates" / "mw-crystal-io" / "tests" / "fixtures"
LIBRARIES = ["libmw_python.dylib", "libmw_python.so", "mw_python.dll"]

checks = 0


def check(condition, what):
    global checks
    if not condition:
        raise AssertionError(what)
    checks += 1


def raises(kind, call, what):
    try:
        call()
    except kind as error:
        check(True, what)
        return error
    raise AssertionError(f"{what}: {kind.__name__} not raised")


def load(target):
    library = next((target / name for name in LIBRARIES if (target / name).exists()), None)
    if library is None:
        sys.exit(f"no library in {target}; build it with "
                 "cargo build -p mw-python --features extension-module")
    directory = Path(tempfile.mkdtemp(prefix="mw-python-"))
    suffix = ".pyd" if os.name == "nt" else ".so"
    shutil.copy(library, directory / f"materials_workbench{suffix}")
    sys.path.insert(0, str(directory))
    import materials_workbench

    return materials_workbench, directory


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    executable = None
    if sys.argv[1] == "--installed":
        import materials_workbench as mw

        directory = Path(tempfile.mkdtemp(prefix="mw-python-"))
        print(f"materials_workbench {mw.__version__} installed at {Path(mw.__file__).parent}")
    else:
        mw, directory = load(Path(sys.argv[1]).resolve())
        print(f"materials_workbench {mw.__version__} from {directory}")
        if len(sys.argv) > 2:
            executable = Path(sys.argv[2]).resolve()
            if not executable.exists() and executable.with_suffix(".exe").exists():
                executable = executable.with_suffix(".exe")

    # Reading and querying.
    rutile = mw.read(FIXTURES / "rutile.cif")
    check(rutile.name == "rutile", "named after the file")
    check(rutile.file == "rutile.cif", "the file it came from")
    summary = rutile.summary()
    check(summary["formula"] == "O2Ti", "formula")
    check(summary["space_group"]["number"] == 136, "space group")
    check(summary["atoms_in_cell"] == 6, "atoms in the cell")
    check("O2Ti" in repr(rutile), repr(rutile))
    sites = rutile.sites()
    check([site["label"] for site in sites] == ["Ti1", "O1"], "sites")
    site = rutile.call("structure.site_symmetry", {"site": sites[1]["site"]})
    check(site["wyckoff"]["letter"] == "f", "a capability by name")
    same = rutile.call("structure.site_symmetry", json.dumps({"site": sites[1]["site"]}))
    check(same == site, "arguments as JSON text")
    contcar = mw.read(FIXTURES / "poscar" / "CONTCAR")
    check(contcar.name == "poscar · CONTCAR", contcar.name)
    check(contcar.summary()["edit_mode"] == "p1", "POSCAR is P1")
    near = mw.read(FIXTURES / "rutile_near_special.cif")
    codes = {d["code"] for d in near.diagnostics()}
    check("atom.snapped" in codes, codes)
    blocks = mw.read_all(FIXTURES / "multi_block.cif")
    check(len(blocks) >= 2 and all(b.name.startswith("multi_block · ") for b in blocks), "blocks")
    text = (FIXTURES / "rutile.cif").read_text(encoding="utf-8")
    check(mw.read_text(text, "cif").name == "rutile", "text named after its block")

    # Supercells are new structures.
    supercell = rutile.supercell(2)
    check(supercell.name == "rutile 2×2×2 supercell", supercell.name)
    check(supercell.summary()["atoms_in_cell"] == 48, "2×2×2 supercell")
    check(rutile.summary()["atoms_in_cell"] == 6, "the source does not change")
    check(rutile.supercell([2, 2, 1]).summary()["atoms_in_cell"] == 24, "repeats")
    skewed = rutile.supercell([[1, 1, 0], [-1, 1, 0], [0, 0, 1]])
    check(skewed.summary()["atoms_in_cell"] == 12, "a supercell matrix")
    error = raises(mw.CapabilityError, lambda: rutile.supercell([[1, 0, 0], [0, 1, 0], [0, 0, 0]]),
                   "a flat supercell")
    check(error.code == "supercell_not_positive", error.code)
    raises(ValueError, lambda: rutile.supercell(0), "a repeat of 0")
    raises(TypeError, lambda: rutile.supercell("2x2x2"), "a size as text")

    # Changes of cell and symmetrizing happen in place, undoably.
    corundum = mw.read(FIXTURES / "corundum_hall.cif")
    result = corundum.transform_cell("primitive")
    check(result["changed"] and result["atoms_in_cell"] == 10, "primitive cell")
    check(corundum.undo() and corundum.summary()["atoms_in_cell"] == 30, "undo")
    check(corundum.redo() and corundum.summary()["atoms_in_cell"] == 10, "redo")
    check(corundum.undo(), "undo again")
    copy = corundum.copy()
    shifted = copy.transform_cell(matrix=[[1, 0, 0], [0, 1, 0], [0, 0, 1]],
                                  origin=[0, 0.5, Fraction(1, 4)])
    check(shifted["origin"] == ["0", "1/2", "1/4"], shifted["origin"])
    check(corundum.summary()["atoms_in_cell"] == 30, "a copy is independent")
    error = raises(mw.CapabilityError,
                   lambda: corundum.transform_cell(matrix=[["1", 0, 0], [0, 1, 0], [0, 0, 0]]),
                   "a singular matrix")
    check(error.code == "transform_not_positive", error.code)
    check(isinstance(error.guidance, list), "guidance")
    raises(ValueError, lambda: corundum.transform_cell("primitive", setting="R -3 c :R"),
           "two cells at once")

    halite = mw.read(FIXTURES / "halite_p1.cif")
    preview = halite.detect_symmetry()
    check(preview["detected"]["number"] == 225, "detected symmetry")
    done = halite.symmetrize()
    check(done["result"] == preview, "symmetrizing does what the preview says")
    check(halite.summary()["space_group"]["number"] == 225, "symmetrized")
    error = raises(mw.CapabilityError, rutile.detect_symmetry, "no higher symmetry")
    check(error.code == "symmetrize_no_higher_symmetry", error.code)

    # Writing.
    poscar = rutile.export("poscar")
    check(poscar.splitlines()[0] == "rutile · rutile.cif", poscar.splitlines()[0])
    out = directory / "POSCAR"
    rutile.write(out)
    check(out.read_text(encoding="utf-8") == poscar, "written as exported")
    check(mw.read(out).summary()["formula"] == "O2Ti", "read back")
    error = raises(mw.CapabilityError, lambda: mw.read(FIXTURES / "bst_su_aniso.cif").export("poscar"),
                   "partial occupancy in POSCAR")
    check(error.code == "partial_occupancy", error.code)
    raises(ValueError, lambda: rutile.write(directory / "out.txt"), "a name without a format")

    # The registry.
    names = [c["name"] for c in mw.capabilities()]
    check("structure.summary" in names and "structure.transform_cell" in names, "capabilities")
    error = raises(mw.CapabilityError, lambda: rutile.call("structure.nothing"), "unknown capability")
    check(error.code == "unknown_capability", error.code)

    # Files that cannot be read.
    raises(FileNotFoundError, lambda: mw.read(FIXTURES / "missing.cif"), "a missing file")
    raises(mw.ReadError, lambda: mw.read(ROOT / "README.md"), "an unknown format")
    error = raises(mw.ReadError, lambda: mw.read(FIXTURES / "malformed.cif"), "import errors")
    check("line 13" in str(error) and error.diagnostics["failed"], str(error))
    found = mw.diagnostics(FIXTURES / "malformed.cif")
    check(found["failed"] and found["blocks"][0]["errors"] > 0, "diagnostics of a failed file")
    check(issubclass(mw.ReadError, mw.WorkbenchError), "one base class")

    # The command line writes the same.
    if executable is not None:
        def mw_cli(*args):
            return subprocess.run([str(executable), *map(str, args)], check=True,
                                  capture_output=True, text=True, encoding="utf-8").stdout

        check(mw_cli("convert", FIXTURES / "rutile.cif", "--to", "poscar") == poscar,
              "mw convert writes what export writes")
        check(mw_cli("supercell", FIXTURES / "rutile.cif", "--repeat", "2", "--to", "cif")
              == supercell.export("cif"), "mw supercell makes the same supercell")
        cli = json.loads(mw_cli("symmetry", "--json", FIXTURES / "halite_p1.cif"))
        check(cli["results"][0]["output"] == preview, "mw symmetry finds the same")

    shutil.rmtree(directory, ignore_errors=True)
    print(f"ok: {checks} checks")


if __name__ == "__main__":
    main()
