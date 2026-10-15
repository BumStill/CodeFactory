// SPDX-License-Identifier: Apache-2.0
//! M89: process-scoped idle-sleep assertion, independent of GUI/lock state.
use sqlx::SqlitePool;

pub(super) async fn has_background_work(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objectives WHERE status IN ('active','waiting_system')")
        .fetch_one(pool).await?;
    Ok(count > 0)
}

#[derive(Default)]
pub(super) struct PowerAssertion {
    held: bool,
    #[cfg(target_os = "macos")]
    id: u32,
}

#[cfg(target_os = "macos")]
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOPMAssertionCreateWithName(kind: *const std::ffi::c_void, level: u32, name: *const std::ffi::c_void, id: *mut u32) -> i32;
    fn IOPMAssertionRelease(id: u32) -> i32;
}
#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(allocator: *const std::ffi::c_void, text: *const std::ffi::c_char, encoding: u32) -> *const std::ffi::c_void;
    fn CFRelease(value: *const std::ffi::c_void);
}
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn SetThreadExecutionState(flags: u32) -> u32;
}

impl PowerAssertion {
    // Called from a dedicated OS thread: Windows execution-state assertions
    // belong to the calling thread, not to a migratable async task.
    pub(super) fn update(&mut self, needed: bool) -> Result<(), String> {
        if needed == self.held { return Ok(()); }
        #[cfg(target_os = "macos")]
        unsafe {
            if needed {
                let kind = CFStringCreateWithCString(std::ptr::null(), c"PreventUserIdleSystemSleep".as_ptr(), 0x08000100);
                let reason = CFStringCreateWithCString(std::ptr::null(), c"CodeFactory background objectives".as_ptr(), 0x08000100);
                if kind.is_null() || reason.is_null() {
                    if !kind.is_null() { CFRelease(kind); }
                    if !reason.is_null() { CFRelease(reason); }
                    return Err("cannot allocate power assertion strings".into());
                }
                let result = IOPMAssertionCreateWithName(kind, 255, reason, &mut self.id);
                CFRelease(kind);
                CFRelease(reason);
                if result != 0 { return Err(format!("IOPMAssertionCreateWithName: {result}")); }
            } else {
                let result = IOPMAssertionRelease(self.id);
                if result != 0 { return Err(format!("IOPMAssertionRelease: {result}")); }
            }
        }
        #[cfg(windows)]
        unsafe {
            if SetThreadExecutionState(0x80000000 | if needed { 1 } else { 0 }) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
        }
        self.held = needed;
        Ok(())
    }
}
impl Drop for PowerAssertion {
    fn drop(&mut self) { let _ = self.update(false); }
}

pub(super) fn spawn(pool: SqlitePool) {
    std::thread::Builder::new().name("objective-power-assertion".into()).spawn(move || {
        let mut assertion = PowerAssertion::default();
        loop {
            match tauri::async_runtime::block_on(has_background_work(&pool)) {
                Ok(needed) => if let Err(error) = assertion.update(needed) {
                    tracing::warn!(%error, "objective power assertion failed");
                },
                Err(error) => tracing::warn!(%error, "cannot observe background objectives for power assertion"),
            }
            if pool.is_closed() { break; }
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    }).expect("start objective power assertion thread");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn m89_power_assertion_tracks_only_background_objectives() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1)
            .connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE objectives (status TEXT)").execute(&pool).await.unwrap();
        let mut assertion = PowerAssertion::default();
        for status in ["active", "waiting_system", "waiting_user", "completed", "cancelled", "failed"] {
            sqlx::query("DELETE FROM objectives").execute(&pool).await.unwrap();
            sqlx::query("INSERT INTO objectives VALUES (?)").bind(status).execute(&pool).await.unwrap();
            let needed = has_background_work(&pool).await.unwrap();
            assertion.update(needed).unwrap();
            assert_eq!(assertion.held, matches!(status, "active" | "waiting_system"));
        }
        sqlx::query("DELETE FROM objectives").execute(&pool).await.unwrap();
        assertion.update(has_background_work(&pool).await.unwrap()).unwrap();
        assert!(!assertion.held);
    }
}
