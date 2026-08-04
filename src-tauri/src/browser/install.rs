// SPDX-License-Identifier: Apache-2.0
//! On-demand Chromium install.
//!
//! The app ships without a browser and fetches one the first time the user
//! turns browser control on. That keeps the installer light without pushing
//! the setup onto the user: a ~150 MB download has to happen either way, and
//! the app is in a far better position to run it than a person following
//! terminal instructions — it can show progress, retry a broken transfer, and
//! repair an install that was interrupted half-way.
//!
//! We use [Chrome for Testing], Google's distribution built for automation:
//! stable, versioned URLs and a published index of known-good builds, rather
//! than scraping a CDN path that can move under us.
//!
//! Everything in this module except the actual transfer is pure, so the layout,
//! detection, and repair rules are tested without touching the network.
//!
//! [Chrome for Testing]: https://googlechromelabs.github.io/chrome-for-testing/

use std::path::{Path, PathBuf};

/// Index of known-good builds, keyed by channel and platform.
pub const VERSIONS_URL: &str =
    "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";

const DOWNLOAD_BASE: &str = "https://storage.googleapis.com/chrome-for-testing-public";

/// Written next to the extracted browser so a later run knows what it has.
const MARKER: &str = ".codefactory-chromium-version";

/// A platform Chrome for Testing publishes builds for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux64,
    MacArm64,
    MacX64,
    Win64,
}

impl Platform {
    /// The identifier Chrome for Testing uses in URLs and its version index.
    pub fn id(self) -> &'static str {
        match self {
            Self::Linux64 => "linux64",
            Self::MacArm64 => "mac-arm64",
            Self::MacX64 => "mac-x64",
            Self::Win64 => "win64",
        }
    }

    /// Detect the platform this build is running on.
    ///
    /// `None` on targets Chrome for Testing does not publish (32-bit Windows,
    /// non-x86/ARM Linux). Callers surface that as "browser control isn't
    /// available on this platform" rather than failing mid-download.
    pub fn current() -> Option<Self> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => Some(Self::MacArm64),
            ("macos", "x86_64") => Some(Self::MacX64),
            ("linux", "x86_64") => Some(Self::Linux64),
            ("windows", "x86_64") => Some(Self::Win64),
            _ => None,
        }
    }

    /// Path of the executable inside the extracted archive.
    ///
    /// Chrome for Testing archives unpack to a single `chrome-<platform>/`
    /// directory; on macOS the binary sits inside an app bundle.
    pub fn binary_relative_path(self) -> PathBuf {
        let root = format!("chrome-{}", self.id());
        match self {
            Self::Linux64 => Path::new(&root).join("chrome"),
            Self::Win64 => Path::new(&root).join("chrome.exe"),
            Self::MacArm64 | Self::MacX64 => Path::new(&root)
                .join("Google Chrome for Testing.app")
                .join("Contents")
                .join("MacOS")
                .join("Google Chrome for Testing"),
        }
    }
}

/// Directory segment used for browser cache storage.
const BROWSER_CACHE_SEGMENTS: &[&str] = &["browser", "chromium"];

/// Where downloaded browsers live.
///
/// On Windows installed apps may run from `Program Files` and user profiles may
/// be redirected or locked down; the fallback browser must therefore live under
/// the per-user LocalAppData tree (`%LOCALAPPDATA%\CodeFactory\browser\chromium`)
/// instead of the app install directory or process working directory.
/// Non-Windows keeps the historical `~/.codefactory/browser/chromium` location.
pub fn install_root() -> Option<PathBuf> {
    install_root_from_dirs(dirs::data_local_dir(), dirs::home_dir())
}

fn install_root_from_dirs(local_data_dir: Option<PathBuf>, home_dir: Option<PathBuf>) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        local_data_dir
            .map(|dir| append_segments(dir.join("CodeFactory"), BROWSER_CACHE_SEGMENTS))
            .or_else(|| {
                home_dir.map(|home| append_segments(home.join(".codefactory"), BROWSER_CACHE_SEGMENTS))
            })
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = local_data_dir;
        home_dir.map(|home| append_segments(home.join(".codefactory"), BROWSER_CACHE_SEGMENTS))
    }
}

