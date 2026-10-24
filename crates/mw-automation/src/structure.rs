//! Structure queries and edits (ai-integration design §5.2): the same
//! domain commands the interface runs (crystallography design §7.2), named
//! by stable ids.

use mw_crystal::{
    AtomRef, Axis, CellParam, CellPreset, CellTransform, CellValues, Command, Constraint,
    CoordinateConstraint, CrystalDocument, DetectSymmetry, Edit, EditError, EditMode, Formula,
    Measured, Measurement, Outcome, Rational, Setting, SiteId, SpaceGroupType, Species, Structure,
    Symmetrization, Target, make_supercell,
};

use crate::registry::{CallError, Effect, Host, Output, Registry, Spec, StructureEntryId};
use crate::schema::{NonEmpty, schema_struct, string_enum};

/// Neighbors returned at most.
const MAX_NEIGHBORS: usize = 100;
/// Å; the neighbor search grows with the cube of the cutoff.
const MAX_CUTOFF: f64 = 10.0;
/// Sites per page.
const MAX_SITES: u32 = 500;
/// Atoms a supercell may have; the interface warns from here on that
/// showing it is slow, and a model has no reason to go further.
pub const MAX_SUPERCELL_ATOMS: usize = 100_000;

schema_struct! {
    pub struct StructureInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
    }
}

string_enum! {
    pub enum ModeOutput {
        Symmetry => "symmetry",
        P1 => "p1",
    }
}

schema_struct! {
    pub struct SpaceGroupOutput {
        /// International Tables number, 1–230.
        pub number: u16,
        /// Short Hermann–Mauguin symbol, e.g. P 4_2/m n m.
        pub symbol: String,
        /// "standard", another tabulated setting with its code, or
        /// "transformed" for a setting the tables do not list.
        pub setting: String,
    }
}

schema_struct! {
    pub struct DetectedOutput {
        pub number: u16,
        pub symbol: String,
    }
}

schema_struct! {
    pub struct LinkOutput {
        pub parameter: CellParam,
        /// The parameter it always equals.
        pub equals: CellParam,
    }
}

schema_struct! {
    pub struct CellOutput {
        /// Å.
        pub a: f64,
        /// Å.
        pub b: f64,
        /// Å.
        pub c: f64,
        /// Degrees.
        pub alpha: f64,
        /// Degrees.
        pub beta: f64,
        /// Degrees.
        pub gamma: f64,
        /// Å³.
        pub volume: f64,
        /// Parameters the symmetry fixes, e.g. 90° angles; they cannot be set.
        pub fixed: Vec<CellParam>,
        /// Parameters that change together with another.
        pub linked: Vec<LinkOutput>,
    }
}

schema_struct! {
    pub struct SummaryOutput {
        pub name: String,
        /// Per formula unit, elements in Hill order, e.g. O2Ti.
        pub formula: String,
        /// Formula units per cell, Z.
        pub formula_units: u32,
        /// "symmetry": edits apply to whole sites, all equivalent atoms at
        /// once. "p1": every atom is its own site.
        pub edit_mode: ModeOutput,
        /// Identified from the symmetry operations; null if identification
        /// failed.
        pub space_group: Option<SpaceGroupOutput>,
        pub cell: CellOutput,
        /// g/cm³; null when an element has no known atomic weight.
        pub density: Option<f64>,
        /// Sites in the asymmetric unit.
        pub sites: usize,
        /// Atoms in the unit cell.
        pub atoms_in_cell: usize,
        /// Symmetry operations, including centering translations.
        pub operations: usize,
        /// For a structure in P1, the higher symmetry its atoms have; null
        /// otherwise.
        pub detected_space_group: Option<DetectedOutput>,
    }
}

schema_struct! {
    pub struct SitesInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// Sites to skip, for paging; 0 for the first page.
        pub offset: u32,
        /// Sites to return, 1 to 500.
        pub limit: u32,
    }
}

schema_struct! {
    pub struct OccupantOutput {
        /// Unique in the structure; names the occupant in edits.
        pub label: String,
        /// Element symbol, or X for an unknown species.
        pub element: String,
        /// The species as the source wrote it, e.g. Fe3+.
        pub symbol: String,
        /// 0 < occupancy ≤ 1.
        pub occupancy: f64,
    }
}

schema_struct! {
    pub struct SiteOutput {
        pub site: SiteId,
        /// Label of the first occupant.
        pub label: String,
        /// Fractional coordinates as stored, not wrapped into the cell.
        pub position: [f64; 3],
        /// Equivalent atoms in the unit cell.
        pub multiplicity: usize,
        /// Wyckoff position, e.g. 4f; null if not identified.
        pub wyckoff: Option<String>,
        /// The coordinates the site's special position allows, e.g. x, x, 0.
        pub coordinate_form: String,
        /// More than one for mixed occupancy.
        pub occupants: Vec<OccupantOutput>,
    }
}

schema_struct! {
    pub struct SitesOutput {
        /// Sites in the asymmetric unit.
        pub total: usize,
        pub sites: Vec<SiteOutput>,
    }
}

schema_struct! {
    pub struct SiteInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// The site's id, from structure.sites.
        pub site: SiteId,
    }
}

string_enum! {
    pub enum CoordinateKind {
        Free => "free",
        /// Determined by one free coordinate, e.g. y = 2x.
        Follows => "follows",
        /// Determined by several free coordinates (non-standard settings).
        Combination => "combination",
        Fixed => "fixed",
    }
}

schema_struct! {
    pub struct CoordinateOutput {
        pub axis: String,
        pub constraint: CoordinateKind,
        /// The coordinate in terms of the free ones, e.g. 2x or 1/4.
        pub expression: String,
    }
}

schema_struct! {
    pub struct WyckoffOutput {
        pub letter: String,
        /// In the conventional cell of the tabulated setting.
        pub multiplicity: usize,
        /// Oriented site-symmetry symbol, e.g. m.2m.
        pub site_symmetry: String,
    }
}

schema_struct! {
    pub struct SiteSymmetryOutput {
        pub site: SiteId,
        pub label: String,
        /// Order of the site-symmetry group: operations that leave the site
        /// in place.
        pub order: usize,
        /// Equivalent atoms in the unit cell.
        pub multiplicity: usize,
        pub wyckoff: Option<WyckoffOutput>,
        /// x, y and z.
        pub coordinates: Vec<CoordinateOutput>,
    }
}

schema_struct! {
    pub struct AtomsInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// Two atoms for a distance; three for an angle, the second being
        /// the vertex.
        pub atoms: NonEmpty<AtomRef>,
    }
}

schema_struct! {
    pub struct DistanceOutput {
        /// Å.
        pub distance: f64,
    }
}

schema_struct! {
    pub struct AngleOutput {
        /// Degrees, at the second atom.
        pub angle: f64,
    }
}

schema_struct! {
    pub struct NeighborsInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// The central atom; site id, member 0 and cell [0, 0, 0] is the
        /// site itself.
        pub atom: AtomRef,
        /// Å, more than 0 and at most 10.
        pub cutoff: f64,
    }
}

schema_struct! {
    pub struct NeighborOutput {
        pub atom: AtomRef,
        pub label: String,
        pub element: String,
        /// Å.
        pub distance: f64,
    }
}

schema_struct! {
    pub struct NeighborsOutput {
        /// Nearest first.
        pub neighbors: Vec<NeighborOutput>,
        /// Whether more atoms were within the cutoff than returned.
        pub truncated: bool,
    }
}

