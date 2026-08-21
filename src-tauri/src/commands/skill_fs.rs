// SPDX-License-Identifier: Apache-2.0

//! Capability-oriented filesystem access for Skill packages.
//!
//! The ambient path is resolved exactly once when a root is opened. Every
//! package directory and file operation after that is relative to an already
//! open directory handle. Final directory components and package files are
//! opened without following symlinks/reparse points.

use cap_fs_ext::{FollowSymlinks, MetadataExt, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use std::ffi::{OsStr, OsString};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

pub(crate) struct SecureDir {
    dir: Dir,
    display_path: PathBuf,
}

impl SecureDir {
    pub(crate) fn open_existing(path: &Path) -> Result<Self, String> {
        Self::open_ambient(path, false)
    }

    pub(crate) fn open_or_create(path: &Path) -> Result<Self, String> {
        Self::open_ambient(path, true)
    }

    fn open_ambient(path: &Path, create: bool) -> Result<Self, String> {
        if create {
            std::fs::create_dir_all(path).map_err(|error| {
                format!("SKILL_IO_FAILED: cannot create {}: {error}", path.display())
            })?;
        }
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| format!("SKILL_PATH_UNSAFE: current directory: {error}"))?
                .join(path)
        };
        let name = absolute.file_name().ok_or_else(|| {
            format!(
                "SKILL_PATH_UNSAFE: directory has no final component: {}",
                absolute.display()
            )
        })?;
        let parent = absolute.parent().ok_or_else(|| {
            format!(
                "SKILL_PATH_UNSAFE: directory has no parent: {}",
                absolute.display()
            )
        })?;
        let parent_dir =
            Dir::open_ambient_dir(parent, cap_std::ambient_authority()).map_err(|error| {
                format!(
                    "SKILL_PATH_UNSAFE: open parent {}: {error}",
                    parent.display()
                )
            })?;
        let dir = open_dir_nofollow(&parent_dir, name)?;
        Ok(Self {
            dir,
            display_path: absolute,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.display_path
    }

    pub(crate) fn entry_names(&self) -> Result<Vec<OsString>, String> {
        let entries = self.dir.entries().map_err(|error| {
            format!(
                "SKILL_IO_FAILED: list {}: {error}",
                self.display_path.display()
            )
        })?;
        entries
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|error| format!("SKILL_IO_FAILED: list entry: {error}"))
            })
            .collect()
    }

    pub(crate) fn open_child_dir(&self, name: &OsStr) -> Result<Self, String> {
        validate_component(name)?;
        let dir = open_dir_nofollow(&self.dir, name)?;
        Ok(Self {
            dir,
            display_path: self.display_path.join(name),
        })
    }

    pub(crate) fn create_child_dir(&self, name: &str) -> Result<Self, String> {
        validate_component(OsStr::new(name))?;
        self.dir.create_dir(name).map_err(|error| {
            if error.kind() == ErrorKind::AlreadyExists {
                format!(
                    "SKILL_ID_ALREADY_INSTALLED: install target already exists: {}",
                    self.display_path.join(name).display()
                )
            } else {
                format!("SKILL_IO_FAILED: create directory {name:?}: {error}")
            }
        })?;
        self.open_child_dir(OsStr::new(name))
    }

    pub(crate) fn read_optional(&self, filename: &str) -> Result<Option<Vec<u8>>, String> {
        validate_filename(filename)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let mut file = match self.dir.open_with(filename, &options) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "SKILL_PATH_UNSAFE: open {}: {error}",
                    self.display_path.join(filename).display()
                ))
            }
        };
        let metadata = file
            .metadata()
            .map_err(|error| format!("SKILL_PATH_UNSAFE: file metadata: {error}"))?;
        if !metadata.is_file() || MetadataExt::nlink(&metadata) != 1 {
            return Err(format!(
                "SKILL_PATH_UNSAFE: package file is not a unique regular file: {}",
                self.display_path.join(filename).display()
            ));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| format!("SKILL_IO_FAILED: read {filename}: {error}"))?;
        Ok(Some(bytes))
    }

    pub(crate) fn read_required(&self, filename: &str) -> Result<Vec<u8>, String> {
        self.read_optional(filename)?.ok_or_else(|| {
            format!(
                "SKILL_SOURCE_FILE_INVALID: missing {}",
                self.display_path.join(filename).display()
            )
        })
    }

    pub(crate) fn read_string_optional(&self, filename: &str) -> Result<Option<String>, String> {
        self.read_optional(filename)?
            .map(|bytes| {
                String::from_utf8(bytes)
                    .map_err(|error| format!("SKILL_SOURCE_UTF8_INVALID: {filename}: {error}"))
            })
            .transpose()
    }

    pub(crate) fn read_string_required(&self, filename: &str) -> Result<String, String> {
        String::from_utf8(self.read_required(filename)?)
            .map_err(|error| format!("SKILL_SOURCE_UTF8_INVALID: {filename}: {error}"))
    }

    pub(crate) fn has_regular_file(&self, filename: &str) -> bool {
        self.read_optional(filename)
            .is_ok_and(|value| value.is_some())
    }

    /// Replace a package file through a newly-created inode and a relative
    /// rename. Existing hardlinks are never modified in place.
    pub(crate) fn write_atomic(&self, filename: &str, bytes: &[u8]) -> Result<(), String> {
        validate_filename(filename)?;
        let mut last_collision = None;
        for _ in 0..8 {
            let temp_name = format!(".cf-write-{}", uuid::Uuid::new_v4());
            let mut options = OpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            let mut file = match self.dir.open_with(&temp_name, &options) {
                Ok(file) => file,
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    last_collision = Some(error);
                    continue;
                }
                Err(error) => {
                    return Err(format!("SKILL_IO_FAILED: create temporary file: {error}"))
                }
            };
            let result = (|| {
                let metadata = file
                    .metadata()
                    .map_err(|error| format!("SKILL_PATH_UNSAFE: temporary metadata: {error}"))?;
                if !metadata.is_file() || MetadataExt::nlink(&metadata) != 1 {
                    return Err("SKILL_PATH_UNSAFE: temporary file is not unique".to_string());
                }
                file.write_all(bytes)
                    .map_err(|error| format!("SKILL_IO_FAILED: write {filename}: {error}"))?;
                file.sync_all()
                    .map_err(|error| format!("SKILL_IO_FAILED: sync {filename}: {error}"))?;
                drop(file);
                self.dir
                    .rename(&temp_name, &self.dir, filename)
                    .map_err(|error| format!("SKILL_IO_FAILED: commit {filename}: {error}"))?;
                Ok(())
            })();
            if result.is_err() {
                let _ = self.dir.remove_file(&temp_name);
            }
            return result;
        }
        Err(format!(
            "SKILL_IO_FAILED: could not allocate temporary file: {:?}",
            last_collision
        ))
    }

    pub(crate) fn remove_file(&self, filename: &str) -> Result<(), String> {
        validate_filename(filename)?;
        match self.dir.remove_file(filename) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "SKILL_IO_FAILED: remove {}: {error}",
                self.display_path.join(filename).display()
            )),
        }
    }

    pub(crate) fn remove_open_dir_all(self) -> Result<(), String> {
        #[cfg(windows)]
        {
            // cap-primitives 4.0.3 closes the directory handle and falls back
            // to path-based recursive deletion on Windows. Fail closed until
            // CodeFactory has an exact-handle reparse-safe implementation.
            let _ = self;
            return Err(
                "SKILL_REMOVE_UNAVAILABLE_WINDOWS_PHASE0: safe recursive removal is not available on this Windows build"
                    .to_string(),
            );
        }
        #[cfg(not(windows))]
        self.dir
            .remove_open_dir_all()
            .map_err(|error| format!("SKILL_IO_FAILED: remove package: {error}"))
    }
}

