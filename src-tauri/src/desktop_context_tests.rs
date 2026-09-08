// SPDX-License-Identifier: Apache-2.0
use super::*;
use std::cell::Cell;

struct World {
    _temp: tempfile::TempDir,
    request: Request,
    observed: ObservedPaths,
}

impl World {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("codefactory-scenario-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let run = uuid::Uuid::new_v4().to_string();
        let owner = uuid::Uuid::new_v4().to_string();
        let identifier = format!("com.codefactory.scenario.{}", run.replace('-', ""));
        let home = root.join("home");
        let config = home.join("Library/Application Support");
        let cache = home.join("Library/Caches");
        for dir in [
            &home,
            &home.join("Library"),
            &config,
            &cache,
            &config.join(&identifier),
            &cache.join(&identifier),
            &root.join("tmp"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
            private_permissions(dir);
        }
        private_permissions(&root);
        let manifest = root.join("manifest.json");
        std::fs::write(
            &manifest,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1, "run_id": run, "owner_token": owner,
                "identifier": identifier, "capabilities": ["isolated_app_data"]
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            root.join("owner.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1, "run_id": run, "owner_token": owner
            }))
            .unwrap(),
        )
        .unwrap();
        Self {
            _temp: temp,
            request: Request {
                manifest,
                run_id: run,
                owner_token: owner,
                identifier,
            },
            observed: ObservedPaths {
                home: home.clone(),
                config: config.clone(),
                data: config,
                cache,
                env_home: home,
                tmp: root.join("tmp"),
                real_home: root.parent().unwrap().join("real-user"),
                temporary_root: root.parent().unwrap().to_path_buf(),
            },
        }
    }
    fn validate(&self) -> GuardResult<SyntheticContext> {
        validate(&self.request, &self.observed)
    }
}

fn private_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[test]
fn desktop_context_normal_secret_delegation_uses_only_injected_storage() {
    let calls = Cell::new(0);
    let mode = DesktopContext::Normal;
    assert_eq!(
        mode.read_secret(|| {
            calls.set(calls.get() + 1);
            Ok(Some("fake".into()))
        })
        .unwrap(),
        Some("fake".into())
    );
    mode.write_secret(|| {
        calls.set(calls.get() + 1);
        Ok(())
    })
    .unwrap();
    assert_eq!(calls.get(), 2);
    let mode = DesktopContext::Rejected;
    assert!(mode
        .read_secret(|| panic!("real storage must not run"))
        .is_err());
    assert!(mode
        .write_secret(|| panic!("real storage must not run"))
        .is_err());
}

#[test]
fn desktop_context_duplicate_initialization_is_permanently_rejected() {
    let mut state = None;
    assert!(initialize_once(&mut state, || Ok(DesktopContext::Normal)).is_ok());
    assert!(initialize_once(&mut state, || panic!("must not reinitialize")).is_err());
    assert!(matches!(state, Some(DesktopContext::Rejected)));
    assert!(initialize_once(&mut state, || Ok(DesktopContext::Normal)).is_err());
    let mut failed = None;
    assert!(initialize_once(&mut failed, || Err("bad manifest".into())).is_err());
    assert!(matches!(failed, Some(DesktopContext::Rejected)));
}

#[test]
fn desktop_context_cli_without_request_stays_normal_but_partial_requests_never_do() {
    let mut state = None;
    assert!(matches!(
        access_context(&mut state, false),
        DesktopContext::Normal
    ));
    assert!(state.is_none()); // Normal CLI access does not consume desktop initialization.
    assert!(matches!(
        access_context(&mut state, true),
        DesktopContext::Rejected
    ));
    assert!(matches!(
        access_context(&mut state, false),
        DesktopContext::Rejected
    ));
    assert!(
        request_from_values("com.codefactory.app", [None, None, None])
            .unwrap()
            .is_none()
    );
    assert!(request_from_values("com.codefactory.scenario.a", [None, None, None]).is_err());
    for mask in 1..7 {
        let values = std::array::from_fn(|i| {
            if mask & (1 << i) != 0 {
                Some("synthetic".into())
            } else {
                None
            }
        });
        assert!(
            request_from_values("com.codefactory.app", values).is_err(),
            "mask {mask}"
        );
    }
    assert!(access_context(&mut state, true)
        .read_secret(|| panic!("CLI cannot touch credentials with a world request"))
        .is_err());
}

#[test]
fn desktop_context_plugin_authority_is_default_deny_independent_of_invoke_handler() {
    let authority = synthetic_authority();
    let local = tauri::ipc::Origin::Local;
    assert!(authority
        .resolve_access("plugin:app|set_app_theme", "main", "main", &local)
        .is_some());
    assert!(authority
        .resolve_access("plugin:app|set_app_theme", "other", "other", &local)
        .is_none());
    for command in [
        "plugin:updater|check",
        "plugin:shell|open",
        "plugin:process|restart",
        "plugin:webview|create_webview",
        "plugin:path|resolve_directory",
        "plugin:event|emit",
        "plugin:app|set_app_theme_extra",
    ] {
        assert!(
            authority
                .resolve_access(command, "main", "main", &local)
                .is_none(),
            "{command}"
        );
    }
    assert!(authority
        .resolve_access(
            "plugin:app|set_app_theme",
            "main",
            "main",
            &tauri::ipc::Origin::Remote {
                url: "https://example.com".parse().unwrap()
            }
        )
        .is_none());
}

