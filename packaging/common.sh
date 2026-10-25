# What the macOS and Linux packaging scripts share (docs/design/release.md).
# Sourced, not run: sets ROOT (the repository) and VERSION (the
# application's), and defines the functions below. The Windows script does
# the same in PowerShell.

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
cd "$ROOT"

# One version for the tag, the application and the packages (§2.2): the
# workbench crate's. `cargo pkgid` ends in `#0.3.0` or `#workbench@0.3.0`.
VERSION=$(cargo pkgid -p workbench)
VERSION=${VERSION##*[#@]}

# The build number macOS records beside the version: the CI run's.
BUILD=${MW_BUILD_NUMBER:-${GITHUB_RUN_NUMBER:-1}}

die() {
    echo "$(basename "$0"): $*" >&2
    exit 1
}

# third_party_licenses OUT TARGET...: the licenses of every crate compiled
# into the executables for these targets, as HTML (§7.2). MW_NO_LICENSES=1
# writes a stub instead, for trying the scripts without cargo-about; such a
# package is not for release.
third_party_licenses() {
    out=$1
    shift
    if [ "${MW_NO_LICENSES:-}" = 1 ]; then
        echo "warning: MW_NO_LICENSES=1, the package lacks the third-party licenses" >&2
        printf '<!doctype html><title>Third-party licenses</title><p>Not generated (MW_NO_LICENSES=1).</p>\n' >"$out"
        return
    fi
    command -v cargo-about >/dev/null ||
        die "cargo-about is needed for the third-party licenses: cargo install cargo-about --locked (or MW_NO_LICENSES=1 to try without)"
    targets=
    for target in "$@"; do
        targets="$targets --target $target"
    done
    # Target names have no spaces: split on purpose.
    # shellcheck disable=SC2086
    cargo about generate --locked --fail --config about.toml $targets --output-file "$out" about.hbs
}
