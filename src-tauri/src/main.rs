// SPDX-License-Identifier: Apache-2.0
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if codefactory_lib::run_history_session_smoke_cli() {
        return;
    }
    if codefactory_lib::run_unattended_long_task_smoke_cli() {
        return;
    }
    if codefactory_lib::run_evolution_smoke_cli() {
        return;
    }
    if codefactory_lib::run_browser_session_smoke_cli() {
        return;
    }
    if codefactory_lib::run_browser_chrome_attach_smoke_cli() {
        return;
    }
    if codefactory_lib::run_headless_smoke_cli() {
        return;
    }
    codefactory_lib::run()
}
