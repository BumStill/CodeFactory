// SPDX-License-Identifier: Apache-2.0
//! Deliberately narrow, same-user accident-prevention boundary, not an OS sandbox.
//! The synthetic branch never enters the normal setup or command dispatcher.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::Deserialize;

type GuardResult<T> = Result<T, String>;
const MANIFEST_ENV: &str = "CODEFACTORY_SCENARIO_MANIFEST";
const RUN_ENV: &str = "CODEFACTORY_SCENARIO_RUN_ID";
const OWNER_ENV: &str = "CODEFACTORY_SCENARIO_OWNER_TOKEN";
static CONTEXT: Mutex<Option<DesktopContext>> = Mutex::new(None);

#[derive(Clone)]
pub(crate) enum DesktopContext {
    Normal,
    Synthetic(Arc<SyntheticContext>),
    Rejected,
}

#[derive(Clone)]
struct Request {
    manifest: PathBuf,
    run_id: String,
    owner_token: String,
    identifier: String,
}

struct ObservedPaths {
    home: PathBuf,
    env_home: PathBuf,
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
    tmp: PathBuf,
    real_home: PathBuf,
    temporary_root: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    run_id: String,
    owner_token: String,
    identifier: String,
    capabilities: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    schema_version: u32,
    run_id: String,
    owner_token: String,
}

#[derive(PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
}

pub(crate) struct SyntheticContext {
    directories: Vec<(PathBuf, Identity)>,
    files: Vec<(PathBuf, Identity, Vec<u8>)>,
    settings: PathBuf,
    identifier: String,
}

fn identity(path: &Path, directory: bool) -> GuardResult<Identity> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "isolated path unavailable")?;
    metadata_identity(&metadata, directory)
}

fn metadata_identity(metadata: &std::fs::Metadata, directory: bool) -> GuardResult<Identity> {
    if metadata.file_type().is_symlink()
        || metadata.is_dir() != directory
        || (!directory && !metadata.is_file())
    {
        return Err("isolated path has wrong type or is a link".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
            || (!directory && metadata.nlink() != 1)
        {
            return Err("isolated path ownership, permissions or link count invalid".into());
        }
        Ok(Identity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(not(unix))]
    Err("synthetic desktop file identity is not implemented on this platform".into())
}

fn read_sealed_file(path: &Path) -> GuardResult<(Identity, Vec<u8>)> {
    use std::io::Read;
    let before = identity(path, false)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|_| "isolated file could not be opened")?;
    if before
        != metadata_identity(
            &file.metadata().map_err(|_| "isolated handle unavailable")?,
            false,
        )?
    {
        return Err("isolated opened file identity changed".into());
    }
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| "isolated file unreadable")?;
    if bytes.len() > 65536 || before != identity(path, false)? {
        return Err("isolated file changed or exceeds the size limit".into());
    }
    Ok((before, bytes))
}

fn validate(request: &Request, observed: &ObservedPaths) -> GuardResult<SyntheticContext> {
    let run = uuid::Uuid::parse_str(&request.run_id).map_err(|_| "invalid run ID")?;
    let owner = uuid::Uuid::parse_str(&request.owner_token).map_err(|_| "invalid owner token")?;
    if run.to_string() != request.run_id
        || owner.to_string() != request.owner_token
        || request.identifier != format!("com.codefactory.scenario.{}", run.simple())
    {
        return Err("synthetic identifier/run/owner mismatch".into());
    }
    let root = request.manifest.parent().ok_or("missing world root")?;
    if !root.is_absolute()
        || root.parent() != Some(observed.temporary_root.as_path())
        || !root
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.starts_with("codefactory-scenario-"))
        || root.canonicalize().map_err(|_| "world root unavailable")? != root
        || root.starts_with(&observed.real_home)
        || observed.real_home.starts_with(root)
        || request.manifest != root.join("manifest.json")
    {
        return Err("world root is not an independent canonical temporary directory".into());
    }
    let home = root.join("home");
    let library = home.join("Library");
    let config = library.join("Application Support");
    let cache = library.join("Caches");
    if observed.home != home
        || observed.env_home != home
        || observed.config != config
        || observed.data != config
        || observed.cache != cache
        || observed.tmp != root.join("tmp")
    {
        return Err("actual HOME/config/data/cache/tmp do not match the isolated layout".into());
    }
    let app_data = config.join(&request.identifier);
    let app_cache = cache.join(&request.identifier);
    let directories = [
        root.to_path_buf(),
        home,
        library,
        config,
        cache,
        app_data.clone(),
        app_cache,
        root.join("tmp"),
    ]
    .into_iter()
    .map(|path| identity(&path, true).map(|id| (path, id)))
    .collect::<GuardResult<Vec<_>>>()?;
    let mut files = Vec::new();
    for path in [request.manifest.clone(), root.join("owner.json")] {
        let (id, bytes) = read_sealed_file(&path)?;
        files.push((path, id, bytes));
    }
    let manifest: Manifest =
        serde_json::from_slice(&files[0].2).map_err(|_| "invalid manifest schema")?;
    let owner: Owner = serde_json::from_slice(&files[1].2).map_err(|_| "invalid owner schema")?;
    if manifest.schema_version != 1
        || owner.schema_version != 1
        || manifest.run_id != request.run_id
        || owner.run_id != request.run_id
        || manifest.owner_token != request.owner_token
        || owner.owner_token != request.owner_token
        || manifest.identifier != request.identifier
        || manifest.capabilities != ["isolated_app_data"]
    {
        return Err("manifest/owner identity or capability mismatch".into());
    }
    let guard = SyntheticContext {
        directories,
        files,
        settings: app_data.join("settings.json"),
        identifier: request.identifier.clone(),
    };
    guard.settings_path()?;
    Ok(guard)
}

