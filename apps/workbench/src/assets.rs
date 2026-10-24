//! Application assets: GPUI Kit's default icon bundle plus the Lucide icons
//! the features use beyond it. Feature crates reference icons from the full
//! catalog (`gpui_kit::assets::IconName`), but only the binary can embed
//! them, so every such icon must be listed here.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        Atom, Axis3d, Box, Boxes, Circle, Clock, Download, FileClock, Folder, FolderOpen, FolderX,
        Grid3x3, Hexagon, Languages, Link, Lock, Maximize, Moon, Move, PanelLeft, PanelRight, Plus,
        Redo2, Ruler, Save, Settings, Trash, Undo2, X,
    ]
);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}