fn append_segments(mut root: PathBuf, segments: &[&str]) -> PathBuf {
    for segment in segments {
        root.push(segment);
    }
    root
}

/// Directory for one version, so an upgrade doesn't clobber a working install.
pub fn version_dir(root: &Path, version: &str) -> PathBuf {
    root.join(version)
}

/// Download URL for a version + platform.
pub fn download_url(version: &str, platform: Platform) -> String {
    format!(
        "{DOWNLOAD_BASE}/{version}/{platform_id}/chrome-{platform_id}.zip",
        platform_id = platform.id()
    )
}

/// A usable browser on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromiumInstall {
    pub version: String,
    pub binary: PathBuf,
}

/// What [`detect`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallState {
    Ready(ChromiumInstall),
    /// Nothing usable — the caller should download. `previous` names a version
    /// whose directory exists but is unusable, so the UI can say "repairing"
    /// instead of "installing".
    Missing { previous: Option<String> },
}

/// Look for a usable browser under `root`.
///
/// A version marker alone is not enough: an install interrupted part-way
/// leaves the directory and sometimes the marker behind, so the binary itself
/// must be present. Reporting that as `Missing` is what makes a broken install
/// self-healing rather than a permanent error.
pub fn detect(root: &Path, platform: Platform) -> InstallState {
    let Some(version) = read_marker(root) else {
        return InstallState::Missing { previous: None };
    };
    let binary = version_dir(root, &version).join(platform.binary_relative_path());
    if binary.is_file() {
        InstallState::Ready(ChromiumInstall { version, binary })
    } else {
        InstallState::Missing {
            previous: Some(version),
        }
    }
}

/// Record a completed install. Written only after the binary is in place, so a
/// crashed download can never look like a finished one.
pub fn write_marker(root: &Path, version: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(root)?;
    std::fs::write(root.join(MARKER), version)
}

fn read_marker(root: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(root.join(MARKER)).ok()?;
    let version = raw.trim();
    // Reject anything that isn't a plain version, so a corrupted marker can't
    // steer path construction.
    let ok = !version.is_empty()
        && version.len() <= 32
        && version
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' );
    ok.then(|| version.to_string())
}

