// SPDX-License-Identifier: Apache-2.0
//! The extension, shipped inside the app and written out ready to load.
//!
//! Before this module the only way to get the extension was to check out the
//! repository and run `pnpm ext:build`, then copy a port and a pairing token
//! into a form by hand — and because both changed on every app restart, that
//! hand-copying was not a one-time cost but a recurring chore. Neither step is
//! something a user of a desktop app should ever be asked to do.
//!
//! Two decisions remove them:
//!
//!   * **The extension is compiled into the binary** with `include_str!`, not
//!     shipped as bundle resources. The build then fails if a file goes missing,
//!     `page.js` stays the single copy the desktop backend also injects, and
//!     there is no difference between `pnpm tauri dev` and an installed app —
//!     which is exactly where a missing-resource bug would otherwise hide.
//!   * **Pairing is a file inside the extension folder.** An extension cannot
//!     read the user's disk, which is why a human used to carry the token — but
//!     it can always read its *own* package. Since CodeFactory is what writes
//!     that package, it can drop the current port and token in as
//!     `pairing.json`, and the service worker picks them up on its own. Nothing
//!     to copy, and a restart that changes the port repairs itself.
//!
//! The folder is per-user and stable, which also pins the unpacked extension's
//! ID: Chrome derives it from the path, so loading it once keeps working across
//! restarts and upgrades instead of appearing as a new extension each time.

use std::path::{Path, PathBuf};

use super::install;

/// Where the extension is written, relative to a browser data root.
const EXTENSION_SEGMENT: &str = "extension";

/// The pairing file the service worker reads out of its own package.
pub const PAIRING_FILE: &str = "pairing.json";

/// Every file the loadable extension is made of.
///
/// `page.js` is the same script the CDP backend injects; taking it from its home
/// next to the Rust that uses it is what keeps one source of truth. Listing the
/// files here rather than globbing a directory means a rename is a compile error
/// instead of an extension that silently loads without a page script.
const FILES: &[(&str, &str)] = &[
    (
        "manifest.json",
        include_str!("../../../extension/manifest.json"),
    ),
    (
        "background.js",
        include_str!("../../../extension/background.js"),
    ),
    (
        "options.html",
        include_str!("../../../extension/options.html"),
    ),
    (
        "options.js",
        include_str!("../../../extension/options.js"),
    ),
    ("content/page.js", include_str!("page.js")),
];

/// Candidate folders for the unpacked extension, best first.
///
/// Mirrors the browser install roots so a machine where `%LOCALAPPDATA%` is
/// locked down still gets a working extension folder somewhere else.
pub fn dir_candidates() -> Vec<PathBuf> {
    install::install_root_candidates()
        .into_iter()
        .filter_map(|root| root.parent().map(|parent| parent.join(EXTENSION_SEGMENT)))
        .collect()
}

/// The folder the extension is currently written to, if one exists.
///
/// Used by the UI to show the path to load without creating anything: a folder
/// that has never been prepared should not be offered as loadable.
pub fn existing_dir() -> Option<PathBuf> {
    dir_candidates()
        .into_iter()
        .find(|dir| dir.join("manifest.json").is_file())
}

/// Write the extension out and stamp the current pairing into it.
///
/// Idempotent, and deliberately re-uses whichever folder is already prepared:
/// the unpacked extension's ID follows its path, so moving it would make Chrome
/// treat it as a different extension and quietly drop the user's install.
pub fn prepare(port: u16, token: &str) -> Result<PathBuf, String> {
    let dir = match existing_dir() {
        Some(dir) => dir,
        None => {
            let candidates = dir_candidates();
            if candidates.is_empty() {
                return Err(
                    "Could not resolve a folder for the extension — no home or app-data \
                     directory is available."
                        .into(),
                );
            }
            install::first_writable_root(&candidates)
                .map_err(|attempts| install::unwritable_message(&attempts))?
        }
    };

    materialize(&dir)?;
    write_pairing(&dir, port, token)?;
    Ok(dir)
}

/// Write the extension's files, touching only what changed.
///
/// Rewriting an unchanged file would be enough to make Chrome consider the
/// unpacked extension dirty, so an app that prepares on every launch would nag
/// the user to reload it forever.
pub fn materialize(dir: &Path) -> Result<(), String> {
    for (relative, contents) in FILES {
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create {}: {error}", parent.display()))?;
        }
        let unchanged = std::fs::read_to_string(&path)
            .map(|existing| existing == *contents)
            .unwrap_or(false);
        if unchanged {
            continue;
        }
        std::fs::write(&path, contents)
            .map_err(|error| format!("Could not write {}: {error}", path.display()))?;
    }
    Ok(())
}

