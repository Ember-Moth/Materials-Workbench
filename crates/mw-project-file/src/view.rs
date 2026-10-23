//! `view.json` (design §8): how each structure was last shown. Not part of
//! the project's data; unreadable view state is dropped.

use std::collections::BTreeMap;

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
            panel,
            panel_open,
        }
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

/// The side panel's tabs (interface design §4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelTab {
    Properties,
    Sites,
    Measurements,
    Diagnostics,
}
