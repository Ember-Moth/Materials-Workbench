//! View tools (ai-integration design §5.3): what the application's window
//! shows of an open structure, and a picture of it. They change no data and
//! make no undo step, so clients need no permission for them; only the
//! application has a window, so only it offers them.

use mw_crystal::{AtomRef, EditError, SiteId, Structure, Target};

use crate::registry::{
    Availability, Effect, Host, Output, Registry, Spec, StructureEntryId, ViewChange,
};
use crate::schema::{schema_struct, string_enum};
use crate::structure::{StructureInput, document, invalid};

/// The smallest side of a snapshot, in pixels.
pub const MIN_SNAPSHOT_SIZE: u32 = 64;
/// The largest side of a snapshot, in pixels: a picture for a model to look
/// at, not for print.
pub const MAX_SNAPSHOT_SIZE: u32 = 2048;

string_enum! {
    /// How the window draws atoms.
    pub enum DisplayModel {
        BallAndStick => "ball_and_stick",
        SpaceFilling => "space_filling",
        Polyhedra => "polyhedra",
        Ellipsoids => "ellipsoids",
    }
}

schema_struct! {
    pub struct SelectInput {
        /// The structure's id, from structure.list; open it with
        /// structure.open first.
        pub structure: StructureEntryId,
        /// Whole sites to select, with all their equivalent atoms; ids from
        /// structure.sites.
        pub sites: Vec<SiteId>,
        /// Single atoms to select: a site id, which of its equivalent atoms
        /// (member) and a lattice cell, as structure.neighbors gives them.
        pub atoms: Vec<AtomRef>,
    }
}

schema_struct! {
    pub struct SelectOutput {
        /// The sites selected, or with an atom selected.
        pub sites: Vec<SiteId>,
        /// Single atoms selected.
        pub atoms: usize,
    }
}

schema_struct! {
    pub struct DisplayModelInput {
        /// The structure's id, from structure.list; open it with
        /// structure.open first.
        pub structure: StructureEntryId,
        /// ball_and_stick; space_filling, atoms as large spheres without
        /// bonds; polyhedra, coordination polyhedra around the cations
        /// with the other atoms as balls and sticks; or ellipsoids, each
        /// atom's displacement ellipsoid from its ADPs, at the probability
        /// set in the window (50 % unless changed).
        pub model: DisplayModel,
    }
}

schema_struct! {
    pub struct DisplayModelOutput {
        pub model: DisplayModel,
    }
}

schema_struct! {
    pub struct ResetOutput {
        /// Always true: the camera is at its default angle and zoom.
        pub reset: bool,
    }
}

schema_struct! {
    pub struct SnapshotInput {
        /// The structure's id, from structure.list; open it with
        /// structure.open first.
        pub structure: StructureEntryId,
        /// Pixels, 64 to 2048; 0, with height 0, for the size of the
        /// application's viewport.
        pub width: u32,
        /// Pixels, 64 to 2048; 0, with width 0, for the size of the
        /// application's viewport.
        pub height: u32,
    }
}

schema_struct! {
    pub struct SnapshotOutput {
        /// Pixels.
        pub width: u32,
        /// Pixels.
        pub height: u32,
    }
}

/// The structure's name, as listed.
fn name(host: &dyn Host, id: StructureEntryId) -> String {
    host.structures()
        .into_iter()
        .find(|info| info.id == id)
        .map(|info| info.name)
        .unwrap_or_default()
}

fn label(structure: &Structure, site: SiteId) -> String {
    structure
        .site(site)
        .map_or_else(String::new, |s| s.label().to_string())
}