#[cfg(unix)]
#[test]
fn desktop_context_synthetic_credentials_do_not_call_any_storage() {
    let world = World::new();
    let mode = DesktopContext::Synthetic(Arc::new(world.validate().unwrap()));
    assert_eq!(
        mode.read_secret(|| panic!("keyring/fallback/legacy must not run"))
            .unwrap(),
        None
    );
    assert!(mode
        .write_secret(|| panic!("keyring/fallback/legacy must not run"))
        .is_err());
}

#[cfg(unix)]
#[test]
fn desktop_context_theme_roundtrip_and_invalid_settings_never_fall_back() {
    let world = World::new();
    let guard = world.validate().unwrap();
    let mut settings = crate::config::settings::load_synthetic(&guard).unwrap();
    assert!(settings.onboarded);
    assert!(settings.endpoints.is_empty());
    assert!(settings.mcp_servers.is_empty());
    settings.theme = crate::config::settings::Theme::Light;
    crate::config::settings::save_synthetic(&guard, &settings).unwrap();
    let reopened = world.validate().unwrap();
    assert!(matches!(
        crate::config::settings::load_synthetic(&reopened)
            .unwrap()
            .theme,
        crate::config::settings::Theme::Light
    ));
    settings.im_webhook_url = "https://must-not-send.invalid".into();
    assert!(crate::config::settings::save_synthetic(&guard, &settings).is_err());
    let path = guard.settings_path().unwrap();
    std::fs::write(&path, b"broken json").unwrap();
    assert!(crate::config::settings::load_synthetic(&guard).is_err());
    std::fs::write(
        &path,
        br#"{"schema_version":1,"theme":"dark","unknown":true}"#,
    )
    .unwrap();
    assert!(crate::config::settings::load_synthetic(&guard).is_err());
}

#[cfg(unix)]
#[test]
fn desktop_context_rejects_wrong_manifest_identity_and_real_roots() {
    let mut world = World::new();
    let original = world.request.clone();
    world.request.identifier = "com.codefactory.app".into();
    assert!(world.validate().is_err());
    world.request = original.clone();
    world.request.owner_token = uuid::Uuid::new_v4().to_string();
    assert!(world.validate().is_err());
    world.request = original.clone();
    world.request.run_id = uuid::Uuid::new_v4().to_string();
    assert!(world.validate().is_err());
    world.request = original;
    let bytes = std::fs::read(&world.request.manifest).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    manifest["schema_version"] = 2.into();
    std::fs::write(
        &world.request.manifest,
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert!(world.validate().is_err());
    std::fs::write(&world.request.manifest, bytes).unwrap();
    world.observed.real_home = world.request.manifest.parent().unwrap().to_path_buf();
    assert!(world.validate().is_err());
}

#[cfg(unix)]
#[test]
fn desktop_context_rejects_each_actual_path_mismatch() {
    for index in 0..6 {
        let mut world = World::new();
        let target = match index {
            0 => &mut world.observed.home,
            1 => &mut world.observed.config,
            2 => &mut world.observed.data,
            3 => &mut world.observed.cache,
            4 => &mut world.observed.env_home,
            _ => &mut world.observed.tmp,
        };
        *target = target.join("wrong");
        assert!(world.validate().is_err(), "path {index}");
    }
}

#[cfg(unix)]
#[test]
fn desktop_context_rejects_links_replacements_and_manifest_tampering() {
    use std::os::unix::fs::symlink;
    let world = World::new();
    let guard = world.validate().unwrap();
    let path = guard.settings_path().unwrap();
    let outside = world._temp.path().join("outside.json");
    std::fs::write(&outside, "untouched").unwrap();
    symlink(&outside, &path).unwrap();
    assert!(guard.settings_path().is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::hard_link(&outside, &path).unwrap();
    assert!(guard.settings_path().is_err());
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "untouched");

    let world = World::new();
    let guard = world.validate().unwrap();
    let cache = world.observed.cache.join(&world.request.identifier);
    std::fs::rename(&cache, cache.with_extension("old")).unwrap();
    std::fs::create_dir(&cache).unwrap();
    private_permissions(&cache);
    assert!(guard.verify().is_err());

    let world = World::new();
    let guard = world.validate().unwrap();
    std::fs::write(&world.request.manifest, b"{}").unwrap();
    assert!(guard.verify().is_err());
}

#[test]
fn desktop_context_only_theme_ipc_and_local_navigation_are_admitted() {
    assert!(command_allowed("get_settings"));
    assert!(command_allowed("save_settings"));
    for name in [
        "chat",
        "codex_account",
        "get_models",
        "delivery_channel_status",
        "browser_session",
        "plugin:updater|check",
        "plugin:shell|open",
        "plugin:process|restart",
        "plugin:webview|create_webview",
        "unknown",
    ] {
        assert!(!command_allowed(name), "{name}");
    }
    assert!(navigation_allowed(
        &"tauri://localhost/index.html".parse().unwrap()
    ));
    assert!(!navigation_allowed(&"https://example.com".parse().unwrap()));
    assert!(!navigation_allowed(&"file:///etc/passwd".parse().unwrap()));
    assert!(!navigation_allowed(
        &"http://localhost:1420".parse().unwrap()
    ));
}

#[test]
fn desktop_context_unknown_ipc_fields_are_not_silently_discarded() {
    let settings = crate::config::settings::Settings::default();
    let payload = serde_json::json!({"newSettings": settings});
    assert!(parse_settings_payload(&payload).is_ok());
    for level in 0..3 {
        let mut value = payload.clone();
        let target = match level {
            0 => &mut value,
            1 => &mut value["newSettings"],
            _ => &mut value["newSettings"]["permissions"],
        };
        target["unknown"] = true.into();
        assert!(
            parse_settings_payload(&value).is_err(),
            "unknown field at level {level}"
        );
    }
}
