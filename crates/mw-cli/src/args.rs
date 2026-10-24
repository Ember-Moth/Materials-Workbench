//! The command line's arguments.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use mw_scripting::{Format, NewCell};

const ABOUT: &str = "The Materials Workbench command line: convert structure files, make \
supercells, change cells, find and apply symmetry. Every command takes a batch of files and \
works on them as the application does.";

const EXAMPLES: &str = "\
Examples:
  mw info rutile.cif run/CONTCAR
  mw convert rutile.cif --to poscar -o POSCAR
  mw convert *.cif --to poscar --out-dir vasp/
  mw supercell rutile.cif --repeat 2x2x2 --to xyz -o rutile-222.xyz
  mw transform corundum.cif --cell primitive
  mw symmetry relaxed/CONTCAR --tolerance 0.05
  mw symmetrize relaxed/CONTCAR --standardize -o symmetrized.cif
  mw diagnostics --json downloads/*.cif

Files are read by name: .cif as CIF; .vasp, .poscar and names that start with POSCAR or
CONTCAR as POSCAR; --from overrides it, and - reads stdin. Written structures go to stdout,
to --output for one structure, or into --out-dir, named after each structure.";

#[derive(Parser, Debug)]
#[command(name = "mw", version, about = ABOUT, after_help = EXAMPLES)]
pub struct Cli {
    /// Print one JSON document on stdout instead of text
    #[arg(long, global = true)]
    pub json: bool,
    #[command(subcommand)]
    pub command: Command,
}

// Parsed once per run; boxing the transform's matrix of strings would only
// make the arguments harder to read.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Summarize structures: formula, space group, cell, sites and atoms
    Info(Inputs),
    /// List what reading each file found, with source lines; fails for
    /// files with errors
    Diagnostics(Inputs),
    /// Write structures in another format
    Convert {
        #[command(flatten)]
        inputs: Inputs,
        #[command(flatten)]
        output: Output,
    },
    /// Make supercells: new structures in P1, the source repeated
    Supercell {
        #[command(flatten)]
        inputs: Inputs,
        #[command(flatten)]
        size: SupercellSize,
        #[command(flatten)]
        output: Output,
    },
    /// Describe structures in another cell; the atoms stay where they are
    Transform {
        #[command(flatten)]
        inputs: Inputs,
        #[command(flatten)]
        cell: CellArgs,
        #[command(flatten)]
        output: Output,
    },
    /// Find the symmetry the atoms have and preview symmetrizing by it
    Symmetry {
        #[command(flatten)]
        inputs: Inputs,
        #[command(flatten)]
        options: SymmetryArgs,
    },
    /// Symmetrize structures by the symmetry their atoms have and write
    /// them
    Symmetrize {
        #[command(flatten)]
        inputs: Inputs,
        #[command(flatten)]
        options: SymmetryArgs,
        #[command(flatten)]
        output: Output,
    },
    /// Call a read-only capability of the registry on each structure;
    /// --json gives its structured output
    Call {
        /// The capability, e.g. structure.sites (see `mw capabilities`)
        capability: String,
        #[command(flatten)]
        inputs: Inputs,
        /// The capability's arguments as a JSON object, less the structure
        #[arg(long, value_name = "JSON", default_value = "{}")]
        args: String,
    },
    /// List the capabilities of the registry; --json adds their schemas
    Capabilities,
}

#[derive(Args, Debug)]
pub struct Inputs {
    /// Structure files; - reads stdin (with --from)
    #[arg(required = true, value_name = "FILE")]
    pub files: Vec<PathBuf>,
    /// Read the files as this format, whatever their names: cif or poscar
    #[arg(long, value_name = "FORMAT", value_parser = input_format)]
    pub from: Option<Format>,
}

#[derive(Args, Debug)]
pub struct Output {
    /// The format to write: cif, poscar or xyz. Default: told from
    /// --output's name, or cif except for convert
    #[arg(long, value_name = "FORMAT", value_parser = format)]
    pub to: Option<Format>,
    /// Write to this file; for one structure only
    #[arg(short, long, value_name = "FILE", conflicts_with = "out_dir")]
    pub output: Option<PathBuf>,
    /// Write each structure into this directory, named after it
    #[arg(long, value_name = "DIR")]
    pub out_dir: Option<PathBuf>,
    /// Replace files that exist
    #[arg(long)]
    pub force: bool,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct SupercellSize {
    /// Repeats along a, b and c: 2x2x2, 2,2,1, or 3 for 3x3x3
    #[arg(long, value_name = "AxBxC", value_parser = repeat)]
    pub repeat: Option<[i32; 3]>,
    /// The supercell matrix M by rows, integers: column j is the new
    /// vector j in the old basis, e.g. "1 1 0; -1 1 0; 0 0 1"
    #[arg(long = "matrix", id = "supercell_matrix", value_name = "ROWS", value_parser = integer_matrix)]
    pub matrix: Option<[[i32; 3]; 3]>,
}

impl SupercellSize {
    pub fn matrix(&self) -> [[i32; 3]; 3] {
        match (self.repeat, self.matrix) {
            (_, Some(matrix)) => matrix,
            (Some([a, b, c]), None) => [[a, 0, 0], [0, b, 0], [0, 0, c]],
            (None, None) => unreachable!("clap requires one of them"),
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Preset {
    /// The conventional cell's centering removed
    Primitive,
    /// The conventional cell of the standard setting
    Conventional,
    /// The Niggli-reduced primitive cell
    Niggli,
}

#[derive(Args, Debug)]
pub struct CellArgs {
    #[command(flatten)]
    pub choice: CellChoice,
    /// With --matrix: the new origin p in the old basis, three fractions,
    /// e.g. "0 0 1/4"
    // `requires` alone is waived when --cell or --setting, which conflict
    // with --matrix, are given.
    #[arg(
        long,
        value_name = "XYZ",
        requires = "cell_matrix",
        conflicts_with_all = ["cell", "setting"],
        value_parser = fractions
    )]
    pub origin: Option<[String; 3]>,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct CellChoice {
    /// A cell of the structure's space group type
    #[arg(long, value_enum, value_name = "CELL")]
    pub cell: Option<Preset>,
    /// Another tabulated setting of the same space group type, by its
    /// Hermann–Mauguin symbol, e.g. "P 1 1 21/a" or "R -3 c :R"
    #[arg(long, value_name = "SYMBOL")]
    pub setting: Option<String>,
    /// The change of cell P of International Tables A by rows of
    /// fractions: column j is the new basis vector j in the old basis,
    /// e.g. "0 1/2 1/2; 1/2 0 1/2; 1/2 1/2 0"
    #[arg(long = "matrix", id = "cell_matrix", value_name = "ROWS", value_parser = fraction_matrix)]
    pub matrix: Option<[[String; 3]; 3]>,
}

impl CellArgs {
    pub fn new_cell(&self) -> NewCell {
        let choice = &self.choice;
        if let Some(preset) = choice.cell {
            return match preset {
                Preset::Primitive => NewCell::Primitive,
                Preset::Conventional => NewCell::Conventional,
                Preset::Niggli => NewCell::Niggli,
            };
        }
        if let Some(setting) = &choice.setting {
            return NewCell::Setting(setting.clone());
        }
        NewCell::Matrix {
            matrix: Box::new(choice.matrix.clone().expect("clap requires one choice")),
            origin: self.origin.clone(),
        }
    }

    /// What a file name says about the change, e.g. `-primitive`.
    pub fn suffix(&self) -> &'static str {
        match self.new_cell() {
            NewCell::Primitive => "-primitive",
            NewCell::Conventional => "-conventional",
            NewCell::Niggli => "-niggli",
            NewCell::Setting(_) => "-setting",
            NewCell::Matrix { .. } => "-transformed",
        }
    }
}

#[derive(Args, Debug)]
pub struct SymmetryArgs {
    /// Å: how far an atom may be from its symmetric image and still count
    /// as on it. Default: the structure's own, 0.01 Å
    #[arg(long, value_name = "Å")]
    pub tolerance: Option<f64>,
    /// Describe the result in the conventional cell of the standard
    /// setting instead of the structure's cell
    #[arg(long)]
    pub standardize: bool,
}

impl SymmetryArgs {
    /// As the registry takes it: 0 for the structure's own.
    pub fn tolerance(&self) -> f64 {
        self.tolerance.unwrap_or(0.0)
    }
}

fn format(text: &str) -> Result<Format, String> {
    Format::parse(text).ok_or_else(|| format!("{text:?} is no format; use cif, poscar or xyz"))
}

fn input_format(text: &str) -> Result<Format, String> {
    let format = format(text)?;
    if !format.can_read() {
        return Err(format!(
            "{} files are written, not read; use cif or poscar",
            format.title()
        ));
    }
    Ok(format)
}

/// Splits `a b c; d e f; g h i` (commas work too, and nine numbers without
/// semicolons) into three rows of three.
fn rows(text: &str) -> Result<[[String; 3]; 3], String> {
    let words = |part: &str| -> Vec<String> {
        part.split(|c: char| c == ',' || c.is_whitespace())
            .filter(|w| !w.is_empty())
            .map(str::to_string)
            .collect()
    };
    let rows: Vec<Vec<String>> = if text.contains(';') {
        text.split(';').map(words).collect()
    } else {
        words(text).chunks(3).map(<[String]>::to_vec).collect()
    };
    let wrong =
        || format!("{text:?} is not three rows of three numbers, such as \"2 0 0; 0 2 0; 0 0 1\"");
    let rows: Vec<[String; 3]> = rows
        .into_iter()
        .map(|row| <[String; 3]>::try_from(row).map_err(|_| wrong()))
        .collect::<Result<_, _>>()?;
    <[[String; 3]; 3]>::try_from(rows).map_err(|_| wrong())
}

fn integer_matrix(text: &str) -> Result<[[i32; 3]; 3], String> {
    let rows = rows(text)?;
    let mut matrix = [[0; 3]; 3];
    for (i, row) in rows.iter().enumerate() {
        for (j, entry) in row.iter().enumerate() {
            matrix[i][j] = entry.parse().map_err(|_| {
                format!("{entry:?} is not an integer; a supercell matrix has integers")
            })?;
        }
    }
    Ok(matrix)
}

fn fraction_matrix(text: &str) -> Result<[[String; 3]; 3], String> {
    rows(text)
}

fn fractions(text: &str) -> Result<[String; 3], String> {
    let words: Vec<String> = text
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect();
    <[String; 3]>::try_from(words)
        .map_err(|_| format!("{text:?} is not three fractions, such as \"0 0 1/4\""))
}

fn repeat(text: &str) -> Result<[i32; 3], String> {
    let parts: Vec<&str> = text
        .split(|c: char| c == 'x' || c == 'X' || c == '×' || c == ',' || c.is_whitespace())
        .filter(|p| !p.is_empty())
        .collect();
    let wrong = || format!("{text:?} is not a repeat such as 2x2x2, 2,2,1 or 3");
    let numbers: Vec<i32> = parts
        .iter()
        .map(|p| p.parse::<i32>().map_err(|_| wrong()))
        .collect::<Result<_, _>>()?;
    let repeat = match numbers[..] {
        [n] => [n, n, n],
        [a, b, c] => [a, b, c],
        _ => return Err(wrong()),
    };
    if repeat.iter().any(|&n| n < 1) {
        return Err(format!(
            "{text:?}: every repeat is a whole number of at least 1"
        ));
    }
    Ok(repeat)
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn the_command_line_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn repeats() {
        assert_eq!(repeat("2x2x2"), Ok([2, 2, 2]));
        assert_eq!(repeat("2×3×1"), Ok([2, 3, 1]));
        assert_eq!(repeat("2,2,1"), Ok([2, 2, 1]));
        assert_eq!(repeat("3"), Ok([3, 3, 3]));
        assert!(repeat("2x2").is_err());
        assert!(repeat("0x1x1").is_err());
        assert!(repeat("ax1x1").is_err());
    }

    #[test]
    fn matrices() {
        assert_eq!(
            integer_matrix("1 1 0; -1 1 0; 0 0 1"),
            Ok([[1, 1, 0], [-1, 1, 0], [0, 0, 1]])
        );
        assert_eq!(
            integer_matrix("2,0,0,0,2,0,0,0,1"),
            Ok([[2, 0, 0], [0, 2, 0], [0, 0, 1]])
        );
        assert!(integer_matrix("1 0 0; 0 1 0").is_err());
        assert!(integer_matrix("1/2 0 0; 0 1 0; 0 0 1").is_err());
        let p = fraction_matrix("0 1/2 1/2; 1/2 0 1/2; 1/2 1/2 0").unwrap();
        assert_eq!(p[0], ["0", "1/2", "1/2"]);
        assert_eq!(fractions("0, 0, 1/4").unwrap(), ["0", "0", "1/4"]);
        assert!(fractions("0 0").is_err());
    }
}
