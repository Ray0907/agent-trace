use std::net::TcpStream;
use std::process::{Child, Command};
use std::sync::Mutex;

use tauri::Manager;

struct DaemonProcess(Mutex<Option<Child>>);

fn daemon_already_running() -> bool {
    TcpStream::connect("127.0.0.1:7842").is_ok()
}

fn spawn_daemon() -> Option<Child> {
    // In dev, the binary lives next to the workspace root.
    // In production it will be a bundled sidecar — for now resolve relative to cwd.
    let candidates = [
        // release build inside the workspace
        concat!(env!("CARGO_MANIFEST_DIR"), "/../target/release/agent-trace"),
        // debug build
        concat!(env!("CARGO_MANIFEST_DIR"), "/../target/debug/agent-trace"),
    ];

    for path in &candidates {
        if std::path::Path::new(path).exists() {
            match Command::new(path).arg("serve").spawn() {
                Ok(child) => {
                    // Give the daemon a moment to bind the port.
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    return Some(child);
                }
                Err(e) => eprintln!("agent-trace: failed to spawn daemon at {path}: {e}"),
            }
        }
    }

    eprintln!("agent-trace: daemon binary not found; run `cargo build --release` first");
    None
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let child = if daemon_already_running() {
                None // external daemon — don't manage its lifetime
            } else {
                spawn_daemon()
            };

            app.manage(DaemonProcess(Mutex::new(child)));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                // Kill the daemon we spawned when the last window closes.
                if window.app_handle().webview_windows().is_empty() {
                    if let Some(state) = window.app_handle().try_state::<DaemonProcess>() {
                        if let Ok(mut guard) = state.0.lock() {
                            if let Some(mut child) = guard.take() {
                                let _ = child.kill();
                            }
                        }
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