fn open_dir_nofollow(parent: &Dir, name: &OsStr) -> Result<Dir, String> {
    validate_component(name)?;
    let parent_file = parent
        .try_clone()
        .map_err(|error| format!("SKILL_PATH_UNSAFE: clone directory handle: {error}"))?
        .into_std_file();
    let child = cap_primitives::fs::open_dir_nofollow(&parent_file, Path::new(name))
        .map_err(|error| format!("SKILL_PATH_UNSAFE: open directory without links: {error}"))?;
    Ok(Dir::from_std_file(child))
}

fn validate_component(name: &OsStr) -> Result<(), String> {
    let path = Path::new(name);
    let mut components = path.components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(format!(
            "SKILL_PATH_UNSAFE: invalid path component {name:?}"
        ));
    }
    Ok(())
}

fn validate_filename(filename: &str) -> Result<(), String> {
    if filename.is_empty() || filename == "." || filename == ".." {
        return Err(format!(
            "SKILL_PATH_UNSAFE: invalid package filename {filename:?}"
        ));
    }
    validate_component(OsStr::new(filename))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_a_hardlink_without_mutating_its_outside_target() {
        let fixture = tempfile::tempdir().unwrap();
        let root_path = fixture.path().join("skills");
        std::fs::create_dir(&root_path).unwrap();
        let outside = fixture.path().join("outside.txt");
        std::fs::write(&outside, "OUTSIDE").unwrap();

        let root = SecureDir::open_existing(&root_path).unwrap();
        let skill = root.create_child_dir("safe-skill").unwrap();
        std::fs::hard_link(&outside, skill.path().join("system_prompt.md")).unwrap();

        assert!(skill.read_optional("system_prompt.md").is_err());
        skill.write_atomic("system_prompt.md", b"INSIDE").unwrap();

        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "OUTSIDE");
        assert_eq!(
            std::fs::read_to_string(skill.path().join("system_prompt.md")).unwrap(),
            "INSIDE"
        );
    }

    #[cfg(unix)]
    #[test]
    fn root_handle_stays_inside_when_the_ambient_path_is_replaced() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let root_path = fixture.path().join("skills");
        let held_path = fixture.path().join("held-skills");
        let outside = fixture.path().join("outside");
        std::fs::create_dir(&root_path).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let root = SecureDir::open_existing(&root_path).unwrap();

        std::fs::rename(&root_path, &held_path).unwrap();
        symlink(&outside, &root_path).unwrap();

        let skill = root.create_child_dir("safe-skill").unwrap();
        skill.write_atomic("system_prompt.md", b"SAFE").unwrap();

        assert!(outside.read_dir().unwrap().next().is_none());
        assert_eq!(
            std::fs::read_to_string(held_path.join("safe-skill/system_prompt.md")).unwrap(),
            "SAFE"
        );
    }

    #[cfg(unix)]
    #[test]
    fn delete_consumes_the_open_directory_not_a_replacement_symlink() {
        use std::os::unix::fs::symlink;

        let fixture = tempfile::tempdir().unwrap();
        let root_path = fixture.path().join("skills");
        let outside = fixture.path().join("outside");
        std::fs::create_dir(&root_path).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("victim"), "UNCHANGED").unwrap();
        let root = SecureDir::open_existing(&root_path).unwrap();
        let skill = root.create_child_dir("safe-skill").unwrap();
        skill.write_atomic("system_prompt.md", b"SAFE").unwrap();

        std::fs::rename(root_path.join("safe-skill"), root_path.join("moved-skill")).unwrap();
        symlink(&outside, root_path.join("safe-skill")).unwrap();
        skill.remove_open_dir_all().unwrap();

        assert!(!root_path.join("moved-skill").exists());
        assert!(root_path.join("safe-skill").is_symlink());
        assert_eq!(
            std::fs::read_to_string(outside.join("victim")).unwrap(),
            "UNCHANGED"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_recursive_skill_removal_fails_closed() {
        let fixture = tempfile::tempdir().unwrap();
        let root_path = fixture.path().join("skills");
        std::fs::create_dir(&root_path).unwrap();
        let root = SecureDir::open_existing(&root_path).unwrap();
        let skill = root.create_child_dir("safe-skill").unwrap();
        skill.write_atomic("sentinel.txt", b"UNCHANGED").unwrap();

        let error = skill.remove_open_dir_all().unwrap_err();

        assert!(error.starts_with("SKILL_REMOVE_UNAVAILABLE_WINDOWS_PHASE0:"));
        assert_eq!(
            std::fs::read_to_string(root_path.join("safe-skill/sentinel.txt")).unwrap(),
            "UNCHANGED"
        );
    }

    #[cfg(unix)]
    #[test]
    fn root_replacement_subprocess_probe() {
        use std::os::unix::fs::symlink;

        let Some(root_path) = std::env::var_os("CODEFACTORY_SKILL_HANDLE_PROBE_ROOT") else {
            return;
        };
        let outside =
            PathBuf::from(std::env::var_os("CODEFACTORY_SKILL_HANDLE_PROBE_OUTSIDE").unwrap());
        let root_path = PathBuf::from(root_path);
        let held_path = root_path.with_file_name("held-skills");
        let root = SecureDir::open_existing(&root_path).unwrap();
        std::fs::rename(&root_path, &held_path).unwrap();
        symlink(&outside, &root_path).unwrap();
        let skill = root.create_child_dir("subprocess-skill").unwrap();
        skill.write_atomic("system_prompt.md", b"SAFE").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn root_replacement_in_a_child_process_cannot_write_outside_the_handle() {
        let fixture = tempfile::tempdir().unwrap();
        let root_path = fixture.path().join("skills");
        let outside = fixture.path().join("outside");
        std::fs::create_dir(&root_path).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("sentinel"), "UNCHANGED").unwrap();

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("commands::skill_fs::tests::root_replacement_subprocess_probe")
            .arg("--exact")
            .env("CODEFACTORY_SKILL_HANDLE_PROBE_ROOT", &root_path)
            .env("CODEFACTORY_SKILL_HANDLE_PROBE_OUTSIDE", &outside)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "probe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(outside.join("sentinel")).unwrap(),
            "UNCHANGED"
        );
        assert_eq!(outside.read_dir().unwrap().count(), 1);
        assert_eq!(
            std::fs::read_to_string(
                fixture
                    .path()
                    .join("held-skills/subprocess-skill/system_prompt.md")
            )
            .unwrap(),
            "SAFE"
        );
    }
}
