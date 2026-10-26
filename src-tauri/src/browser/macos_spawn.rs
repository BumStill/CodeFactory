// SPDX-License-Identifier: Apache-2.0
//! macOS browser spawn: never attribute Chrome's bundle writes to CodeFactory.

use std::{ffi::CString, io, os::unix::ffi::OsStrExt, path::Path};

type Disclaim = unsafe extern "C" fn(*mut libc::posix_spawnattr_t, libc::c_int) -> libc::c_int;

pub(crate) struct LaunchPlan {
    pub args: Vec<String>,
    pub disclaim: bool,
}

impl LaunchPlan {
    pub fn managed(profile: &Path, port: u16, headless: bool) -> Self {
        let mut args = vec![
            format!("--user-data-dir={}", profile.display()),
            format!("--remote-debugging-port={port}"),
            "--remote-debugging-address=127.0.0.1".into(),
            "--disable-background-networking".into(),
            "--enable-features=NetworkService,NetworkServiceInProcess".into(),
            "--disable-client-side-phishing-detection".into(),
            "--disable-component-extensions-with-background-pages".into(),
            "--disable-features=TranslateUI".into(),
            "--disable-hang-monitor".into(),
            "--disable-ipc-flooding-protection".into(),
            "--disable-popup-blocking".into(),
            "--disable-prompt-on-repost".into(),
            "--force-color-profile=srgb".into(),
            "--enable-automation".into(),
            "--enable-blink-features=IdleDetection".into(),
            "--lang=en_US".into(),
            "--disable-background-timer-throttling".into(),
            "--disable-backgrounding-occluded-windows".into(),
            "--disable-renderer-backgrounding".into(),
            "--disable-breakpad".into(),
            "--disable-component-update".into(),
            "--disable-default-apps".into(),
            "--disable-extensions".into(),
            "--disable-sync".into(),
            "--disable-dev-shm-usage".into(),
            "--metrics-recording-only".into(),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--password-store=basic".into(),
            "--use-mock-keychain".into(),
            "about:blank".into(),
        ];
        if headless {
            args.extend(
                [
                    "--headless",
                    "--hide-scrollbars",
                    "--mute-audio",
                    "--no-sandbox",
                    "--disable-gpu",
                ]
                .map(String::from),
            );
        }
        Self {
            args,
            disclaim: true,
        }
    }
}

fn apply_disclaim(symbol: Option<Disclaim>, attr: *mut libc::posix_spawnattr_t) -> bool {
    match symbol {
        Some(set) if unsafe { set(attr, 1) } == 0 => true,
        _ => {
            tracing::warn!(
                "browser responsibility disclaim unavailable; falling back to ordinary posix_spawn"
            );
            false
        }
    }
}

/// Owns only a child launched here. Drop is the cancellation/early-error backstop.
pub(crate) struct Child {
    pub pid: libc::pid_t,
    reaped: bool,
}
impl Child {
    pub fn stop(&mut self) -> io::Result<()> {
        if self.reaped {
            return Ok(());
        }
        unsafe {
            let mut status = 0;
            let waited = libc::waitpid(self.pid, &mut status, libc::WNOHANG);
            if waited == self.pid {
                // Chrome's main process can exit before its helpers do.
                libc::kill(-self.pid, libc::SIGKILL);
                self.reaped = true;
                return Ok(());
            }
            if waited == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
                self.reaped = true;
                return Ok(());
            }
            // Kill the isolated browser process group, not any user-owned Chrome.
            if libc::kill(-self.pid, libc::SIGKILL) != 0
                && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            {
                return Err(io::Error::last_os_error());
            }
            loop {
                let result = libc::waitpid(self.pid, &mut status, 0);
                if result == self.pid
                    || (result == -1
                        && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD))
                {
                    break;
                }
                if io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        self.reaped = true;
        Ok(())
    }
    /// The extensions page belongs to the user; reap without killing on return.
    pub fn detach(mut self) {
        self.reaped = true;
        let pid = self.pid;
        std::thread::spawn(move || unsafe {
            while libc::waitpid(pid, std::ptr::null_mut(), 0) < 0 {
                if io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    break;
                }
            }
        });
    }
}
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub(crate) fn spawn(executable: &Path, args: &[String]) -> io::Result<Child> {
    let address = unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            c"responsibility_spawnattrs_setdisclaim".as_ptr(),
        )
    };
    let symbol = if address.is_null() {
        None
    } else {
        Some(unsafe { std::mem::transmute::<*mut libc::c_void, Disclaim>(address) })
    };
    spawn_with_symbol(executable, args, symbol)
}