schema_struct! {
    pub struct CellValueInput {
        pub parameter: CellParam,
        /// Å for a, b and c; degrees for alpha, beta and gamma.
        pub value: f64,
    }
}

schema_struct! {
    pub struct SetCellInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// Parameters to set; the others keep their values or follow the
        /// ones they are linked to.
        pub values: NonEmpty<CellValueInput>,
    }
}

schema_struct! {
    pub struct SetElementInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// Label of the occupant to change, from structure.sites.
        pub occupant: String,
        /// Element symbol, e.g. Zr.
        pub element: String,
        /// Leave empty to change every equivalent atom of the site. Listing
        /// atoms asks for only those; under symmetry that is refused unless
        /// they are all the site's equivalent atoms.
        pub atoms: Vec<AtomRef>,
    }
}

schema_struct! {
    pub struct SetOccupancyInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// Label of the occupant to change, from structure.sites.
        pub occupant: String,
        /// More than 0 and at most 1.
        pub occupancy: f64,
        /// Leave empty to change every equivalent atom of the site. Listing
        /// atoms asks for only those; under symmetry that is refused unless
        /// they are all the site's equivalent atoms.
        pub atoms: Vec<AtomRef>,
    }
}

schema_struct! {
    pub struct AddOccupantInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// The site's id, from structure.sites.
        pub site: SiteId,
        /// Element symbol, e.g. Ni.
        pub element: String,
        /// More than 0 and at most 1.
        pub occupancy: f64,
    }
}

schema_struct! {
    pub struct OccupantInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// Label of the occupant, from structure.sites.
        pub occupant: String,
    }
}

schema_struct! {
    pub struct RenameInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// Current label of the occupant.
        pub occupant: String,
        /// New label: unique in the structure, non-empty, without white
        /// space, not starting with _ # $ ' " [ ] or ;.
        pub label: String,
    }
}

schema_struct! {
    pub struct DeleteSitesInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// Sites to delete with all their equivalent atoms.
        pub sites: Vec<SiteId>,
        /// Atoms to delete. Under symmetry, atoms stand for their site only
        /// when they are all its equivalent atoms (or it has one); otherwise
        /// the deletion is refused.
        pub atoms: Vec<AtomRef>,
    }
}

schema_struct! {
    pub struct SetPositionInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// The site's id, from structure.sites.
        pub site: SiteId,
        /// New fractional coordinates.
        pub position: [f64; 3],
    }
}

schema_struct! {
    pub struct MoveSiteInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// The atom to drag, as structure.neighbors gives atoms; all its
        /// equivalent atoms move with it.
        pub atom: AtomRef,
        /// The atom's displacement in fractional coordinates, multiples of
        /// the cell vectors a, b and c.
        pub displacement: [f64; 3],
        /// False to keep the site on its special position: only the part of
        /// the displacement along it counts. True to move by the whole
        /// displacement, leaving the special position; the site then has
        /// more equivalent atoms.
        pub leave_special_position: bool,
    }
}

schema_struct! {
    pub struct SetCoordinateInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// The site's id, from structure.sites.
        pub site: SiteId,
        /// The fractional coordinate to set; structure.site_symmetry tells
        /// which can be set.
        pub axis: Axis,
        /// Its new value, in fractions of the cell vector.
        pub value: f64,
    }
}

schema_struct! {
    pub struct AddSiteInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// Element symbol, e.g. O or Fe.
        pub element: String,
        /// Fractional coordinates.
        pub position: [f64; 3],
        /// More than 0 and at most 1; 1 for a fully occupied site.
        pub occupancy: f64,
    }
}

string_enum! {
    pub enum CellChoice {
        Primitive => "primitive",
        Conventional => "conventional",
        Niggli => "niggli",
        Setting => "setting",
        Matrix => "matrix",
    }
}

schema_struct! {
    pub struct TransformCellInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// The new cell. primitive: the conventional cell's centering
        /// removed. conventional: the conventional cell of the standard
        /// setting. niggli: the Niggli-reduced primitive cell. setting:
        /// another tabulated setting of the same space group type, named
        /// by `setting`. matrix: the change of cell given by `matrix` and
        /// `origin`.
        pub cell: CellChoice,
        /// For cell = setting: the Hermann–Mauguin symbol of the setting,
        /// e.g. P 1 1 21/a, P n m a, F d -3 m :1 or R -3 c :R. Empty
        /// otherwise.
        pub setting: String,
        /// For cell = matrix: the transformation matrix P of International
        /// Tables A, three rows of three fractions such as "1/2" or "-1".
        /// Column j holds new basis vector j in the old basis, so
        /// [["0","1/2","1/2"],["1/2","0","1/2"],["1/2","1/2","0"]] makes
        /// a′ = (b + c)/2, b′ = (a + c)/2, c′ = (a + b)/2. Its determinant
        /// must be positive and between 1/8 and 8. Empty otherwise.
        pub matrix: Vec<[String; 3]>,
        /// For cell = matrix: the new origin p in the old basis, three
        /// fractions. Empty to keep the origin.
        pub origin: Vec<String>,
    }
}

schema_struct! {
    pub struct TransformCellOutput {
        /// False when the structure already was in that cell; nothing
        /// changed.
        pub changed: bool,
        /// The document's revision after the call; it counts changes.
        pub revision: u64,
        /// The change of cell made, in International Tables notation: the
        /// new basis vectors in the old ones, then the new origin, e.g.
        /// a-b, a+b, c; 0, 0, 1/4.
        pub transformation: String,
        /// The rows of P, as fractions.
        pub matrix: Vec<[String; 3]>,
        /// The new origin in the old basis, as fractions.
        pub origin: Vec<String>,
        /// In the new cell; null if identification failed.
        pub space_group: Option<SpaceGroupOutput>,
        pub cell: CellOutput,
        /// Atoms in the new unit cell.
        pub atoms_in_cell: usize,
        /// Symmetry operations in the new cell, including centering
        /// translations.
        pub operations: usize,
    }
}

schema_struct! {
    pub struct SymmetrizeInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// Å: how far an atom may be from its symmetric image and still count
        /// as on it. 0 for the structure's own tolerance, 0.01 Å unless set
        /// otherwise.
        pub tolerance: f64,
        /// True to describe the result in the conventional cell of the
        /// standard setting; false to keep the structure's cell.
        pub standardize: bool,
    }
}

schema_struct! {
    pub struct SymmetrizationOutput {
        /// The space group type of the atoms' symmetry.
        pub detected: DetectedOutput,
        /// The type of the symmetrized structure's operations: the detected
        /// type, or a subgroup of it when the kept cell cannot hold all of
        /// it (a 2a × a × a cell of a cubic crystal; standardizing gives all
        /// of it). Null if identification failed.
        pub space_group: Option<SpaceGroupOutput>,
        /// Sites of the symmetrized asymmetric unit.
        pub sites: usize,
        /// Atoms in the symmetrized unit cell.
        pub atoms_in_cell: usize,
        /// Symmetry operations, including centering translations.
        pub operations: usize,
        /// Å: the largest distance an atom moves, the change of the cell
        /// included and a shift of the origin not.
        pub max_displacement: f64,
        /// The label of the site whose atoms move most; empty when none
        /// moves.
        pub moved_most: String,
        /// Whether the result is described in another cell.
        pub cell_changed: bool,
        /// Whether the cell lies differently in Cartesian space.
        pub orientation_changed: bool,
        /// Å: how far the origin moves; 0 when it stays.
        pub origin_shift: f64,
        /// The symmetrized cell.
        pub cell: CellOutput,
    }
}