impl SyntheticContext {
    pub(crate) fn verify(&self) -> GuardResult<()> {
        for (path, expected) in &self.directories {
            if identity(path, true)? != *expected {
                return Err("isolated directory was replaced".into());
            }
        }
        for (path, expected, bytes) in &self.files {
            let (actual, current) = read_sealed_file(path)?;
            if actual != *expected || current != *bytes {
                return Err("isolated manifest or owner was replaced".into());
            }
        }
        Ok(())
    }

    pub(crate) fn settings_path(&self) -> GuardResult<PathBuf> {
        self.verify()?;
        match std::fs::symlink_metadata(&self.settings) {
            Ok(_) => {
                identity(&self.settings, false)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("isolated settings unavailable".into()),
        }
        Ok(self.settings.clone())
    }

    pub(crate) fn read_settings(&self) -> GuardResult<Option<Vec<u8>>> {
        let path = self.settings_path()?;
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("isolated settings unavailable".into()),
            Ok(_) => read_sealed_file(&path).map(|(_, bytes)| Some(bytes)),
        }
    }
}

fn initialize_once(
    state: &mut Option<DesktopContext>,
    validate: impl FnOnce() -> GuardResult<DesktopContext>,
) -> GuardResult<DesktopContext> {
    if state.is_some() {
        *state = Some(DesktopContext::Rejected);
        return Err("desktop context was already initialized".into());
    }
    *state = Some(DesktopContext::Rejected);
    let context = validate()?;
    *state = Some(context.clone());
    Ok(context)
}

fn request_from_env(identifier: &str) -> GuardResult<Option<Request>> {
    let values = [MANIFEST_ENV, RUN_ENV, OWNER_ENV].map(std::env::var_os);
    request_from_values(identifier, values)
}

fn request_from_values(
    identifier: &str,
    values: [Option<std::ffi::OsString>; 3],
) -> GuardResult<Option<Request>> {
    if values.iter().all(Option::is_none) {
        return if identifier.starts_with("com.codefactory.scenario.") {
            Err("synthetic identifier requires a complete world request".into())
        } else {
            Ok(None)
        };
    }
    let [manifest, run, owner] = values;
    Ok(Some(Request {
        manifest: PathBuf::from(manifest.ok_or("missing synthetic manifest")?),
        run_id: run
            .and_then(|v| v.into_string().ok())
            .ok_or("missing synthetic run ID")?,
        owner_token: owner
            .and_then(|v| v.into_string().ok())
            .ok_or("missing synthetic owner token")?,
        identifier: identifier.into(),
    }))
}