/// Put the live port and token where the service worker can read them.
///
/// This is the file that replaces the copy-and-paste. It is rewritten every time
/// the bridge starts, so an extension installed weeks ago reconnects to today's
/// port without the user opening Settings at all.
pub fn write_pairing(dir: &Path, port: u16, token: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("Could not create {}: {error}", dir.display()))?;
    let path = dir.join(PAIRING_FILE);
    let body = serde_json::json!({
        "port": port,
        "token": token,
        "protocol_version": super::bridge::PROTOCOL_VERSION,
    })
    .to_string();

    // Same reasoning as `materialize`: only write when it actually changed.
    if std::fs::read_to_string(&path)
        .map(|existing| existing == body)
        .unwrap_or(false)
    {
        return Ok(());
    }
    std::fs::write(&path, &body)
        .map_err(|error| format!("Could not write {}: {error}", path.display()))?;

    // The token is a capability: anything that can read it can drive the bridge.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_extension_is_complete_without_a_repository_checkout() {
        // The point of embedding: an installed app has every file, including the
        // page script that used to require `pnpm ext:build`.
        let dir = tempfile::tempdir().unwrap();
        materialize(dir.path()).expect("materialize");

        for (relative, _) in FILES {
            assert!(
                dir.path().join(relative).is_file(),
                "{relative} must be written"
            );
        }
        let manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("manifest.json")).unwrap())
                .expect("the manifest must be valid JSON");
        assert_eq!(manifest["manifest_version"], 3);
    }

    #[test]
    fn the_page_script_is_the_same_one_the_desktop_backend_injects() {
        // Two copies of the extraction logic would mean only one of them is
        // tested; the include_str! is what makes this true at compile time, and
        // this asserts it stays true.
        let embedded = FILES
            .iter()
            .find(|(name, _)| *name == "content/page.js")
            .expect("page script is shipped")
            .1;
        assert_eq!(embedded, include_str!("page.js"));
        assert!(
            !embedded.contains("\nimport ") && !embedded.starts_with("import "),
            "the extension loads this file directly, with no bundler"
        );
    }

    #[test]
    fn pairing_lands_where_the_service_worker_can_read_it() {
        let dir = tempfile::tempdir().unwrap();
        write_pairing(dir.path(), 47615, "0123456789abcdef0123456789abcdef").unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(PAIRING_FILE)).unwrap())
                .unwrap();
        assert_eq!(written["port"], 47615);
        assert_eq!(written["token"], "0123456789abcdef0123456789abcdef");
        assert_eq!(written["protocol_version"], super::super::bridge::PROTOCOL_VERSION);
    }

    #[test]
    fn a_new_port_replaces_the_old_pairing_rather_than_being_appended() {
        // A restart picks a different port; the file has to describe the live one
        // or the extension would keep dialling a socket nobody is listening on.
        let dir = tempfile::tempdir().unwrap();
        write_pairing(dir.path(), 100, "token-a").unwrap();
        write_pairing(dir.path(), 200, "token-b").unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join(PAIRING_FILE)).unwrap())
                .unwrap();
        assert_eq!(written["port"], 200);
        assert_eq!(written["token"], "token-b");
    }

    #[test]
    fn preparing_again_does_not_touch_files_that_did_not_change() {
        // Chrome treats a rewritten file as a change and asks the user to reload
        // the unpacked extension. Preparing on every launch must stay invisible.
        let dir = tempfile::tempdir().unwrap();
        materialize(dir.path()).unwrap();
        let manifest = dir.path().join("manifest.json");
        let before = std::fs::metadata(&manifest).unwrap().modified().unwrap();

        materialize(dir.path()).unwrap();

        assert_eq!(
            std::fs::metadata(&manifest).unwrap().modified().unwrap(),
            before,
            "an unchanged file must not be rewritten"
        );
    }

    #[test]
    fn an_upgrade_overwrites_a_stale_copy_of_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("background.js"), "// from an old version").unwrap();

        materialize(dir.path()).unwrap();

        let written = std::fs::read_to_string(dir.path().join("background.js")).unwrap();
        assert!(written.contains("PROTOCOL_VERSION"));
    }

    #[test]
    fn the_extension_folder_sits_beside_the_browser_data_not_inside_a_version_dir() {
        // It has to be a stable path: Chrome derives an unpacked extension's ID
        // from its location, so a per-version folder would look like a new
        // extension after every upgrade.
        for dir in dir_candidates() {
            assert!(dir.ends_with(EXTENSION_SEGMENT), "{dir:?}");
            assert!(
                !dir.to_string_lossy().contains("chromium"),
                "{dir:?} must not sit under the versioned browser tree"
            );
        }
    }
}
