//! Structure queries and edits (ai-integration design §5.2): the same
//! domain commands the interface runs (crystallography design §7.2), named
//! by stable ids.

use mw_crystal::{
    AtomRef, Axis, CellParam, CellValues, Constraint, CoordinateConstraint, CrystalDocument, Edit,
    EditError, EditMode, Formula, Measured, Measurement, Outcome, Setting, SiteId, SpaceGroupType,
    Species, Structure, Target,
};

use crate::registry::{
    Availability, CallError, Effect, Host, Output, Registry, Spec, StructureEntryId,
};
use crate::schema::{NonEmpty, schema_struct, string_enum};

/// Neighbors returned at most.
const MAX_NEIGHBORS: usize = 100;
/// Å; the neighbor search grows with the cube of the cutoff.
const MAX_CUTOFF: f64 = 10.0;
/// Sites per page.
const MAX_SITES: u32 = 500;

schema_struct! {
    pub struct NoInput {}
}

schema_struct! {
    pub struct StructureInput {
        /// The structure's id, from structure.list.
        pub structure: StructureEntryId,
    }
}

schema_struct! {
    pub struct ListOutput {
        pub structures: Vec<ListEntry>,
    }
}

schema_struct! {
    pub struct ListEntry {
        /// Id to name the structure by in other calls.
        pub structure: StructureEntryId,
        pub name: String,
        pub project: String,
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
    pub struct WarningOutput {
        /// Stable, e.g. occupancy_sum or short_contact.
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
    }
}

const STRUCTURE_ID_HINT: &str = "structure.list gives the ids";

pub(crate) fn document(
    host: &dyn Host,
    id: StructureEntryId,
) -> Result<&CrystalDocument, CallError> {
    host.document(id).ok_or_else(|| unknown_structure(id))
}

pub(crate) fn document_mut(
    host: &mut dyn Host,
    id: StructureEntryId,
) -> Result<&mut CrystalDocument, CallError> {
    host.document_mut(id).ok_or_else(|| unknown_structure(id))
}

fn unknown_structure(id: StructureEntryId) -> CallError {
    CallError::new(
        "unknown_structure",
        format!("no structure with id {}; {STRUCTURE_ID_HINT}", id.0),
    )
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
    }
}

/// The summary of an edit, with its warnings.
fn edit_summary(output: &EditOutput, done: String) -> String {
    let mut summary = if output.changed {
        done
    } else {
        "Nothing changed: the structure already was as asked.".into()
    };
    for warning in &output.warnings {
        summary.push_str(&format!(" Warning: {}.", warning.message));
    }
    summary
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
    let query = |name, description| Spec {
        name,
        description,
        effect: Effect::ReadOnly,
        availability: Availability::Both,
    };
    let edit = |name, description| Spec {
        name,
        description,
        effect: Effect::Undoable,
        availability: Availability::Both,
    };

    registry.register(
        query(
            "structure.list",
            "List the structures that can be named by id, with their names and projects. \
             Call this first: other structure tools take these ids.",
        ),
        |host: &mut dyn Host, _: NoInput| {
            let structures: Vec<ListEntry> = host
                .structures()
                .into_iter()
                .map(|info| ListEntry {
                    structure: info.id,
                    name: info.name,
                    project: info.project,
                })
                .collect();
            let summary = format!("{} structures.", structures.len());
            Ok(Output::new(ListOutput { structures }, summary))
        },
    );

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
             such as a 90° angle, cannot change; structure.summary lists both. Undoable.",
        ),
        |host: &mut dyn Host, input: SetCellInput| {
            let mut values = CellValues::default();
            for item in &input.values.0 {
                if values.get(item.parameter).is_some() {
                    return Err(invalid("a parameter is given twice"));
                }
                values = values.with(item.parameter, item.value);
            }
            let document = document_mut(host, input.structure)?;
            let outcome = document.edit(Edit::SetCell { values })?;
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
        ),
        |host: &mut dyn Host, input: SetElementInput| {
            let element = element_symbol(&input.element);
            let document = document_mut(host, input.structure)?;
            let site = occupant_site(document.structure(), &input.occupant)?;
            whole_site(document, site, &input.atoms)?;
            let outcome = document.edit(Edit::SetSpecies {
                site,
                occupant: input.occupant.clone(),
                species: Species::element(&element),
            })?;
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
        ),
        |host: &mut dyn Host, input: SetOccupancyInput| {
            let document = document_mut(host, input.structure)?;
            let site = occupant_site(document.structure(), &input.occupant)?;
            whole_site(document, site, &input.atoms)?;
            let outcome = document.edit(Edit::SetOccupancy {
                site,
                occupant: input.occupant.clone(),
                occupancy: Measured::exact(input.occupancy),
            })?;
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
        ),
        |host: &mut dyn Host, input: AddOccupantInput| {
            let element = element_symbol(&input.element);
            let document = document_mut(host, input.structure)?;
            let outcome = document.edit(Edit::AddOccupant {
                site: input.site,
                species: Species::element(&element),
                occupancy: Measured::exact(input.occupancy),
                label: None,
            })?;
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
            let document = document_mut(host, input.structure)?;
            let site = occupant_site(document.structure(), &input.occupant)?;
            let outcome = document.edit(Edit::RemoveOccupant {
                site,
                occupant: input.occupant.clone(),
            })?;
            let output = edit_output(document, outcome, vec![site]);
            let summary = edit_summary(&output, format!("Removed {}.", input.occupant));
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.rename_label",
            "Rename an occupant's label. Labels are unique in a structure. Undoable.",
        ),
        |host: &mut dyn Host, input: RenameInput| {
            let document = document_mut(host, input.structure)?;
            let site = occupant_site(document.structure(), &input.occupant)?;
            let outcome = document.edit(Edit::RenameLabel {
                site,
                occupant: input.occupant.clone(),
                label: input.label.clone(),
            })?;
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
            let document = document_mut(host, input.structure)?;
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
            let outcome = document.edit(edit)?;
            let output = edit_output(document, outcome, sites);
            let summary = edit_summary(&output, format!("Deleted {}.", deleted.join(", ")));
            Ok(Output::new(output, summary))
        },
    );

    registry.register(
        edit(
            "structure.set_position",
            "Move a site to new fractional coordinates. Only in P1 mode; under symmetry it is \
             refused (convert the structure to P1 first). Warns when an atom comes closer to \
             another than the short-contact distance, 0.5 Å by default. Undoable.",
        ),
        |host: &mut dyn Host, input: SetPositionInput| {
            let document = document_mut(host, input.structure)?;
            let outcome = document.edit(Edit::SetPosition {
                site: input.site,
                position: input.position,
            })?;
            let output = edit_output(document, outcome, vec![input.site]);
            let [x, y, z] = input.position;
            let summary = edit_summary(
                &output,
                format!(
                    "Moved {} to ({x}, {y}, {z}).",
                    label(document.structure(), input.site)
                ),
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