#[cfg(target_os = "macos")]
fn observed_paths() -> GuardResult<ObservedPaths> {
    // passwd is independent of the caller's HOME override. No credential read.
    let real_home = unsafe {
        let mut entry: libc::passwd = std::mem::zeroed();
        let mut result = std::ptr::null_mut();
        let mut buffer = vec![0u8; 16384];
        if libc::getpwuid_r(
            libc::geteuid(),
            &mut entry,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        ) != 0
            || result.is_null()
            || entry.pw_dir.is_null()
        {
            return Err("OS user root unavailable".into());
        }
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(
            std::ffi::CStr::from_ptr(entry.pw_dir).to_bytes(),
        ))
    };
    Ok(ObservedPaths {
        home: dirs::home_dir().ok_or("HOME unavailable")?,
        env_home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME missing")?,
        config: dirs::config_dir().ok_or("config root unavailable")?,
        data: dirs::data_dir().ok_or("data root unavailable")?,
        cache: dirs::cache_dir().ok_or("cache root unavailable")?,
        tmp: std::env::temp_dir(),
        real_home,
        temporary_root: Path::new("/tmp")
            .canonicalize()
            .map_err(|_| "temporary root unavailable")?,
    })
}

pub(crate) fn initialize(context: &tauri::Context<tauri::Wry>) -> GuardResult<DesktopContext> {
    let mut state = CONTEXT
        .lock()
        .map_err(|_| "desktop context lock poisoned")?;
    initialize_once(&mut state, || {
        let Some(request) = request_from_env(&context.config().identifier)? else {
            return Ok(DesktopContext::Normal);
        };
        #[cfg(target_os = "macos")]
        {
            Ok(DesktopContext::Synthetic(Arc::new(validate(
                &request,
                &observed_paths()?,
            )?)))
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = request;
            Err("synthetic desktop startup is only reviewed for macOS".into())
        }
    })
}

pub(crate) fn current() -> DesktopContext {
    match CONTEXT.lock() {
        Ok(mut state) => {
            // Existing non-desktop CLI paths keep normal behavior, but a world
            // request can never enable credentials before validation.
            access_context(
                &mut state,
                [MANIFEST_ENV, RUN_ENV, OWNER_ENV]
                    .iter()
                    .any(|key| std::env::var_os(key).is_some()),
            )
        }
        Err(_) => DesktopContext::Rejected,
    }
}

pub(crate) fn reject_world_request_for_cli() {
    // These legacy smokes are not Scenario World desktop entrypoints. Even a
    // complete world request cannot authorize their independent side effects.
    if [MANIFEST_ENV, RUN_ENV, OWNER_ENV]
        .iter()
        .any(|key| std::env::var_os(key).is_some())
        || !matches!(current(), DesktopContext::Normal)
    {
        eprintln!("Scenario World requests cannot run this legacy CLI smoke");
        std::process::exit(2);
    }
}

fn access_context(state: &mut Option<DesktopContext>, has_world_request: bool) -> DesktopContext {
    if state.is_none() && has_world_request {
        *state = Some(DesktopContext::Rejected);
    }
    state.clone().unwrap_or(DesktopContext::Normal)
}

impl DesktopContext {
    fn read_secret(
        &self,
        real: impl FnOnce() -> crate::errors::Result<Option<String>>,
    ) -> crate::errors::Result<Option<String>> {
        match self {
            Self::Normal => real(),
            Self::Synthetic(context) => {
                context.verify().map_err(crate::errors::AppError::Other)?;
                Ok(None)
            }
            Self::Rejected => Err(crate::errors::AppError::Other(
                "desktop context rejected credential access".into(),
            )),
        }
    }
    fn write_secret(
        &self,
        real: impl FnOnce() -> crate::errors::Result<()>,
    ) -> crate::errors::Result<()> {
        match self {
            Self::Normal => real(),
            _ => Err(crate::errors::AppError::Other(
                "synthetic or rejected desktop cannot mutate credentials".into(),
            )),
        }
    }
}

pub(crate) fn read_secret(
    real: impl FnOnce() -> crate::errors::Result<Option<String>>,
) -> crate::errors::Result<Option<String>> {
    current().read_secret(real)
}
pub(crate) fn write_secret(
    real: impl FnOnce() -> crate::errors::Result<()>,
) -> crate::errors::Result<()> {
    current().write_secret(real)
}

fn command_allowed(command: &str) -> bool {
    matches!(command, "get_settings" | "save_settings")
}

fn parse_settings_payload(
    value: &serde_json::Value,
) -> GuardResult<crate::config::settings::Settings> {
    if value.as_object().is_none_or(|object| object.len() != 1) {
        return Err("synthetic settings payload must contain only newSettings".into());
    }
    let requested = value.get("newSettings").ok_or("missing newSettings")?;
    let settings: crate::config::settings::Settings =
        serde_json::from_value(requested.clone()).map_err(|_| "invalid synthetic settings")?;
    if serde_json::to_value(&settings).map_err(|_| "invalid synthetic settings")? != *requested {
        return Err("unknown, omitted or noncanonical synthetic settings fields".into());
    }
    Ok(settings)
}

