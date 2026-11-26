// SPDX-License-Identifier: Apache-2.0
#[cfg(not(test))]
#[path = "agent/unattended_smoke.rs"]
mod driver;

/// Execute the network-hermetic, cross-process long-task contract before any
/// other CLI entrypoint or Tauri initialization. Parent and internal workers
/// are copies of this exact formal executable, never a substitute test EXE.
#[cfg(not(test))]
pub fn run() -> bool {
    let args = std::env::args().collect::<Vec<_>>();
    let Some(flag) = args.get(1).map(String::as_str) else {
        return false;
    };
    if !matches!(
        flag,
        "--unattended-long-task-smoke" | "--unattended-long-task-worker"
    ) {
        return false;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|error| {
            eprintln!("Unattended long-task smoke could not start: {error}");
            std::process::exit(1);
        });
    match flag {
        "--unattended-long-task-smoke" => {
            if args.len() != 3 {
                eprintln!("usage: CodeFactory --unattended-long-task-smoke <receipt.json>");
                std::process::exit(2);
            }
            let output = std::path::PathBuf::from(&args[2]);
            let outcome = runtime.block_on(driver::run_parent());
            let rendered = serde_json::to_string_pretty(&outcome.receipt).unwrap_or_default();
            if let Err(error) = std::fs::write(&output, rendered.as_bytes()) {
                eprintln!(
                    "Unattended smoke could not write {}: {error}",
                    output.display()
                );
                std::process::exit(1);
            }
            if let Some(error) = outcome.error {
                eprintln!("Unattended long-task smoke failed: {error}");
                std::process::exit(1);
            }
            println!("{rendered}");
            true
        }
        "--unattended-long-task-worker" => {
            if args.len() != 5 {
                eprintln!(
                    "usage: CodeFactory --unattended-long-task-worker <state-dir> <provider-url> <phase>"
                );
                std::process::exit(2);
            }
            let state_dir = std::path::PathBuf::from(&args[2]);
            let phase = args[4].parse::<u8>().unwrap_or_else(|_| {
                eprintln!("unattended worker phase must be 1, 2, or 3");
                std::process::exit(2);
            });
            let mut start_gate = String::new();
            if let Err(error) = std::io::stdin().read_line(&mut start_gate) {
                eprintln!("Unattended worker start gate failed: {error}");
                std::process::exit(1);
            }
            if start_gate != "start\n" {
                eprintln!("Unattended worker start gate was not released");
                std::process::exit(1);
            }
            if let Err(error) = runtime.block_on(driver::run_worker(&state_dir, &args[3], phase)) {
                eprintln!("Unattended long-task worker failed: {error}");
                std::process::exit(1);
            }
            true
        }
        _ => unreachable!(),
    }
}
