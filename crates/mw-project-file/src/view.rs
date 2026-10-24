//! `view.json` (design §8): how each structure was last shown. Not part of
//! the project's data; unreadable view state is dropped.

use std::collections::BTreeMap;

use mw_crystal::AtomRef;
use serde::{Deserialize, Serialize};

use crate::EntryId;

/// The view state of a project.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ViewState {
    /// The entry that was open when the project was left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<EntryId>,
    #[serde(default)]
    pub entries: BTreeMap<EntryId, EntryView>,
}

/// How one structure was shown. Missing parts take the defaults.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EntryView {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<Camera>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_model: Option<DisplayModel>,
    /// Since `view.json` version 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_range: Option<DisplayRange>,
    /// The elements whose atoms are the centers of polyhedra, once the
    /// user chose them (crystallography design §5.6); `None` for the
    /// default rule. Since `view.json` version 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polyhedron_centers: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<PanelTab>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_open: Option<bool>,
}

impl EntryView {
    pub fn new(
        camera: Option<Camera>,
        display_model: Option<DisplayModel>,
        panel: Option<PanelTab>,
        panel_open: Option<bool>,
    ) -> Self {
        Self {
            camera,
            display_model,
            display_range: None,
            polyhedron_centers: None,
            panel,
            panel_open,
        }
    }

    pub fn with_display_range(mut self, range: DisplayRange) -> Self {
        self.display_range = Some(range);
        self
    }

    /// The elements chosen as the centers of polyhedra, in order.
    pub fn with_polyhedron_centers(mut self, mut elements: Vec<String>) -> Self {
        elements.sort();
        elements.dedup();
        self.polyhedron_centers = Some(elements);
        self
    }
}

/// An orbit camera around the scene center: angles in radians, and half
/// the visible height in Å. The scene's size is computed again on opening.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub half_height: f32,
    /// Å, relative to the scene center.
    pub target: [f32; 3],
}

impl Camera {
    pub fn new(yaw: f32, pitch: f32, half_height: f32, target: [f32; 3]) -> Self {
        Self {
            yaw,
            pitch,
            half_height,
            target,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayModel {
    BallAndStick,
    SpaceFilling,
    Polyhedra,
}

/// What part of the crystal is shown (crystallography design §5.5).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayRange {
    /// `counts` cells along a, b and c; `boundary`, whether atoms on the
    /// range's faces are repeated on the opposite ones.
    Cells { counts: [u32; 3], boundary: bool },
    /// Atoms within `radius` Å of the center.
    Sphere { center: SphereCenter, radius: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SphereCenter {
    Atom(AtomRef),
    /// Fractional coordinates.
    Point([f64; 3]),
}

/// The side panel's tabs (interface design §4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelTab {
    Properties,
    Sites,
    Measurements,
    Diagnostics,
}