schema_struct! {
    pub struct MakeSupercellInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
        /// The integer matrix M by rows: the supercell's basis vectors are
        /// A·M, so [[2,0,0],[0,2,0],[0,0,2]] doubles every axis. Its
        /// determinant, the number of cells the supercell holds, must be
        /// positive.
        pub matrix: [[i32; 3]; 3],
        /// The new structure's name; empty for the default, e.g. 金红石
        /// 2×2×2 supercell.
        pub name: String,
    }
}

schema_struct! {
    pub struct MakeSupercellOutput {
        /// The new structure's id; it is open.
        pub structure: StructureEntryId,
        pub name: String,
        /// The structure it was made from.
        pub source: StructureEntryId,
        /// Cells of the original the supercell holds, det M.
        pub cells: i64,
        /// Sites of the supercell, which is in P1: one per atom.
        pub sites: usize,
        pub cell: CellOutput,
    }
}

/// The default name of a supercell made by `matrix`, e.g. `金红石 2×2×2
/// supercell`; hosts with an interface language may name it in that.
pub fn supercell_name(base: &str, matrix: [[i32; 3]; 3]) -> String {
    let diagonal = (0..3).all(|i| (0..3).all(|j| i == j || matrix[i][j] == 0));
    if diagonal {
        format!(
            "{base} {}×{}×{} supercell",
            matrix[0][0], matrix[1][1], matrix[2][2]
        )
    } else {
        format!("{base} supercell")
    }
}

