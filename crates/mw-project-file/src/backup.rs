//! Backups of what an MCP session without a window overwrites (design
//! §15): before a file of the project is replaced for the first time in the
//! session, its old content is copied to `backup/mcp-<time>/` at the same
//! relative path.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::store::structure_path;
use crate::{MANIFEST, Save, Timestamp};

/// Session backups kept in a project; older ones are removed.
pub const KEEP_BACKUPS: usize = 10;
const PREFIX: &str = "mcp-";

/// The files one session replaced in one project, as they were.
#[derive(Debug)]
pub struct SessionBackup {
    project: PathBuf,
    /// `backup/mcp-<time>`, made with the first copy.
    dir: Option<PathBuf>,
    /// Relative paths already copied, or found missing.
    done: HashSet<PathBuf>,
}

impl SessionBackup {
    pub fn new(project: &Path) -> Self {
        Self {
            project: project.to_path_buf(),
            dir: None,
            done: HashSet::new(),
        }
    }

    /// Where the copies go, once there are any.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Copies the files `save` will replace that this session has not
    /// copied yet: the manifest and the structure files it rewrites. New
    /// files need no copy.
    pub fn protect(&mut self, save: &Save) -> io::Result<()> {
        let mut files = vec![PathBuf::from(MANIFEST)];
        for (id, _) in &save.structures {
            let path = structure_path(Path::new(""), *id);
            files.push(path);
        }
        for file in files {
            self.copy(&file)?;
        }
        Ok(())
    }

    fn copy(&mut self, relative: &Path) -> io::Result<()> {
        if self.done.contains(relative) {
            return Ok(());
        }
        let source = self.project.join(relative);
        if source.exists() {
            let dir = match &self.dir {
                Some(dir) => dir.clone(),
                None => self.start()?,
            };
            let target = dir.join(relative);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source, &target)?;
        }
        self.done.insert(relative.to_path_buf());
        Ok(())
    }

    /// Makes this session's directory, after removing the oldest session
    /// backups beyond [`KEEP_BACKUPS`].
    fn start(&mut self) -> io::Result<PathBuf> {
        let backups = self.project.join("backup");
        fs::create_dir_all(&backups)?;
        prune(&backups);
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        // `2026-10-24T09:12:03Z` without the separators Windows does not
        // allow in file names.
        let stamp: String = Timestamp::from_unix(seconds)
            .as_str()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        let mut dir = backups.join(format!("{PREFIX}{stamp}"));
        let mut n = 1;
        while dir.exists() {
            n += 1;
            dir = backups.join(format!("{PREFIX}{stamp}-{n}"));
        }
        fs::create_dir(&dir)?;
        self.dir = Some(dir.clone());
        Ok(dir)
    }
}

/// Removes the oldest `mcp-*` directories so that, with the one about to be
/// made, at most [`KEEP_BACKUPS`] remain. Names sort by time. Failures leave
/// a backup behind and are not reported.
fn prune(backups: &Path) {
    let Ok(entries) = fs::read_dir(backups) else {
        return;
    };
    let mut sessions: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with(PREFIX))
        })
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    sessions.sort();
    let excess = (sessions.len() + 1).saturating_sub(KEEP_BACKUPS);
    for old in &sessions[..excess] {
        let _ = fs::remove_dir_all(old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EntryId, Manifest, StructureFile};

    fn save(entries: &[EntryId]) -> Save {
        let structure = mw_crystal::sample::load(mw_crystal::sample::SampleKey::Rutile).structure;
        Save {
            manifest: Manifest::new("p"),
            structures: entries
                .iter()
                .map(|id| (*id, StructureFile::new(structure.clone(), vec![], vec![])))
                .collect(),
            sources: Vec::new(),
            from: None,
            view: None,
        }
    }

    #[test]
    fn only_replaced_files_are_copied_once() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path();
        let (old, new) = (EntryId::new(), EntryId::new());
        fs::create_dir(project.join("structures")).unwrap();
        fs::write(project.join(MANIFEST), "manifest 1").unwrap();
        fs::write(structure_path(project, old), "structure 1").unwrap();

        let mut backup = SessionBackup::new(project);
        assert!(backup.dir().is_none());
        backup.protect(&save(&[old, new])).unwrap();
        let dir = backup.dir().unwrap().to_path_buf();
        assert!(
            dir.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("mcp-")
        );
        assert_eq!(
            fs::read_to_string(dir.join(MANIFEST)).unwrap(),
            "manifest 1"
        );
        let copied = structure_path(&dir, old);
        assert_eq!(fs::read_to_string(&copied).unwrap(), "structure 1");
        // A structure that was not there before has nothing to keep.
        assert!(!structure_path(&dir, new).exists());

        // Later saves of the session keep the first copies.
        fs::write(project.join(MANIFEST), "manifest 2").unwrap();
        backup.protect(&save(&[old])).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(MANIFEST)).unwrap(),
            "manifest 1"
        );
    }

    #[test]
    fn old_session_backups_are_removed() {
        let temp = tempfile::tempdir().unwrap();
        let project = temp.path();
        let backups = project.join("backup");
        for n in 0..12 {
            fs::create_dir_all(backups.join(format!("mcp-2026010{n:02}"))).unwrap();
        }
        fs::create_dir_all(backups.join("format-v1")).unwrap();
        fs::write(project.join(MANIFEST), "manifest").unwrap();
        let mut backup = SessionBackup::new(project);
        backup.protect(&save(&[])).unwrap();
        let mut left: Vec<String> = fs::read_dir(&backups)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left.len(), KEEP_BACKUPS + 1);
        assert_eq!(left[0], "format-v1");
        assert_eq!(left[1], "mcp-202601003");
    }
}