/// Pick this platform's download entry out of the Chrome for Testing index.
///
/// Kept separate from the HTTP call so the parsing is testable against a
/// captured payload.
pub fn download_url_from_index(
    index: &serde_json::Value,
    channel: &str,
    platform: Platform,
) -> Option<(String, String)> {
    let channel = index.get("channels")?.get(channel)?;
    let version = channel.get("version")?.as_str()?.to_string();
    let url = channel
        .get("downloads")?
        .get("chrome")?
        .as_array()?
        .iter()
        .find(|entry| entry.get("platform").and_then(|p| p.as_str()) == Some(platform.id()))?
        .get("url")?
        .as_str()?
        .to_string();
    Some((version, url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn index() -> serde_json::Value {
        json!({
            "channels": {
                "Stable": {
                    "version": "151.0.7922.47",
                    "downloads": {
                        "chrome": [
                            {"platform": "linux64", "url": "https://example/linux64.zip"},
                            {"platform": "mac-arm64", "url": "https://example/mac-arm64.zip"}
                        ]
                    }
                }
            }
        })
    }

    #[test]
    fn the_stable_entry_for_this_platform_is_selected() {
        let (version, url) =
            download_url_from_index(&index(), "Stable", Platform::MacArm64).expect("entry");
        assert_eq!(version, "151.0.7922.47");
        assert_eq!(url, "https://example/mac-arm64.zip");
    }

    #[test]
    fn a_platform_the_index_does_not_carry_is_not_guessed() {
        // Better to report "no build for this platform" than to fabricate a URL
        // and fail with a 404 halfway through a download.
        assert!(download_url_from_index(&index(), "Stable", Platform::Win64).is_none());
        assert!(download_url_from_index(&index(), "Canary", Platform::MacArm64).is_none());
    }

    #[test]
    fn download_urls_follow_the_published_layout() {
        assert_eq!(
            download_url("151.0.7922.47", Platform::Linux64),
            "https://storage.googleapis.com/chrome-for-testing-public/151.0.7922.47/linux64/chrome-linux64.zip"
        );
    }

    #[test]
    fn each_platform_knows_where_its_executable_lands() {
        assert_eq!(
            Platform::Linux64.binary_relative_path(),
            Path::new("chrome-linux64").join("chrome")
        );
        assert_eq!(
            Platform::Win64.binary_relative_path(),
            Path::new("chrome-win64").join("chrome.exe")
        );
        assert!(Platform::MacArm64
            .binary_relative_path()
            .ends_with("Google Chrome for Testing"));
    }

    #[test]
    fn nothing_installed_reports_missing() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            detect(root.path(), Platform::Linux64),
            InstallState::Missing { previous: None }
        );
    }

    #[test]
    fn a_complete_install_is_detected() {
        let root = tempfile::tempdir().unwrap();
        let binary = version_dir(root.path(), "151.0.7922.47")
            .join(Platform::Linux64.binary_relative_path());
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, "#!/bin/sh\n").unwrap();
        write_marker(root.path(), "151.0.7922.47").unwrap();

        assert_eq!(
            detect(root.path(), Platform::Linux64),
            InstallState::Ready(ChromiumInstall {
                version: "151.0.7922.47".into(),
                binary,
            })
        );
    }

    #[test]
    fn an_interrupted_install_repairs_instead_of_failing_forever() {
        // Marker present, binary never finished extracting. This must read as
        // "download again", not as a usable install.
        let root = tempfile::tempdir().unwrap();
        write_marker(root.path(), "151.0.7922.47").unwrap();

        assert_eq!(
            detect(root.path(), Platform::Linux64),
            InstallState::Missing {
                previous: Some("151.0.7922.47".into())
            }
        );
    }

    #[test]
    fn a_corrupted_marker_cannot_steer_path_construction() {
        let root = tempfile::tempdir().unwrap();
        for bad in ["../../etc", "151.0/../..", "", "  ", "not-a-version"] {
            std::fs::write(root.path().join(MARKER), bad).unwrap();
            assert_eq!(
                detect(root.path(), Platform::Linux64),
                InstallState::Missing { previous: None },
                "marker {bad:?} must be rejected"
            );
        }
    }


    #[test]
    fn install_root_policy_uses_windows_local_app_data_when_available() {
        let local = PathBuf::from(r"C:\Users\Ada\AppData\Local");
        let home = PathBuf::from(r"C:\Users\Ada");
        let root = install_root_from_dirs(Some(local), Some(home)).expect("install root");

        #[cfg(target_os = "windows")]
        assert_eq!(
            root,
            PathBuf::from(r"C:\Users\Ada\AppData\Local")
                .join("CodeFactory")
                .join("browser")
                .join("chromium")
        );
        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            root,
            PathBuf::from(r"C:\Users\Ada")
                .join(".codefactory")
                .join("browser")
                .join("chromium")
        );
    }

    #[test]
    fn install_root_policy_never_depends_on_the_current_working_directory() {
        let cwd = std::env::current_dir().unwrap();
        let local = cwd.join("Program Files").join("CodeFactory");
        let home = PathBuf::from(r"C:\Users\Ada");
        let root = install_root_from_dirs(Some(local.clone()), Some(home.clone())).expect("install root");

        #[cfg(target_os = "windows")]
        {
            assert!(root.starts_with(&local));
            assert!(root.ends_with(Path::new("CodeFactory").join("browser").join("chromium")));
        }
        #[cfg(not(target_os = "windows"))]
        assert!(root.starts_with(&home));
    }

    #[test]
    fn versions_live_in_their_own_directories() {
        // An upgrade must not overwrite a working install in place; if the new
        // download breaks, the old directory is still intact.
        let root = Path::new("/tmp/root");
        assert_ne!(
            version_dir(root, "151.0.7922.47"),
            version_dir(root, "152.0.1000.1")
        );
    }
}