fn determinant(m: &[[i32; 3]; 3]) -> i64 {
    let m = m.map(|row| row.map(i64::from));
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

schema_struct! {
    pub struct SymmetrizeOutput {
        /// False when nothing changed.
        pub changed: bool,
        /// The document's revision after the call; it counts changes.
        pub revision: u64,
        /// What symmetrizing did.
        pub result: SymmetrizationOutput,
    }
}

schema_struct! {
    pub struct WarningOutput {
        /// Stable, e.g. occupancy_sum or short_contact.
        pub code: String,
        pub message: String,
    }
}

schema_struct! {
    pub struct InfoOutput {
        /// Stable: snapped (a site was pulled onto a special position) or
        /// multiplicity (a site's number of equivalent atoms changed).
        pub code: String,
        pub message: String,
    }
}

schema_struct! {
    pub struct EditOutput {
        /// False when the structure already was as asked; nothing changed.
        pub changed: bool,
        /// The document's revision after the call; it counts changes.
        pub revision: u64,
        /// The sites the edit applied to.
        pub sites: Vec<SiteId>,
        pub warnings: Vec<WarningOutput>,
        /// What the edit did besides what was asked.
        pub info: Vec<InfoOutput>,
    }
}

const STRUCTURE_ID_HINT: &str = "structure.list gives the ids";

pub(crate) fn document(
    host: &dyn Host,
    id: StructureEntryId,
) -> Result<&CrystalDocument, CallError> {
    host.document(id).ok_or_else(|| missing(host, id))
}

/// Runs an edit through the host as one undo step of the calling client
/// (design §5.3), and gives the document as it is afterwards.
pub(crate) fn run(
    host: &mut dyn Host,
    id: StructureEntryId,
    edit: Edit,
) -> Result<(&CrystalDocument, Outcome), CallError> {
    document(host, id)?;
    let agent = host.agent();
    let outcome = host.execute(id, Command::Edit(edit), agent)?;
    Ok((document(host, id)?, outcome))
}

/// Why there is no document: a structure of a project that is not open
/// yet, or no structure at all.
fn missing(host: &dyn Host, id: StructureEntryId) -> CallError {
    crate::project::not_open(host, id).unwrap_or_else(|| {
        CallError::new(
            "unknown_structure",
            format!("no structure with id {}; {STRUCTURE_ID_HINT}", id.0),
        )
    })
}

pub(crate) fn invalid(message: impl Into<String>) -> CallError {
    CallError::new("invalid_arguments", message)
}

fn label(structure: &Structure, site: SiteId) -> String {
    structure
        .site(site)
        .map_or_else(String::new, |s| s.label().to_string())
}

/// The site that holds the occupant with this label.
fn occupant_site(structure: &Structure, occupant: &str) -> Result<SiteId, CallError> {
    structure
        .sites
        .iter()
        .find(|site| site.occupants.iter().any(|o| o.label == occupant))
        .map(|site| site.id)
        .ok_or_else(|| {
            CallError::new(
                "unknown_occupant",
                format!("no occupant labelled {occupant:?}; structure.sites lists the labels"),
            )
        })
}

/// An element symbol with its usual case, so that `zr` and `ZR` find Zr.
fn element_symbol(text: &str) -> String {
    let mut chars = text.trim().chars();
    chars.next().map_or_else(String::new, |first| {
        first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect()
    })
}

/// Checks that `atoms`, if any, are all equivalent atoms of `site`: an
/// edit of some of them would break the symmetry (design §7.1).
fn whole_site(
    document: &CrystalDocument,
    site: SiteId,
    atoms: &[AtomRef],
) -> Result<(), CallError> {
    if let Some(atom) = atoms.iter().find(|atom| atom.site != site) {
        return Err(invalid(format!(
            "an atom of site {} is listed, but the occupant is on site {}",
            atom.site.0, site.0
        )));
    }
    if !atoms.is_empty() {
        let targets: Vec<Target> = atoms.iter().copied().map(Target::Atom).collect();
        document.target_sites(&targets)?;
    }
    Ok(())
}

fn edit_output(document: &CrystalDocument, outcome: Outcome, sites: Vec<SiteId>) -> EditOutput {
    EditOutput {
        changed: outcome.changed,
        revision: document.revision(),
        sites,
        warnings: outcome
            .warnings
            .iter()
            .map(|warning| WarningOutput {
                code: warning.code().into(),
                message: warning.to_string(),
            })
            .collect(),
        info: outcome
            .info
            .iter()
            .map(|info| InfoOutput {
                code: info.code().into(),
                message: info.to_string(),
            })
            .collect(),
    }
}

/// The summary of an edit, with what else it did and its warnings.
fn edit_summary(output: &EditOutput, done: String) -> String {
    let mut summary = if output.changed {
        done
    } else {
        "Nothing changed: the structure already was as asked.".into()
    };
    for info in &output.info {
        summary.push_str(&format!(" Note: {}.", info.message));
    }
    for warning in &output.warnings {
        summary.push_str(&format!(" Warning: {}.", warning.message));
    }
    summary
}

/// Where a site is after an edit, with its multiplicity and Wyckoff
/// position, e.g. `O1 at (0.3148, 0.3148, 0.0000), 4 equivalent atoms (4f)`.
fn site_text(document: &CrystalDocument, site: SiteId) -> String {
    let derived = document.derived();
    let Some(position) = document.structure().site(site).map(|s| s.position) else {
        return String::new();
    };
    let [x, y, z] = position;
    let count = derived.multiplicity(site).unwrap_or(0);
    let wyckoff = derived
        .site_symmetry(site)
        .and_then(|s| s.wyckoff.as_ref())
        .map_or_else(String::new, |w| format!(" ({w})"));
    format!(
        "{} at ({x:.4}, {y:.4}, {z:.4}), {count} equivalent {}{wyckoff}",
        label(document.structure(), site),
        if count == 1 { "atom" } else { "atoms" }
    )
}

/// The change of cell a call asks for.
fn cell_transform(
    document: &CrystalDocument,
    input: &TransformCellInput,
) -> Result<CellTransform, CallError> {
    let structure = document.structure();
    let unavailable = |what: &str| {
        CallError::new(
            "no_transformation",
            format!("no {what} could be found for this structure"),
        )
    };
    let preset = match input.cell {
        CellChoice::Primitive => CellPreset::Primitive,
        CellChoice::Conventional => CellPreset::Conventional,
        CellChoice::Niggli => CellPreset::Niggli,
        CellChoice::Setting => {
            let setting = mw_crystal::find_setting(&input.setting).ok_or_else(|| {
                invalid(format!(
                    "{:?} names no tabulated setting; give a Hermann–Mauguin symbol such as \
                     P 1 1 21/a",
                    input.setting
                ))
            })?;
            let current = document.derived().symmetry.space_group.as_ref();
            if current.is_some_and(|sg| sg.number != setting.number) {
                return Err(CallError::new(
                    "other_type",
                    format!(
                        "{} is a setting of another space group type; structure.summary gives the \
                         structure's",
                        setting.hm_symbol
                    ),
                ));
            }
            CellPreset::Setting {
                hall_number: setting.hall_number,
            }
        }
        CellChoice::Matrix => {
            let fraction = |text: &String| {
                Rational::parse(text)
                    .ok_or_else(|| invalid(format!("{text:?} is not a fraction such as 1/2 or -1")))
            };
            let [r0, r1, r2] = <&[[String; 3]; 3]>::try_from(input.matrix.as_slice())
                .map_err(|_| invalid("matrix needs three rows of three fractions"))?;
            let row = |r: &[String; 3]| -> Result<[Rational; 3], CallError> {
                Ok([fraction(&r[0])?, fraction(&r[1])?, fraction(&r[2])?])
            };
            let matrix = [row(r0)?, row(r1)?, row(r2)?];
            let origin = match input.origin.as_slice() {
                [] => [Rational::ZERO; 3],
                [x, y, z] => [fraction(x)?, fraction(y)?, fraction(z)?],
                _ => return Err(invalid("origin needs three fractions, or none")),
            };
            return CellTransform::new(matrix, origin)
                .map_err(|error| EditError::Transform(error).into());
        }
    };
    preset.transform(structure).ok_or_else(|| {
        unavailable(match input.cell {
            CellChoice::Primitive => "primitive cell",
            CellChoice::Conventional => "conventional cell",
            CellChoice::Niggli => "Niggli cell",
            _ => "change of cell to that setting",
        })
    })
}

/// The symmetry search a call asks for (crystallography design §7.10).
fn detect_symmetry(
    structure: &Structure,
    input: &SymmetrizeInput,
) -> Result<DetectSymmetry, CallError> {
    if !(input.tolerance.is_finite() && input.tolerance >= 0.0) {
        return Err(invalid("tolerance must be a positive number of Å, or 0"));
    }
    Ok(DetectSymmetry {
        symprec: if input.tolerance == 0.0 {
            structure.tolerances.symprec
        } else {
            input.tolerance
        },
        standardize: input.standardize,
    })
}

fn symmetrization(result: &Symmetrization) -> SymmetrizationOutput {
    let document = CrystalDocument::new(result.structure.clone());
    let structure = document.structure();
    SymmetrizationOutput {
        detected: DetectedOutput {
            number: result.detected_number,
            symbol: result.detected_symbol.into(),
        },
        space_group: result.space_group.as_ref().map(space_group),
        sites: structure.sites.len(),
        atoms_in_cell: document.derived().atoms_in_cell(),
        operations: structure.symmetry.operations().len(),
        max_displacement: result.max_displacement,
        moved_most: result
            .moved_most
            .map_or_else(String::new, |site| label(structure, site)),
        cell_changed: result.cell_changed,
        orientation_changed: result.orientation_changed,
        origin_shift: result.origin_shift,
        cell: cell(&document),
    }
}

/// E.g. `P m -3 m (No. 221): 3 sites, 5 atoms in the cell; atoms move at
/// most 0.0040 Å (O1); the cell stays.`
fn symmetrization_text(output: &SymmetrizationOutput) -> String {
    let group = output.space_group.as_ref().map_or_else(
        || "an unidentified space group".to_string(),
        |sg| format!("{} (No. {})", sg.symbol, sg.number),
    );
    let subgroup = match &output.space_group {
        Some(sg) if sg.number != output.detected.number => format!(
            ", a subgroup of the detected {} (No. {}) that the cell can hold",
            output.detected.symbol, output.detected.number
        ),
        _ => String::new(),
    };
    let moved = if output.moved_most.is_empty() {
        String::new()
    } else {
        format!(" ({})", output.moved_most)
    };
    let cell_change = if output.cell_changed {
        format!("the cell becomes {}", cell_text(&output.cell))
    } else {
        "the cell stays".into()
    };
    let origin = if output.origin_shift > 0.0 {
        format!("; the origin moves {:.4} Å", output.origin_shift)
    } else {
        String::new()
    };
    format!(
        "{group}{subgroup}: {} sites, {} atoms in the cell; atoms move at most {:.4} Å{moved}; \
         {cell_change}{origin}.",
        output.sites, output.atoms_in_cell, output.max_displacement
    )
}

fn space_group(space_group: &SpaceGroupType) -> SpaceGroupOutput {
    SpaceGroupOutput {
        number: space_group.number,
        symbol: space_group.hm_symbol.into(),
        setting: match &space_group.setting {
            Setting::Standard => "standard".into(),
            Setting::Tabulated { symbol, code } => format!("{symbol} ({code})"),
            Setting::Transformed => "transformed".into(),
        },
    }
}

fn cell(document: &CrystalDocument) -> CellOutput {
    let params = document.structure().lattice.params();
    let constraints = &document.derived().symmetry.cell;
    let value = |param| params.get(param).value();
    let mut fixed = Vec::new();
    let mut linked = Vec::new();
    for param in CellParam::ALL {
        match constraints.get(param) {
            Constraint::Free => {}
            Constraint::Fixed => fixed.push(param),
            Constraint::Equals(equals) => linked.push(LinkOutput {
                parameter: param,
                equals,
            }),
        }
    }
    CellOutput {
        a: value(CellParam::A),
        b: value(CellParam::B),
        c: value(CellParam::C),
        alpha: value(CellParam::Alpha),
        beta: value(CellParam::Beta),
        gamma: value(CellParam::Gamma),
        volume: params.volume(),
        fixed,
        linked,
    }
}

fn cell_text(cell: &CellOutput) -> String {
    format!(
        "a = {:.4} Å, b = {:.4} Å, c = {:.4} Å, α = {:.3}°, β = {:.3}°, γ = {:.3}°",
        cell.a, cell.b, cell.c, cell.alpha, cell.beta, cell.gamma
    )
}

/// Checks that measured atoms exist and differ.
fn measured(document: &CrystalDocument, atoms: &[AtomRef], count: usize) -> Result<(), CallError> {
    if atoms.len() != count {
        return Err(invalid(format!(
            "expected {count} atoms, got {}",
            atoms.len()
        )));
    }
    for (i, atom) in atoms.iter().enumerate() {
        if document.derived().fractional(*atom).is_none() {
            return Err(EditError::UnknownAtom(*atom).into());
        }
        if atoms[..i].contains(atom) {
            return Err(EditError::RepeatedAtom(*atom).into());
        }
    }
    Ok(())
}

fn atom_text(structure: &Structure, atom: AtomRef) -> String {
    format!(
        "{} (member {}, cell {:?})",
        label(structure, atom.site),
        atom.member,
        atom.cell
    )
}

pub(crate) fn register(registry: &mut Registry) {
    let query = |name, description| Spec::new(name, description, Effect::ReadOnly);
    let edit = |name, description| Spec::new(name, description, Effect::Undoable);

    registry.register(
        query(
            "structure.summary",
            "Summarize one structure: formula and Z, space group and setting, edit mode, cell \
             parameters with the ones symmetry fixes or links, volume, density, and the numbers \
             of sites, atoms and symmetry operations. In symmetry mode every edit applies to whole \
             sites (all equivalent atoms at once); in P1 mode every atom is its own site.",
        ),
        |host: &mut dyn Host, input: StructureInput| {
            let name = host
                .structures()
                .into_iter()
                .find(|info| info.id == input.structure)
                .map(|info| info.name)
                .unwrap_or_default();
            let document = document(host, input.structure)?;
            let (structure, derived) = (document.structure(), document.derived());
            let formula = Formula::of(structure, derived);
            let analysis = &derived.symmetry;
            let output = SummaryOutput {
                name,
                formula: formula.to_string(),
                formula_units: formula.z,
                edit_mode: match document.edit_mode() {
                    EditMode::Symmetric => ModeOutput::Symmetry,
                    EditMode::P1 => ModeOutput::P1,
                },
                space_group: analysis.space_group.as_ref().map(space_group),
                cell: cell(document),
                density: derived.density,
                sites: structure.sites.len(),
                atoms_in_cell: derived.atoms_in_cell(),
                operations: structure.symmetry.operations().len(),
                detected_space_group: analysis.detected.as_ref().map(|d| DetectedOutput {
                    number: d.number,
                    symbol: d.hm_symbol.into(),
                }),
            };
            let group = output.space_group.as_ref().map_or_else(
                || "space group not identified".to_string(),
                |sg| format!("{} (No. {})", sg.symbol, sg.number),
            );
            let mode = match output.edit_mode {
                ModeOutput::Symmetry => "symmetry mode",
                ModeOutput::P1 => "P1 mode",
            };
            let summary = format!(
                "{}, Z = {}, {group}, {mode}; {}; {} sites, {} atoms in the cell.",
                output.formula,
                output.formula_units,
                cell_text(&output.cell),
                output.sites,
                output.atoms_in_cell
            );
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        query(
            "structure.sites",
            "List the sites of the asymmetric unit, a page at a time: id, label, fractional \
             position, multiplicity (equivalent atoms in the unit cell), Wyckoff position, the \
             coordinate form its special position allows (e.g. x, x, 0) and its occupants with \
             element and occupancy. Other tools name sites by id and occupants by label.",
        ),
        |host: &mut dyn Host, input: SitesInput| {
            if !(1..=MAX_SITES).contains(&input.limit) {
                return Err(invalid(format!("limit must be from 1 to {MAX_SITES}")));
            }
            let document = document(host, input.structure)?;
            let (structure, derived) = (document.structure(), document.derived());
            let sites: Vec<SiteOutput> = structure
                .sites
                .iter()
                .skip(input.offset as usize)
                .take(input.limit as usize)
                .map(|site| {
                    let symmetry = derived.site_symmetry(site.id);
                    SiteOutput {
                        site: site.id,
                        label: site.label().into(),
                        position: site.position,
                        multiplicity: derived.multiplicity(site.id).unwrap_or(0),
                        wyckoff: symmetry
                            .and_then(|s| s.wyckoff.as_ref())
                            .map(ToString::to_string),
                        coordinate_form: symmetry.map_or_else(String::new, |s| s.form.to_string()),
                        occupants: site
                            .occupants
                            .iter()
                            .map(|o| OccupantOutput {
                                label: o.label.clone(),
                                element: o.species.element.clone(),
                                symbol: o.species.symbol.clone(),
                                occupancy: o.occupancy.value(),
                            })
                            .collect(),
                    }
                })
                .collect();
            let total = structure.sites.len();
            let summary = format!(
                "Sites {} to {} of {total}: {}.",
                input.offset as usize + usize::from(!sites.is_empty()),
                input.offset as usize + sites.len(),
                sites
                    .iter()
                    .map(|s| format!(
                        "{} ({}, {} atoms)",
                        s.label,
                        s.occupants
                            .iter()
                            .map(|o| o.element.as_str())
                            .collect::<Vec<_>>()
                            .join("/"),
                        s.multiplicity
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            Ok(Output::new(SitesOutput { total, sites }, summary))
        },
    );

    registry.register(
        query(
            "structure.site_symmetry",
            "Describe one site's symmetry: the order of its site-symmetry group, its \
             multiplicity and Wyckoff position, and how symmetry constrains each fractional \
             coordinate: free, following another free coordinate, or fixed.",
        ),
        |host: &mut dyn Host, input: SiteInput| {
            let document = document(host, input.structure)?;
            let (structure, derived) = (document.structure(), document.derived());
            let site = structure
                .site(input.site)
                .ok_or(EditError::UnknownSite(input.site))?;
            let symmetry = derived
                .site_symmetry(site.id)
                .ok_or(EditError::UnknownSite(input.site))?;
            let coordinates = Axis::ALL
                .iter()
                .zip(symmetry.coordinates)
                .zip(["x", "y", "z"])
                .map(|((axis, constraint), name)| CoordinateOutput {
                    axis: name.into(),
                    constraint: match constraint {
                        CoordinateConstraint::Free => CoordinateKind::Free,
                        CoordinateConstraint::Follows(_) => CoordinateKind::Follows,
                        CoordinateConstraint::Combination => CoordinateKind::Combination,
                        CoordinateConstraint::Fixed => CoordinateKind::Fixed,
                    },
                    expression: symmetry.form.expression(*axis),
                })
                .collect();
            let output = SiteSymmetryOutput {
                site: site.id,
                label: site.label().into(),
                order: symmetry.order,
                multiplicity: derived.multiplicity(site.id).unwrap_or(0),
                wyckoff: symmetry.wyckoff.as_ref().map(|w| WyckoffOutput {
                    letter: w.letter.to_string(),
                    multiplicity: w.multiplicity,
                    site_symmetry: w.site_symmetry.into(),
                }),
                coordinates,
            };
            let summary = format!(
                "{}: site symmetry of order {}, {} equivalent atoms, {}coordinates {}.",
                output.label,
                output.order,
                output.multiplicity,
                symmetry
                    .wyckoff
                    .as_ref()
                    .map_or_else(String::new, |w| format!(
                        "Wyckoff {w} ({}), ",
                        w.site_symmetry
                    )),
                symmetry.form
            );
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        query(
            "structure.detect_symmetry",
            "Find the symmetry the structure's atoms have, which can be higher than its own \
             operations (a structure written out in P1, a supercell), and preview symmetrizing \
             by it: every atom averaged onto its symmetric position, equivalent atoms merged \
             into one site, occupancies and displacement parameters averaged. Gives the space \
             group type found, the result's space group, sites and atoms, the largest distance \
             an atom moves, and whether the cell, its orientation or the origin change. Atoms of \
             different elements, occupancies or disorder groups are never equivalent. Keeps the \
             cell, or with standardize uses the conventional cell of the standard setting. \
             Nothing changes.",
        ),
        |host: &mut dyn Host, input: SymmetrizeInput| {
            let document = document(host, input.structure)?;
            let detect = detect_symmetry(document.structure(), &input)?;
            let result = detect
                .run(document.structure())
                .map_err(EditError::Symmetrize)?;
            let output = symmetrization(&result);
            let summary = format!("Symmetrizing gives {}", symmetrization_text(&output));
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        query(
            "structure.measure_distance",
            "Measure the distance in Å between two atoms. An atom is a site id, which of its \
             equivalent atoms (member) and a lattice cell, as structure.neighbors gives them.",
        ),
        |host: &mut dyn Host, input: AtomsInput| {
            let document = document(host, input.structure)?;
            let atoms = &input.atoms.0;
            measured(document, atoms, 2)?;
            let structure = document.structure();
            let distance = Measurement::Distance([atoms[0], atoms[1]])
                .value(&structure.lattice, document.derived())
                .expect("checked atoms");
            let summary = format!(
                "{} to {}: {distance:.4} Å.",
                atom_text(structure, atoms[0]),
                atom_text(structure, atoms[1])
            );
            Ok(Output::new(DistanceOutput { distance }, summary))
        },
    );

    registry.register(
        query(
            "structure.measure_angle",
            "Measure the angle in degrees between three atoms, at the second one.",
        ),
        |host: &mut dyn Host, input: AtomsInput| {
            let document = document(host, input.structure)?;
            let atoms = &input.atoms.0;
            measured(document, atoms, 3)?;
            let structure = document.structure();
            let angle = Measurement::Angle([atoms[0], atoms[1], atoms[2]])
                .value(&structure.lattice, document.derived())
                .expect("checked atoms");
            let summary = format!("Angle at {}: {angle:.2}°.", atom_text(structure, atoms[1]));
            Ok(Output::new(AngleOutput { angle }, summary))
        },
    );

    registry.register(
        query(
            "structure.neighbors",
            "List the atoms within a cutoff distance of an atom, nearest first, with labels, \
             elements and distances in Å; for coordination environments. Returns at most 100 \
             atoms; the cutoff is at most 10 Å.",
        ),
        |host: &mut dyn Host, input: NeighborsInput| {
            if !(input.cutoff > 0.0 && input.cutoff <= MAX_CUTOFF) {
                return Err(invalid(format!(
                    "cutoff must be more than 0 and at most {MAX_CUTOFF} Å"
                )));
            }
            let document = document(host, input.structure)?;
            let structure = document.structure();
            let mut found = document
                .derived()
                .neighbors(&structure.lattice, input.atom, input.cutoff)
                .ok_or(EditError::UnknownAtom(input.atom))?;
            let truncated = found.len() > MAX_NEIGHBORS;
            found.truncate(MAX_NEIGHBORS);
            let neighbors: Vec<NeighborOutput> = found
                .iter()
                .map(|n| {
                    let site = structure.site(n.atom.site);
                    NeighborOutput {
                        atom: n.atom,
                        label: site.map_or_else(String::new, |s| s.label().into()),
                        element: site.map_or_else(String::new, |s| s.element().into()),
                        distance: n.distance,
                    }
                })
                .collect();
            let summary = format!(
                "{} atoms within {} Å of {}{}",
                neighbors.len(),
                input.cutoff,
                atom_text(structure, input.atom),
                if neighbors.is_empty() {
                    ".".to_string()
                } else {
                    format!(
                        ": {}{}.",
                        neighbors
                            .iter()
                            .map(|n| format!("{} {:.3} Å", n.label, n.distance))
                            .collect::<Vec<_>>()
                            .join(", "),
                        if truncated { ", …" } else { "" }
                    )
                }
            );
            Ok(Output::new(
                NeighborsOutput {
                    neighbors,
                    truncated,
                },
                summary,
            ))
        },
    );

    registry.register(
        edit(
            "structure.set_cell",
            "Change cell parameters: lengths in Å, angles in degrees. Fractional coordinates \
             stay, so the atoms scale with the cell. Under symmetry, a parameter linked to \
             another changes with it (setting a of a tetragonal cell sets b too) and a fixed one, \
             such as a 90° angle, cannot change; structure.summary lists both. The result always \
             keeps the symmetry, so it also aligns a cell imported slightly off it. Undoable.",
        )
        .idempotent(),
        |host: &mut dyn Host, input: SetCellInput| {
            let mut values = CellValues::default();
            for item in &input.values.0 {
                if values.get(item.parameter).is_some() {
                    return Err(invalid("a parameter is given twice"));
                }
                values = values.with(item.parameter, item.value);
            }
            let (document, outcome) = run(host, input.structure, Edit::SetCell { values })?;
            let output = edit_output(document, outcome, Vec::new());
            let summary = edit_summary(
                &output,
                format!("The cell is now {}.", cell_text(&cell(document))),
            );
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.set_element",
            "Replace the element of an occupant, named by its label, on every equivalent atom \
             of its site. The label stays; rename it with structure.rename_label. Replacing only \
             some equivalent atoms would break the symmetry and is refused; the error suggests \
             replacing all of them or converting the structure to P1. Undoable.",
        )
        .idempotent(),
        |host: &mut dyn Host, input: SetElementInput| {
            let element = element_symbol(&input.element);
            let document = document(host, input.structure)?;
            let site = occupant_site(document.structure(), &input.occupant)?;
            whole_site(document, site, &input.atoms)?;
            let edit = Edit::SetSpecies {
                site,
                occupant: input.occupant.clone(),
                species: Species::element(&element),
            };
            let (document, outcome) = run(host, input.structure, edit)?;
            let count = document.derived().multiplicity(site).unwrap_or(0);
            let output = edit_output(document, outcome, vec![site]);
            let summary = edit_summary(
                &output,
                format!(
                    "{} is now {element} on all {count} equivalent atoms.",
                    input.occupant
                ),
            );
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.set_occupancy",
            "Set the occupancy of an occupant, named by its label, on every equivalent atom of \
             its site; more than 0 and at most 1. Warns when a site's occupancies add up to more \
             than 1.01. Changing only some equivalent atoms would break the symmetry and is \
             refused. Undoable.",
        )
        .idempotent(),
        |host: &mut dyn Host, input: SetOccupancyInput| {
            let document = document(host, input.structure)?;
            let site = occupant_site(document.structure(), &input.occupant)?;
            whole_site(document, site, &input.atoms)?;
            let edit = Edit::SetOccupancy {
                site,
                occupant: input.occupant.clone(),
                occupancy: Measured::exact(input.occupancy),
            };
            let (document, outcome) = run(host, input.structure, edit)?;
            let output = edit_output(document, outcome, vec![site]);
            let summary = edit_summary(
                &output,
                format!(
                    "The occupancy of {} is now {}.",
                    input.occupant, input.occupancy
                ),
            );
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.add_occupant",
            "Add an element to a site at the given occupancy, making the site's occupancy mixed. \
             For example Fe to Fe0.9Ni0.1: add Ni at 0.1, then set Fe to 0.9 with \
             structure.set_occupancy. The new occupant's label is its element symbol and the \
             smallest unused number, e.g. Ni1. Undoable.",
        )
        .additive(),
        |host: &mut dyn Host, input: AddOccupantInput| {
            let element = element_symbol(&input.element);
            let edit = Edit::AddOccupant {
                site: input.site,
                species: Species::element(&element),
                occupancy: Measured::exact(input.occupancy),
                label: None,
            };
            let (document, outcome) = run(host, input.structure, edit)?;
            let structure = document.structure();
            let new_label = structure
                .site(input.site)
                .and_then(|site| site.occupants.last())
                .map_or_else(String::new, |o| o.label.clone());
            let output = edit_output(document, outcome, vec![input.site]);
            let summary = edit_summary(
                &output,
                format!(
                    "Added {new_label} ({element}, occupancy {}) to {}.",
                    input.occupancy,
                    label(structure, input.site)
                ),
            );
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.remove_occupant",
            "Remove an occupant, named by its label, from a site with mixed occupancy. A site \
             keeps at least one occupant: to remove the last one, delete the site. Undoable.",
        ),
        |host: &mut dyn Host, input: OccupantInput| {
            let site = occupant_site(
                document(host, input.structure)?.structure(),
                &input.occupant,
            )?;
            let edit = Edit::RemoveOccupant {
                site,
                occupant: input.occupant.clone(),
            };
            let (document, outcome) = run(host, input.structure, edit)?;
            let output = edit_output(document, outcome, vec![site]);
            let summary = edit_summary(&output, format!("Removed {}.", input.occupant));
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.rename_label",
            "Rename an occupant's label. Labels are unique in a structure. Undoable.",
        )
        .idempotent(),
        |host: &mut dyn Host, input: RenameInput| {
            let site = occupant_site(
                document(host, input.structure)?.structure(),
                &input.occupant,
            )?;
            let edit = Edit::RenameLabel {
                site,
                occupant: input.occupant.clone(),
                label: input.label.clone(),
            };
            let (document, outcome) = run(host, input.structure, edit)?;
            let output = edit_output(document, outcome, vec![site]);
            let summary = edit_summary(
                &output,
                format!("Renamed {} to {}.", input.occupant, input.label),
            );
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.delete_sites",
            "Delete sites with all their equivalent atoms, named by id in sites or by atoms in \
             atoms. Under symmetry, atoms stand for their site only when they are all its \
             equivalent atoms (or the site has one atom); deleting some would break the symmetry \
             and is refused, with the choice of deleting the whole site or converting the \
             structure to P1. Undoable.",
        ),
        |host: &mut dyn Host, input: DeleteSitesInput| {
            let targets: Vec<Target> = input
                .sites
                .iter()
                .copied()
                .map(Target::Site)
                .chain(input.atoms.iter().copied().map(Target::Atom))
                .collect();
            if targets.is_empty() {
                return Err(invalid("name at least one site or atom"));
            }
            let document = document(host, input.structure)?;
            let sites = document.target_sites(&targets)?;
            let deleted: Vec<String> = sites
                .iter()
                .map(|&site| {
                    let count = document.derived().multiplicity(site).unwrap_or(0);
                    format!(
                        "{} ({count} {})",
                        label(document.structure(), site),
                        if count == 1 { "atom" } else { "atoms" }
                    )
                })
                .collect();
            let mut edits: Vec<Edit> = sites
                .iter()
                .map(|&site| Edit::RemoveSite { site })
                .collect();
            let edit = if edits.len() == 1 {
                edits.remove(0)
            } else {
                Edit::Batch { edits }
            };
            let (document, outcome) = run(host, input.structure, edit)?;
            let output = edit_output(document, outcome, sites);
            let summary = edit_summary(&output, format!("Deleted {}.", deleted.join(", ")));
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.set_position",
            "Move a site to new fractional coordinates. Under symmetry all its equivalent \
             atoms move with it and the site takes the site symmetry of the new position: \
             within 0.01 Å of a special position it snaps onto it, so its number of equivalent \
             atoms can change, as info reports. Warns when an atom comes closer to another than the \
             short-contact distance, 0.5 Å by default. Undoable.",
        )
        .idempotent(),
        |host: &mut dyn Host, input: SetPositionInput| {
            let edit = Edit::SetPosition {
                site: input.site,
                position: input.position,
            };
            let (document, outcome) = run(host, input.structure, edit)?;
            let output = edit_output(document, outcome, vec![input.site]);
            let summary = edit_summary(
                &output,
                format!("Moved {}.", site_text(document, input.site)),
            );
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.move_site",
            "Drag an atom by a fractional displacement, as with the mouse; all its equivalent \
             atoms move with it. With leave_special_position false, the site stays on its \
             special position: only the part of the displacement along it counts, the nearest \
             allowed move. With true, it moves by the whole displacement and may leave the \
             special position, which gives it more equivalent atoms. Either way, a site that \
             ends within 0.01 Å of a special position with more symmetry snaps onto it. Changes \
             in the number of equivalent atoms are reported in info. Warns when atoms come \
             closer than 0.5 Å. Undoable.",
        ),
        |host: &mut dyn Host, input: MoveSiteInput| {
            let site = input.atom.site;
            let edit = Edit::MoveSite {
                atom: input.atom,
                displacement: input.displacement,
                leave_special_position: input.leave_special_position,
            };
            let (document, outcome) = run(host, input.structure, edit)?;
            let output = edit_output(document, outcome, vec![site]);
            let summary = edit_summary(&output, format!("Moved {}.", site_text(document, site)));
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.set_coordinate",
            "Set one fractional coordinate of a site, keeping the site on its special \
             position: a free coordinate, or one that follows a single free coordinate, as \
             structure.site_symmetry reports. The coordinates that depend on it follow: setting \
             x of x, 2x, 1/4 sets y to 2x, and setting y sets x to y/2. A fixed coordinate \
             (fixed_coordinate) or a combination of free ones (combined_coordinate) cannot be \
             set; structure.set_position moves a site off its special position. Snaps onto a \
             special position with more symmetry within 0.01 Å. Undoable.",
        )
        .idempotent(),
        |host: &mut dyn Host, input: SetCoordinateInput| {
            let edit = Edit::SetCoordinate {
                site: input.site,
                axis: input.axis,
                value: input.value,
            };
            let (document, outcome) = run(host, input.structure, edit)?;
            let output = edit_output(document, outcome, vec![input.site]);
            let summary = edit_summary(
                &output,
                format!(
                    "Set {} = {}: {}.",
                    input.axis.name(),
                    input.value,
                    site_text(document, input.site)
                ),
            );
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.transform_cell",
            "Describe the structure in another cell (International Tables A): the new basis \
             is A·P and the new origin lies at p in the old basis, so fractional coordinates \
             become P⁻¹·(x − p). The atoms stay where they are; the asymmetric unit keeps its \
             sites, ids and labels, and their equivalent atoms in a cell grow or shrink in \
             number with the cell. A larger cell adds the old lattice translations as \
             centering, a smaller one must consist of translations of the structure \
             (transform_not_a_translation), and every symmetry operation must map the new \
             lattice onto itself (transform_breaks_symmetry). Choose the primitive, \
             conventional or Niggli cell, another tabulated setting (unique axis, cell choice, \
             axis order, origin choice, hexagonal or rhombohedral axes), or give P and p. \
             Atom references and measurements follow the atoms. Undoable.",
        ),
        |host: &mut dyn Host, input: TransformCellInput| {
            let document = document(host, input.structure)?;
            let before = document.derived().atoms_in_cell();
            let transform = cell_transform(document, &input)?;
            let (document, outcome) =
                run(host, input.structure, Edit::TransformCell { transform })?;
            let derived = document.derived();
            let text = |r: &Rational| r.to_string();
            let output = TransformCellOutput {
                changed: outcome.changed,
                revision: document.revision(),
                transformation: transform.to_string(),
                matrix: transform
                    .matrix()
                    .iter()
                    .map(|row| row.each_ref().map(text))
                    .collect(),
                origin: transform.origin().iter().map(text).collect(),
                space_group: derived.symmetry.space_group.as_ref().map(space_group),
                cell: cell(document),
                atoms_in_cell: derived.atoms_in_cell(),
                operations: document.structure().symmetry.operations().len(),
            };
            let summary = if output.changed {
                let group = output.space_group.as_ref().map_or_else(String::new, |sg| {
                    format!(" in {} ({})", sg.symbol, sg.setting)
                });
                format!(
                    "Changed the cell ({}): {}{group}, {} atoms in the cell instead of {before}.",
                    output.transformation,
                    cell_text(&output.cell),
                    output.atoms_in_cell
                )
            } else {
                "Nothing changed: the structure already is in that cell.".into()
            };
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.symmetrize",
            "Symmetrize the structure by the symmetry its atoms have, as \
             structure.detect_symmetry previews it: the operations found within the tolerance, \
             every atom averaged onto its symmetric position, equivalent atoms merged into one \
             site (it keeps the label of its first atom, or O1 for O1_1, O1_2, …), occupancies \
             and displacement parameters averaged, the cell parameters made to fit. Keeps the \
             cell, or with standardize uses the conventional cell of the standard setting. \
             Refused when the atoms have no more symmetry than the structure \
             (symmetrize_no_higher_symmetry); another tolerance may help with \
             symmetrize_not_found or symmetrize_mismatch. Atom references and measurements follow \
             the atoms to the nearest atom. Undoable.",
        ),
        |host: &mut dyn Host, input: SymmetrizeInput| {
            let document = document(host, input.structure)?;
            let detect = detect_symmetry(document.structure(), &input)?;
            let result = detect
                .run(document.structure())
                .map_err(EditError::Symmetrize)?;
            let edit = Edit::Symmetrize {
                symprec: (input.tolerance != 0.0).then_some(input.tolerance),
                standardize: input.standardize,
            };
            let (document, outcome) = run(host, input.structure, edit)?;
            let output = SymmetrizeOutput {
                changed: outcome.changed,
                revision: document.revision(),
                result: symmetrization(&result),
            };
            let summary = if output.changed {
                format!("Symmetrized: {}", symmetrization_text(&output.result))
            } else {
                "Nothing changed: the structure already was symmetric.".into()
            };
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.add_site",
            "Add a site to the asymmetric unit: an element at fractional coordinates, with an \
             occupancy, and all the atoms symmetry makes of it. Under symmetry it snaps onto \
             the special position within 0.01 Å, which sets its number of equivalent atoms. A \
             position on an atom of another site is refused (coincident_site): add the element \
             to that site with structure.add_occupant instead. The label is the element symbol \
             and the smallest unused number, e.g. O2. Warns when atoms come closer than 0.5 Å. \
             Undoable.",
        )
        .additive(),
        |host: &mut dyn Host, input: AddSiteInput| {
            let element = element_symbol(&input.element);
            let edit = Edit::AddSite {
                position: input.position,
                species: Species::element(&element),
                occupancy: Measured::exact(input.occupancy),
                label: None,
            };
            let (document, outcome) = run(host, input.structure, edit)?;
            // A new site goes to the end of the asymmetric unit.
            let site = document.structure().sites.last().map(|s| s.id);
            let output = edit_output(document, outcome, site.into_iter().collect());
            let summary = edit_summary(
                &output,
                format!(
                    "Added {}.",
                    site.map_or_else(String::new, |site| site_text(document, site))
                ),
            );
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.convert_to_p1",
            "Convert the structure to P1: every atom of the unit cell becomes a site of its own, \
             labelled after its site (Ti1_1, Ti1_2, …; a site with one atom keeps its label), \
             and the space group becomes P 1. Then single atoms can be edited, replaced or \
             deleted, which symmetry mode refuses. Atom references and measurements follow the \
             atoms. A structure in P1 already is refused (already_p1). structure.symmetrize goes \
             back to a symmetric structure. Undoable.",
        )
        .idempotent(),
        |host: &mut dyn Host, input: StructureInput| {
            let (document, outcome) = run(host, input.structure, Edit::ExpandToP1)?;
            let sites = document.structure().sites.len();
            let output = edit_output(document, outcome, Vec::new());
            let summary = edit_summary(
                &output,
                format!("Converted to P1: {sites} sites, one per atom."),
            );
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.make_supercell",
            "Make a supercell of a structure as a new structure of the same project, placed \
             after it and open: the basis vectors are A·M for the integer matrix M, the \
             structure is in P1, and each atom's label gets the number of its image (Ti1_1 … \
             Ti1_16). Partial occupancies, displacement parameters and uncertainties carry over; \
             the original stays as it was, and measurements stay with it. At most 100000 atoms. \
             Without the application's window the project file is saved at once.",
        )
        .additive(),
        |host: &mut dyn Host, input: MakeSupercellInput| {
            let source = document(host, input.structure)?;
            let det = determinant(&input.matrix);
            if det <= 0 {
                return Err(invalid(format!(
                    "the matrix has determinant {det}; it must be positive"
                )));
            }
            let atoms = source.derived().atoms_in_cell() as u128 * det as u128;
            if atoms > MAX_SUPERCELL_ATOMS as u128 {
                return Err(CallError::new(
                    "too_large",
                    format!(
                        "the supercell would have {atoms} atoms, more than \
                         {MAX_SUPERCELL_ATOMS}; choose a smaller matrix"
                    ),
                ));
            }
            let structure = make_supercell(source.structure(), input.matrix)
                .map_err(|error| invalid(error.to_string()))?;
            let name = (!input.name.trim().is_empty()).then(|| input.name.trim().to_owned());
            let projects = host.projects_mut().ok_or_else(|| {
                CallError::new(
                    "unavailable",
                    "structure.make_supercell needs a project to add the supercell to",
                )
            })?;
            let id =
                projects.add_derived(input.structure, name.as_deref(), structure, input.matrix)?;
            let name = host
                .projects()
                .and_then(|p| p.entries().into_iter().find(|e| e.structure == id))
                .map_or_else(String::new, |e| e.name);
            let document = document(host, id)?;
            let output = MakeSupercellOutput {
                structure: id,
                name,
                source: input.structure,
                cells: det,
                sites: document.structure().sites.len(),
                cell: cell(document),
            };
            let summary = format!(
                "Made {} (id {}): {} cells, {} atoms; {}.",
                output.name,
                id.0,
                output.cells,
                output.sites,
                cell_text(&output.cell)
            );
            Ok(Output::new(output, summary))
        },
    );
}

#[cfg(test)]
mod tests {
    use mw_crystal::element;

    use super::*;

    #[test]
    fn element_symbols_get_their_usual_case() {
        assert_eq!(element_symbol("zr"), "Zr");
        assert_eq!(element_symbol(" ZR "), "Zr");
        assert_eq!(element_symbol("O"), "O");
        assert!(element::is_element(&element_symbol("ni")));
        assert_eq!(element_symbol(""), "");
    }
}