// DOM accident-prevention only. This does not revoke OS clipboard/media grants.
const SYNTHETIC_INPUT_GUARD: &str = r#"
(() => {
  const deny = event => { event.preventDefault(); event.stopImmediatePropagation(); };
  for (const name of ['paste', 'drop', 'dragenter', 'dragover']) {
    window.addEventListener(name, deny, true);
  }
  window.addEventListener('beforeinput', event => {
    if (event.inputType === 'insertFromPaste' || event.inputType === 'insertFromDrop') deny(event);
  }, true);
  window.addEventListener('click', event => {
    if (event.target?.closest?.('input[type="file"]')) deny(event);
  }, true);
})();
"#;

fn navigation_allowed(url: &url::Url) -> bool {
    url.scheme() == "tauri"
        && url.host_str() == Some("localhost")
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

fn synthetic_authority() -> tauri::ipc::RuntimeAuthority {
    // Plugin invokes bypass the application's invoke_handler. Replace the
    // generated authority, do not just clear config capabilities after build.
    let mut resolved = tauri::utils::acl::resolved::Resolved::default();
    resolved.allowed_commands.insert(
        "plugin:app|set_app_theme".into(),
        vec![tauri::utils::acl::resolved::ResolvedCommand {
            context: tauri::utils::acl::ExecutionContext::Local,
            windows: vec!["main".parse().expect("fixed window glob")],
            ..Default::default()
        }],
    );
    tauri::runtime_authority!(Default::default(), resolved)
}

fn harden_context(context: &mut tauri::Context<tauri::Wry>) {
    *context.runtime_authority_mut() = synthetic_authority();
    let config = context.config_mut();
    config.build.dev_url = None; // Never connect to a dev server with user state.
    config.app.windows.clear(); // Create the audited WebView explicitly below.
    config.app.security.asset_protocol.enable = false;
    config.app.security.capabilities.clear();
    let csp = tauri::utils::config::Csp::Policy("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; connect-src ipc: http://ipc.localhost; object-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'none'".into());
    config.app.security.csp = Some(csp.clone());
    config.app.security.dev_csp = Some(csp);
}

pub(crate) fn run_synthetic(
    mut context: tauri::Context<tauri::Wry>,
    guard: Arc<SyntheticContext>,
) -> GuardResult<()> {
    guard.verify()?;
    crate::config::settings::load_synthetic(&guard)?; // Bad state fails before WebView.
    if context.assets().get(&"index.html".into()).is_none() {
        return Err(
            "synthetic desktop requires embedded frontend assets; dev server is forbidden".into(),
        );
    }
    if context.config().identifier != guard.identifier {
        return Err("context identifier changed".into());
    }
    harden_context(&mut context);
    let ipc_guard = guard.clone();
    tauri::Builder::default()
        .invoke_handler(move |invoke| {
            let command = invoke.message.command();
            if !command_allowed(command) {
                invoke
                    .resolver
                    .reject("command is not reviewed for synthetic desktop");
                return true;
            }
            if let Err(error) = ipc_guard.verify() {
                invoke.resolver.reject(error);
                return true;
            }
            let result = if command == "get_settings" {
                crate::config::settings::load_synthetic(&ipc_guard)
            } else {
                let parsed = match invoke.message.payload() {
                    tauri::ipc::InvokeBody::Json(value) => parse_settings_payload(value),
                    _ => Err("invalid synthetic payload".into()),
                };
                parsed.and_then(|settings| {
                    crate::config::settings::save_synthetic(&ipc_guard, &settings).map(|_| settings)
                })
            };
            match result {
                Ok(settings) => invoke.resolver.resolve(settings),
                Err(error) => invoke.resolver.reject(error),
            }
            true
        })
        .setup(move |app| {
            guard.verify().map_err(std::io::Error::other)?;
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("CodeFactory — Synthetic Scenario")
            .inner_size(1200.0, 800.0)
            .incognito(true)
            .initialization_script(SYNTHETIC_INPUT_GUARD)
            .on_navigation(navigation_allowed)
            .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
            .on_download(|_, _| false)
            .build()?;
            Ok(())
        })
        .run(context)
        .map_err(|error| format!("synthetic desktop failed: {error}"))
}

#[cfg(test)]
#[path = "desktop_context_tests.rs"]
mod tests;
