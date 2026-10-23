//! Where the application keeps its own files (`docs/design/project-format.md`
//! §10): settings and the session in the system's configuration directory,
//! writable copies of the sample projects in its data directory. Both files
//! are JSON with a version; a file that cannot be read gives the defaults,
//! never a failed start.

use std::fs;
use std::path::{Path, PathBuf};

use gpui_kit::Global;
use mw_project_file::{ProjectId, Timestamp};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The application's directories, set once at start.
#[derive(Clone, Debug)]
pub struct AppPaths {
    config: PathBuf,
    data: PathBuf,
}

impl Global for AppPaths {}

impl AppPaths {
    /// The system's directories, or `MW_CONFIG_DIR` for both when set (for
    /// development and the screenshot tour, which must not touch the
    /// user's own files).
    pub fn from_environment() -> Self {
        if let Some(dir) = std::env::var_os("MW_CONFIG_DIR") {
            return Self::in_dir(PathBuf::from(dir));
        }
        let name = "materials-workbench";
        let config = dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(name);
        let data = dirs::data_dir()
            .unwrap_or_else(|| config.clone())
            .join(name);
        Self { config, data }
    }

    /// Both kinds of files in one directory.
    pub fn in_dir(dir: PathBuf) -> Self {
        Self {
            config: dir.clone(),
            data: dir,
        }
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    pub fn session_file(&self) -> PathBuf {
        self.config.join("state.json")
    }

    /// Where the sample projects are installed.
    pub fn samples_dir(&self) -> PathBuf {
        self.data.join("samples")
    }
}

const VERSION: u32 = 1;

/// Reads a versioned JSON file; `None` if it is missing, unreadable or of
/// another version.
fn read<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            log::warn!("ignoring {}: {error}", path.display());
            return None;
        }
    };
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(u64::from(VERSION)) {
        log::warn!("ignoring {}: unknown version", path.display());
        return None;
    }
    serde_json::from_value(value)
        .inspect_err(|error| log::warn!("ignoring {}: {error}", path.display()))
        .ok()
}

/// Writes a JSON file through a temporary one; failures are logged, since
/// losing a setting must not interrupt work.
fn write<T: Serialize>(path: &Path, value: &T) {
    let result = (|| {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let bytes = mw_project_file::to_json(value).map_err(std::io::Error::other)?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(&temporary, path)
    })();
    if let Err(error) = result {
        log::error!("could not write {}: {error}", path.display());
    }
}

/// The interface's appearance and language.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub(crate) version: u32,
    pub dark: bool,
    pub language: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: VERSION,
            dark: false,
            language: "zh-CN".into(),
        }
    }
}

impl Settings {
    pub fn load(paths: &AppPaths) -> Self {
        read(&paths.settings_file()).unwrap_or_default()
    }

    pub fn save(&self, paths: &AppPaths) {
        write(&paths.settings_file(), self);
    }
}

/// How many recent projects are remembered.
const RECENT: usize = 10;

/// A project opened before, most recent first.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: PathBuf,
    pub id: ProjectId,
    pub name: String,
    pub opened_at: Timestamp,
}

/// What survives a restart besides the projects themselves: recent
/// projects, and the ones open at quit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Session {
    pub(crate) version: u32,
    #[serde(default)]
    pub recent: Vec<RecentProject>,
    /// The projects open at quit, in sidebar order.
    #[serde(default)]
    pub open: Vec<PathBuf>,
    /// The project of the structure that was open.
    #[serde(default)]
    pub active: Option<PathBuf>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            version: VERSION,
            recent: Vec::new(),
            open: Vec::new(),
            active: None,
        }
    }
}

impl Session {
    /// The saved session, or `None` on the first start.
    pub fn load(paths: &AppPaths) -> Option<Self> {
        let path = paths.session_file();
        if !path.exists() {
            return None;
        }
        Some(read(&path).unwrap_or_default())
    }

    pub fn save(&self, paths: &AppPaths) {
        write(&paths.session_file(), self);
    }

    /// Puts a project first among the recent ones. A project moved since is
    /// one entry, under its new path.
    pub fn remember(&mut self, path: &Path, id: ProjectId, name: &str) {
        self.recent
            .retain(|recent| recent.id != id && recent.path != path);
        self.recent.insert(
            0,
            RecentProject {
                path: path.to_path_buf(),
                id,
                name: name.to_string(),
                opened_at: Timestamp::now(),
            },
        );
        self.recent.truncate(RECENT);
    }

    pub fn forget(&mut self, path: &Path) {
        self.recent.retain(|recent| recent.path != path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_and_session_survive_a_restart() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::in_dir(temp.path().join("config"));
        assert_eq!(Settings::load(&paths), Settings::default());
        assert_eq!(Session::load(&paths), None);

        let settings = Settings {
            dark: true,
            language: "en".into(),
            ..Settings::default()
        };
        settings.save(&paths);
        assert_eq!(Settings::load(&paths), settings);

        let mut session = Session {
            open: vec!["/p/a.mwproj".into()],
            ..Session::default()
        };
        session.remember(Path::new("/p/a.mwproj"), ProjectId::from_u128(1), "a");
        session.save(&paths);
        assert_eq!(Session::load(&paths), Some(session));
    }

    #[test]
    fn unreadable_files_give_the_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::in_dir(temp.path().to_path_buf());
        fs::write(paths.settings_file(), "{").unwrap();
        assert_eq!(Settings::load(&paths), Settings::default());
        fs::write(paths.session_file(), r#"{"version": 9}"#).unwrap();
        // Not the first start: the session file is there.
        assert_eq!(Session::load(&paths), Some(Session::default()));
    }

    #[test]
    fn recent_projects_are_unique_and_few() {
        let mut session = Session::default();
        for i in 0..12u128 {
            let path = PathBuf::from(format!("/p/{i}.mwproj"));
            session.remember(&path, ProjectId::from_u128(i), &i.to_string());
        }
        assert_eq!(session.recent.len(), RECENT);
        assert_eq!(session.recent[0].name, "11");
        // The same project, moved: one entry, under the new path.
        session.remember(Path::new("/q/5.mwproj"), ProjectId::from_u128(5), "5");
        assert_eq!(session.recent[0].path, PathBuf::from("/q/5.mwproj"));
        assert_eq!(session.recent.iter().filter(|r| r.name == "5").count(), 1);
        session.forget(Path::new("/q/5.mwproj"));
        assert!(session.recent.iter().all(|r| r.name != "5"));
    }
}
