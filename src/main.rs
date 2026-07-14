#![windows_subsystem = "windows"]

use std::env;
use std::fs;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread::{sleep, spawn};
use std::time::Duration;

use systray::Application;
use winapi::shared::minwindef::{BOOL, DWORD, MAX_PATH};
use winapi::um::handleapi::CloseHandle;
use winapi::um::processthreadsapi::OpenProcess;
use winapi::um::psapi::{EnumProcesses, GetModuleFileNameExW};
use winapi::um::winnt::{
    HANDLE, PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};

const CREATE_NO_WINDOW: u32 = 0x08000000;
const TRAY_ICO: &[u8] = include_bytes!("../res/tray.ico");

// Not always exposed by winapi; works with PROCESS_QUERY_LIMITED_INFORMATION.
#[link(name = "kernel32")]
extern "system" {
    fn QueryFullProcessImageNameW(
        hProcess: HANDLE,
        dwFlags: DWORD,
        lpExeName: *mut u16,
        lpdwSize: *mut DWORD,
    ) -> BOOL;
}

fn get_process_id_by_name(process_name: &str) -> Option<u32> {
    let mut process_ids = [0u32; 1024];
    let mut bytes_needed = 0;

    unsafe {
        if EnumProcesses(
            process_ids.as_mut_ptr(),
            std::mem::size_of_val(&process_ids) as u32,
            &mut bytes_needed,
        ) == 0
        {
            eprintln!("Failed to enumerate processes");
            return None;
        }
    }

    let process_count = (bytes_needed / std::mem::size_of::<DWORD>() as u32) as usize;

    for &process_id in &process_ids[..process_count] {
        if process_id == 0 {
            continue;
        }

        let mut process_handle = unsafe {
            OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, process_id)
        };
        let mut has_full_access = !process_handle.is_null();

        if process_handle.is_null() {
            process_handle =
                unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
            has_full_access = false;
        }

        if process_handle.is_null() {
            continue;
        }

        let mut process_name_wide = [0u16; MAX_PATH];
        let success = unsafe {
            if has_full_access {
                GetModuleFileNameExW(
                    process_handle,
                    ptr::null_mut(),
                    process_name_wide.as_mut_ptr(),
                    MAX_PATH as u32,
                ) > 0
            } else {
                let mut size = MAX_PATH as u32;
                QueryFullProcessImageNameW(
                    process_handle,
                    0,
                    process_name_wide.as_mut_ptr(),
                    &mut size,
                ) != 0
            }
        };

        if success {
            let process_name_str = String::from_utf16_lossy(&process_name_wide);
            let trimmed = process_name_str.trim_end_matches('\0');
            if trimmed.contains(process_name) {
                unsafe { CloseHandle(process_handle) };
                return Some(process_id);
            }
        }

        unsafe { CloseHandle(process_handle) };
    }

    None
}

fn kill_process_by_name(process_name: &str) {
    let result = Command::new("taskkill")
        .args(["/IM", process_name, "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    match result {
        Ok(output) => {
            if output.status.success() {
                println!("Stopped process: {}", process_name);
                return;
            }

            let error_msg = String::from_utf8_lossy(&output.stderr);
            if error_msg.contains("Access is denied") || error_msg.contains("ERROR:") {
                eprintln!(
                    "Failed to stop process: {} - Access denied. May need administrator privileges.",
                    process_name
                );
            } else {
                let stdout_msg = String::from_utf8_lossy(&output.stdout);
                if stdout_msg.contains("not found") || stdout_msg.contains("not running") {
                    println!("Process {} is not running (already stopped).", process_name);
                } else {
                    eprintln!(
                        "Failed to stop process: {}. Error: {}",
                        process_name, error_msg
                    );
                }
            }
        }
        Err(e) => {
            eprintln!("Failed to execute taskkill for {}: {}.", process_name, e);
        }
    }
}

fn kill_tracked_processes(processes_to_stop: &[&str]) {
    for process_name in processes_to_stop {
        kill_process_by_name(process_name);
    }
}

/// Load tray icon from embedded bytes. Not an ICON resource — Explorer would
/// otherwise pick it as the EXE shell icon.
fn apply_tray_icon(app: &Application) {
    let tray_path = env::temp_dir().join("AdobeProcessMonitor_tray.ico");
    if fs::write(&tray_path, TRAY_ICO).is_err() {
        eprintln!("Warning: Failed to write tray icon to temp");
        return;
    }

    if let Err(e) = app.set_icon_from_file(tray_path.to_string_lossy().as_ref()) {
        eprintln!("Warning: Failed to load tray icon: {}", e);
    }
    let _ = fs::remove_file(&tray_path);
}

fn main() {
    let should_exit = Arc::new(Mutex::new(false));
    let mut app = Application::new().unwrap();
    app.set_tooltip("Adobe Process Monitor").unwrap();

    let should_exit_clone = Arc::clone(&should_exit);
    app.add_menu_item("Exit", move |app: &mut Application| {
        *should_exit_clone.lock().unwrap() = true;
        app.quit();
        std::process::exit(0);
        #[allow(unreachable_code)]
        Ok::<(), std::io::Error>(())
    })
    .unwrap();

    apply_tray_icon(&app);

    let adobe_processes = [
        "Photoshop.exe",
        "Adobe Premiere Pro.exe",
        "Acrobat.exe",
        "Illustrator.exe",
    ];
    let processes_to_stop = [
        "AdobeIPCBroker.exe",
        "Creative.UWPRPCService.exe",
        "CCXProcess.exe",
        "CCLibrary.exe",
        "dynamiclinkmanager.exe",
        "acrotray.exe",
        "AdobeCollabSync.exe",
    ];

    let should_exit_clone_for_thread = Arc::clone(&should_exit);
    spawn(move || {
        println!("Stopping tracked Adobe processes on startup...");
        kill_tracked_processes(&processes_to_stop);

        loop {
            if *should_exit_clone_for_thread.lock().unwrap() {
                break;
            }

            let mut found_adobe_process = false;

            for process in &adobe_processes {
                if let Some(pid) = get_process_id_by_name(process) {
                    found_adobe_process = true;
                    println!("Adobe process detected: {} (PID: {})", process, pid);

                    println!("Waiting for Adobe process to close...");
                    while get_process_id_by_name(process).is_some() {
                        println!("Process {} is still running. Waiting...", process);
                        sleep(Duration::from_secs(5));
                    }

                    println!("Adobe process {} has closed.", process);
                    println!("Stopping related processes...");
                    kill_tracked_processes(&processes_to_stop);
                    break;
                }
            }

            if !found_adobe_process {
                sleep(Duration::from_secs(5));
            }
        }
    });

    app.wait_for_message().unwrap();
    println!("Exiting application...");
}
