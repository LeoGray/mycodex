#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod host_controller;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use std::ffi::OsString;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use host_controller::{
    get_host_status, issue_local_host_connection, read_host_logs, restart_host, start_host,
    stop_host, update_host_config, HostControllerState,
};

#[cfg(not(any(target_os = "android", target_os = "ios")))]
const HOST_RUN_MARKER: &str = "--mycodex-host-run";

#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn try_run_embedded_host() -> Option<i32> {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).and_then(|arg| arg.to_str()) != Some(HOST_RUN_MARKER) {
        return None;
    }

    let mut forwarded_args = vec![OsString::from("mycodex")];
    forwarded_args.extend(args.into_iter().skip(2));

    let result = tauri::async_runtime::block_on(mycodex::cli::run_with_args(forwarded_args));
    match result {
        Ok(()) => Some(0),
        Err(error) => {
            eprintln!("{error:?}");
            Some(1)
        }
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn try_run_embedded_host() -> Option<i32> {
    None
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    let builder = builder
        .manage(HostControllerState::default())
        .invoke_handler(tauri::generate_handler![
            get_host_status,
            update_host_config,
            start_host,
            stop_host,
            restart_host,
            read_host_logs,
            issue_local_host_connection
        ]);

    builder
        .run(tauri::generate_context!())
        .expect("failed to run MyCodex app shell");
}
