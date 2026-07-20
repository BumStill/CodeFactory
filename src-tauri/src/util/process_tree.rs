// SPDX-License-Identifier: Apache-2.0
use std::process::{Output, Stdio};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

#[derive(Debug, PartialEq, Eq)]
pub enum ProcessOutputError {
    Timeout,
    Unavailable,
    Failed,
}

#[cfg(unix)]
fn isolate(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    command.as_std_mut().process_group(0);
}

#[cfg(not(unix))]
fn isolate(_command: &mut Command) {}

#[cfg(unix)]
async fn terminate(child: &mut Child) {
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
}

#[cfg(windows)]
async fn terminate(child: &mut Child) {
    use crate::util::no_window::NoWindow;

    if let Some(pid) = child.id() {
        let mut taskkill = Command::new("taskkill").no_window();
        taskkill.args(["/PID", &pid.to_string(), "/T", "/F"]);
        let _ = tokio::time::timeout(Duration::from_secs(1), taskkill.status()).await;
    }
    let _ = child.start_kill();
}

#[cfg(not(any(unix, windows)))]
async fn terminate(child: &mut Child) {
    let _ = child.start_kill();
}

async fn settle_reader(task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>) {
    if tokio::time::timeout(Duration::from_millis(500), &mut *task)
        .await
        .is_err()
    {
        task.abort();
        let _ = task.await;
    }
}

async fn collect_readers(
    stdout_task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    stderr_task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
) -> Result<(Vec<u8>, Vec<u8>), ProcessOutputError> {
    let mut stdout_result = None;
    let mut stderr_result = None;
    let settle_deadline = tokio::time::sleep(Duration::from_millis(500));
    tokio::pin!(settle_deadline);

    while stdout_result.is_none() || stderr_result.is_none() {
        tokio::select! {
            result = &mut *stdout_task, if stdout_result.is_none() => {
                stdout_result = Some(
                    result
                        .map_err(|_| ProcessOutputError::Failed)?
                        .map_err(|_| ProcessOutputError::Failed),
                );
            }
            result = &mut *stderr_task, if stderr_result.is_none() => {
                stderr_result = Some(
                    result
                        .map_err(|_| ProcessOutputError::Failed)?
                        .map_err(|_| ProcessOutputError::Failed),
                );
            }
            _ = &mut settle_deadline => break,
        }
    }

    let capture_incomplete = stdout_result.is_none() || stderr_result.is_none();
    if stdout_result.is_none() {
        stdout_task.abort();
    }
    if stderr_result.is_none() {
        stderr_task.abort();
    }

    let stdout = stdout_result.transpose()?.unwrap_or_default();
    let mut stderr = stderr_result.transpose()?.unwrap_or_default();
    if capture_incomplete {
        stderr.extend_from_slice(
            b"[output capture stopped because a background process retained the pipe]\n",
        );
    }
    Ok((stdout, stderr))
}

async fn terminate_and_reap(
    child: &mut Child,
    stdout_task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
    stderr_task: &mut tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,
) {
    terminate(child).await;
    let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    settle_reader(stdout_task).await;
    settle_reader(stderr_task).await;
}

pub async fn output_with_timeout(
    mut command: Command,
    timeout_duration: Duration,
) -> Result<Output, ProcessOutputError> {
    command
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    isolate(&mut command);
    let mut child = match command.spawn() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ProcessOutputError::Unavailable)
        }
        Err(_) => return Err(ProcessOutputError::Failed),
        Ok(child) => child,
    };
    let mut stdout = child.stdout.take().ok_or(ProcessOutputError::Failed)?;
    let mut stderr = child.stderr.take().ok_or(ProcessOutputError::Failed)?;
    let mut stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let mut stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    });

    let status = match tokio::time::timeout(timeout_duration, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(_)) => {
            terminate_and_reap(&mut child, &mut stdout_task, &mut stderr_task).await;
            return Err(ProcessOutputError::Failed);
        }
        Err(_) => {
            terminate_and_reap(&mut child, &mut stdout_task, &mut stderr_task).await;
            return Err(ProcessOutputError::Timeout);
        }
    };

    let (stdout, stderr) = collect_readers(&mut stdout_task, &mut stderr_task).await?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}
