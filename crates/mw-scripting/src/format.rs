//! Structure file formats, and telling them from file names.

use std::fmt;
use std::path::Path;

/// A structure file format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    /// CIF 1.1: read and written.
    Cif,
    /// VASP POSCAR or CONTCAR: read and written (VASP 5).
    Poscar,
    /// Extended XYZ: written only.
    Xyz,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Cif, Format::Poscar, Format::Xyz];

    /// As command lines and Python name it: `cif`, `poscar` or `xyz`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cif => "cif",
            Self::Poscar => "poscar",
            Self::Xyz => "xyz",
        }
    }

    /// For messages: `CIF`, `POSCAR`, `extended XYZ`.
    pub fn title(self) -> &'static str {
        match self {
            Self::Cif => "CIF",
            Self::Poscar => "POSCAR",
            Self::Xyz => "extended XYZ",
        }
    }

    /// The usual file extension, without the dot: `cif`, `vasp` or `xyz`,
    /// as the interface's export dialog suggests.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Cif => "cif",
            Self::Poscar => "vasp",
            Self::Xyz => "xyz",
        }
    }

    /// Whether files of this format can be read.
    pub fn can_read(self) -> bool {
        !matches!(self, Self::Xyz)
    }

    /// A format by name, not case-sensitive: `cif`; `poscar`, `contcar` or
    /// `vasp`; `xyz` or `extxyz`.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "cif" => Some(Self::Cif),
            "poscar" | "contcar" | "vasp" => Some(Self::Poscar),
            "xyz" | "extxyz" => Some(Self::Xyz),
            _ => None,
        }
    }

    /// The format a file is read as, from its name, by the interface's
    /// rule (`mw_structure::import::format_of`, ui-shell design §3):
    /// `.cif` is CIF; `.vasp`, `.poscar` and names that start with POSCAR
    /// or CONTCAR (`POSCAR`, `CONTCAR`, `POSCAR-001`, `CONTCAR.relax`) are
    /// POSCAR.
    pub fn of_input(path: &Path) -> Option<Self> {
        let extension = path.extension().and_then(|e| e.to_str());
        let is = |names: &[&str]| {
            extension.is_some_and(|e| names.iter().any(|n| e.eq_ignore_ascii_case(n)))
        };
        if is(&["cif"]) {
            return Some(Self::Cif);
        }
        let name = path.file_name()?.to_str()?.to_ascii_uppercase();
        (is(&["vasp", "poscar"]) || name.starts_with("POSCAR") || name.starts_with("CONTCAR"))
            .then_some(Self::Poscar)
    }

    /// The format to write a file as, from its name: what [`of_input`]
    /// says, and `.xyz` or `.extxyz` for extended XYZ.
    ///
    /// [`of_input`]: Self::of_input
    pub fn of_output(path: &Path) -> Option<Self> {
        let extension = path.extension().and_then(|e| e.to_str());
        if extension
            .is_some_and(|e| e.eq_ignore_ascii_case("xyz") || e.eq_ignore_ascii_case("extxyz"))
        {
            return Some(Self::Xyz);
        }
        Self::of_input(path)
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_formats_follow_the_interface() {
        let of = |name: &str| Format::of_input(Path::new(name));
        assert_eq!(of("rutile.cif"), Some(Format::Cif));
        assert_eq!(of("dir/RUTILE.CIF"), Some(Format::Cif));
        assert_eq!(of("si.vasp"), Some(Format::Poscar));
        assert_eq!(of("si.POSCAR"), Some(Format::Poscar));
        assert_eq!(of("run/POSCAR"), Some(Format::Poscar));
        assert_eq!(of("CONTCAR"), Some(Format::Poscar));
        assert_eq!(of("contcar.relax"), Some(Format::Poscar));
        assert_eq!(of("POSCAR-001"), Some(Format::Poscar));
        assert_eq!(of("structure.xyz"), None);
        assert_eq!(of("notes.txt"), None);
        assert_eq!(of("cif"), None);
    }

    #[test]
    fn output_formats_add_xyz() {
        let of = |name: &str| Format::of_output(Path::new(name));
        assert_eq!(of("out.xyz"), Some(Format::Xyz));
        assert_eq!(of("out.extxyz"), Some(Format::Xyz));
        assert_eq!(of("out.cif"), Some(Format::Cif));
        assert_eq!(of("POSCAR"), Some(Format::Poscar));
        assert_eq!(of("out.txt"), None);
    }

    #[test]
    fn names() {
        for format in Format::ALL {
            assert_eq!(Format::parse(format.name()), Some(format));
        }
        assert_eq!(Format::parse(" VASP "), Some(Format::Poscar));
        assert_eq!(Format::parse("extxyz"), Some(Format::Xyz));
        assert_eq!(Format::parse("pdb"), None);
    }
}