fn spawn_with_symbol(
    executable: &Path,
    args: &[String],
    symbol: Option<Disclaim>,
) -> io::Result<Child> {
    fn checked(code: libc::c_int) -> io::Result<()> {
        if code == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(code))
        }
    }
    let binary = CString::new(executable.as_os_str().as_bytes())?;
    let argv = std::iter::once(Ok(binary.clone()))
        .chain(args.iter().map(|a| CString::new(a.as_bytes())))
        .collect::<Result<Vec<_>, _>>()?;
    let mut argv_ptrs: Vec<_> = argv
        .iter()
        .map(|a| a.as_ptr() as *mut libc::c_char)
        .collect();
    argv_ptrs.push(std::ptr::null_mut());
    // Snapshot environment into owned storage (never borrow libc's mutable environ).
    let env = std::env::vars_os()
        .map(|(k, v)| {
            let mut bytes = k.as_bytes().to_vec();
            bytes.push(b'=');
            bytes.extend_from_slice(v.as_bytes());
            CString::new(bytes)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut env_ptrs: Vec<_> = env
        .iter()
        .map(|s| s.as_ptr() as *mut libc::c_char)
        .collect();
    env_ptrs.push(std::ptr::null_mut());
    unsafe {
        let mut attr = std::mem::zeroed();
        checked(libc::posix_spawnattr_init(&mut attr))?;
        let result = (|| {
            checked(libc::posix_spawnattr_setflags(
                &mut attr,
                (libc::POSIX_SPAWN_SETPGROUP as libc::c_int | libc::POSIX_SPAWN_CLOEXEC_DEFAULT)
                    as i16,
            ))?;
            checked(libc::posix_spawnattr_setpgroup(&mut attr, 0))?;
            if !apply_disclaim(symbol, &mut attr) {
                // Reset attributes if a future implementation partially modifies them on error.
                libc::posix_spawnattr_destroy(&mut attr);
                checked(libc::posix_spawnattr_init(&mut attr))?;
                checked(libc::posix_spawnattr_setflags(
                    &mut attr,
                    (libc::POSIX_SPAWN_SETPGROUP as libc::c_int | libc::POSIX_SPAWN_CLOEXEC_DEFAULT)
                        as i16,
                ))?;
                checked(libc::posix_spawnattr_setpgroup(&mut attr, 0))?;
            }
            let mut pid = 0;
            checked(libc::posix_spawn(
                &mut pid,
                binary.as_ptr(),
                std::ptr::null(),
                &attr,
                argv_ptrs.as_ptr(),
                env_ptrs.as_ptr(),
            ))?;
            Ok(Child { pid, reaped: false })
        })();
        libc::posix_spawnattr_destroy(&mut attr);
        result
    }
}

/// Future App-bundle writer seam; not a suggestion to grant Chrome permissions.
pub fn app_management_hint(path: &Path, error: &io::Error) -> Option<&'static str> {
    let parts: Vec<_> = path.components().collect();
    if error.raw_os_error() == Some(libc::EPERM)
        && parts.len() >= 3
        && path.starts_with("/Applications")
        && parts[2]
            .as_os_str()
            .to_str()
            .is_some_and(|s| s.ends_with(".app"))
        && !parts
            .iter()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_AppBundles")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_plan_preserves_profile_port_and_disclaim() {
        let plan = LaunchPlan::managed(std::path::Path::new("/tmp/synthetic profile"), 19317, true);
        assert!(plan.disclaim);
        assert!(plan
            .args
            .contains(&"--user-data-dir=/tmp/synthetic profile".into()));
        assert!(plan.args.contains(&"--remote-debugging-port=19317".into()));
        assert!(plan
            .args
            .contains(&"--remote-debugging-address=127.0.0.1".into()));
        assert!(plan.args.contains(&"--headless".into()));
        let headed = LaunchPlan::managed(std::path::Path::new("/tmp/other"), 28491, false);
        assert!(!headed.args.iter().any(|a| a.starts_with("--headless")));
    }

    #[test]
    fn missing_symbol_and_setter_error_fall_back_without_failing_spawn() {
        assert!(!apply_disclaim(None, std::ptr::null_mut()));
        unsafe extern "C" fn denied(
            _: *mut libc::posix_spawnattr_t,
            value: libc::c_int,
        ) -> libc::c_int {
            assert_eq!(value, 1);
            libc::EINVAL
        }
        unsafe extern "C" fn accepted(
            _: *mut libc::posix_spawnattr_t,
            value: libc::c_int,
        ) -> libc::c_int {
            assert_eq!(value, 1);
            0
        }
        assert!(!apply_disclaim(Some(denied), std::ptr::null_mut()));
        assert!(apply_disclaim(Some(accepted), std::ptr::null_mut()));
    }

    #[test]
    fn fallback_spawns_real_child_and_owner_reaps_it() {
        let mut child =
            spawn_with_symbol(std::path::Path::new("/bin/sleep"), &["30".into()], None).unwrap();
        let pid = child.pid;
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
        child.stop().unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
    }

    #[test]
    fn drop_cleans_up_an_early_launch_failure() {
        let pid = {
            let child = spawn(std::path::Path::new("/bin/sleep"), &["30".into()]).unwrap();
            child.pid
        };
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }

    #[test]
    fn app_management_hint_only_matches_bundle_write_eperm() {
        let denied = std::io::Error::from_raw_os_error(libc::EPERM);
        let hint = app_management_hint(
            std::path::Path::new("/Applications/Synthetic.app/Contents/file"),
            &denied,
        )
        .unwrap();
        assert!(hint.contains("Privacy_AppBundles"));
        assert!(
            app_management_hint(std::path::Path::new("/tmp/Synthetic.app/file"), &denied).is_none()
        );
        assert!(app_management_hint(
            std::path::Path::new("/Applications/not-an-app/file"),
            &denied
        )
        .is_none());
        assert!(app_management_hint(
            std::path::Path::new("/Applications/../tmp/Synthetic.app/file"),
            &denied
        )
        .is_none());
        assert!(app_management_hint(
            std::path::Path::new("/Applications/Synthetic.app/file"),
            &std::io::Error::from_raw_os_error(libc::ENOENT)
        )
        .is_none());
    }
}