pub(crate) fn register(registry: &mut Registry) {
    let view =
        |name, description| Spec::new(name, description, Effect::View).only(Availability::Gui);

    registry.register(
        view(
            "view.select",
            "Select sites or atoms in the application's window, as clicking them there would: \
             they are highlighted for the user, and the properties panel shows the last site. \
             Give no sites and no atoms to clear the selection. Shows the structure in the \
             window; changes no data.",
        ),
        |host: &mut dyn Host, input: SelectInput| {
            let document = document(host, input.structure)?;
            let structure = document.structure();
            let mut targets = Vec::new();
            for &site in &input.sites {
                if structure.site(site).is_none() {
                    return Err(EditError::UnknownSite(site).into());
                }
                let target = Target::Site(site);
                if !targets.contains(&target) {
                    targets.push(target);
                }
            }
            for &atom in &input.atoms {
                if document.derived().fractional(atom).is_none() {
                    return Err(EditError::UnknownAtom(atom).into());
                }
                let target = Target::Atom(atom);
                if !targets.contains(&target) {
                    targets.push(target);
                }
            }
            let mut sites: Vec<SiteId> = Vec::new();
            for target in &targets {
                let site = match target {
                    Target::Site(site) => *site,
                    Target::Atom(atom) => atom.site,
                };
                if !sites.contains(&site) {
                    sites.push(site);
                }
            }
            let atoms = targets
                .iter()
                .filter(|t| matches!(t, Target::Atom(_)))
                .count();
            let labels: Vec<String> = sites.iter().map(|&s| label(structure, s)).collect();
            let labels = labels.join(", ");
            let whole = targets.len() - atoms;
            let summary = if targets.is_empty() {
                "Cleared the selection.".to_string()
            } else if atoms == 0 {
                format!("Selected the sites {labels} in the window.")
            } else if whole == 0 {
                format!("Selected {atoms} atoms of {labels} in the window.")
            } else {
                format!(
                    "Selected {whole} whole sites and {atoms} single atoms in the window, of \
                     {labels}."
                )
            };
            host.change_view(input.structure, ViewChange::Select(targets))?;
            Ok(Output::new(SelectOutput { sites, atoms }, summary))
        },
    );

    registry.register(
        view(
            "view.set_display_model",
            "Switch how the application's window draws a structure: balls and sticks, space \
             filling, coordination polyhedra, or displacement ellipsoids. Shows the structure in \
             the window; changes no data.",
        ),
        |host: &mut dyn Host, input: DisplayModelInput| {
            document(host, input.structure)?;
            host.change_view(input.structure, ViewChange::DisplayModel(input.model))?;
            let how = match input.model {
                DisplayModel::BallAndStick => "balls and sticks",
                DisplayModel::SpaceFilling => "space filling",
                DisplayModel::Polyhedra => "coordination polyhedra",
                DisplayModel::Ellipsoids => "displacement ellipsoids",
            };
            let summary = format!("The window shows {} as {how}.", name(host, input.structure));
            Ok(Output::new(
                DisplayModelOutput { model: input.model },
                summary,
            ))
        },
    );

    registry.register(
        view(
            "view.reset",
            "Turn the camera of the application's window back to its default angle and zoom, \
             which take in the whole structure. Shows the structure in the window; changes no \
             data.",
        ),
        |host: &mut dyn Host, input: StructureInput| {
            document(host, input.structure)?;
            host.change_view(input.structure, ViewChange::ResetCamera)?;
            let summary = format!(
                "The window shows {} from the default angle.",
                name(host, input.structure)
            );
            Ok(Output::new(ResetOutput { reset: true }, summary))
        },
    );

    registry.register(
        view(
            "view.snapshot",
            "Return a PNG picture of a structure as the application's window shows it: its \
             camera, display model, display range and selection. For looking at the structure; \
             every number still comes from the other tools. Shows the structure in the window. \
             Width and height are pixels from 64 to 2048, or both 0 for the viewport's size.",
        ),
        |host: &mut dyn Host, input: SnapshotInput| {
            document(host, input.structure)?;
            let sizes = MIN_SNAPSHOT_SIZE..=MAX_SNAPSHOT_SIZE;
            let own = input.width == 0 && input.height == 0;
            if !own && !(sizes.contains(&input.width) && sizes.contains(&input.height)) {
                return Err(invalid(format!(
                    "width and height are {MIN_SNAPSHOT_SIZE} to {MAX_SNAPSHOT_SIZE} pixels, \
                     or both 0; got {} × {}",
                    input.width, input.height
                )));
            }
            let image = host.snapshot(input.structure, input.width, input.height)?;
            let output = SnapshotOutput {
                width: image.width,
                height: image.height,
            };
            let summary = format!(
                "A picture of {} as the window shows it, {} × {} pixels.",
                name(host, input.structure),
                image.width,
                image.height
            );
            Ok(Output::new(output, summary).with_image(image))
        },
    );
}
