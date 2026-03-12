#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(exit_code) = mycodex_app_lib::try_run_embedded_host() {
        std::process::exit(exit_code);
    }
    mycodex_app_lib::run();
}
