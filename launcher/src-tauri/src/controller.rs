use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
use serde::Serialize;
use tauri::{Emitter, Manager};
use crate::environment;

#[derive(Default)]
struct ApiProcess(Mutex<Option<Child>>);

#[derive(Serialize)]
struct ApiStatus {
    running: bool,
    pid: Option<u32>,
    url: String,
}

fn api_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if cfg!(debug_assertions) {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let name = if cfg!(windows) { "workflow-api.exe" } else { "workflow-api" };
        for profile in ["debug", "release"] {
            let candidate = repository.join("api").join("target").join(profile).join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    let resource = app.path().resource_dir().map_err(|e| e.to_string())?;
    let name = if cfg!(windows) { "mdev-api.exe" } else { "mdev-api" };
    let path = resource.join("runtime").join(name);
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("API executable not found: {}", path.display()))
    }
}

fn inspect(process: &ApiProcess, app: &tauri::AppHandle) -> Result<ApiStatus, String> {
    let mut guard = process.0.lock().map_err(|e| e.to_string())?;
    if let Some(child) = guard.as_mut() {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            *guard = None;
        }
    }
    let root = api_path(app)?;
    let root = root.parent().and_then(|dir| dir.parent())
        .ok_or("Invalid API installation root")?;
    let config = environment::api_environment(root)?;

    Ok(ApiStatus {
        running: guard.is_some(),
        pid: guard.as_ref().map(|child| child.id()),
        url: config.url(),
    })
}

#[tauri::command]
fn api_status(
    app: tauri::AppHandle,
    state: tauri::State<'_, ApiProcess>,
) -> Result<ApiStatus, String> {
    inspect(&state, &app)
}

#[tauri::command]
fn api_start(app: tauri::AppHandle, state: tauri::State<'_, ApiProcess>) -> Result<(), String> {
    let mut guard = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(child) = guard.as_mut() {
        if child.try_wait().map_err(|e| e.to_string())?.is_none() { return Ok(()); }
        *guard = None;
    }
    let path = api_path(&app)?;
    let working_dir = if cfg!(debug_assertions) && path.file_name().and_then(|name| name.to_str()).map(|name| name.starts_with("workflow-api")).unwrap_or(false) {
        path.parent()
            .and_then(|dir| dir.parent())
            .and_then(|dir| dir.parent())
            .ok_or("Invalid development API path")?
    } else {
        path.parent().and_then(|dir| dir.parent()).ok_or("Invalid API path")?
    };
    let api_config = environment::api_environment(working_dir)?;
    let mut command = Command::new(&path);
    command
        .current_dir(working_dir)
        .env("WORKFLOW_API_HOST", &api_config.host)
        .env("WORKFLOW_API_PORT", api_config.port.to_string())
        .env("CLICOLOR_FORCE", "1")
        .env("FORCE_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    command
        .env("MDEV_CONTROLLER_MANAGED", "1")
        .stdin(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
        command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }

    let mut child = command.spawn().map_err(|e| e.to_string())?;
    for (stream, pipe) in [
        ("stdout", child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)),
        ("stderr", child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)),
    ] {
        if let Some(pipe) = pipe {
            let handle = app.clone();
            thread::spawn(move || {
                let mut reader = BufReader::new(pipe);
                let mut buffer = Vec::new();
                loop {
                    buffer.clear();
                    match reader.read_until(b'\n', &mut buffer) {
                        Ok(0) => break,
                        Ok(_) => {
                            while matches!(buffer.last(), Some(b'\n' | b'\r')) {
                                buffer.pop();
                            }
                            let message = String::from_utf8_lossy(&buffer).into_owned();
                            let _ = handle.emit("api-log", serde_json::json!({ "stream": stream, "message": message }));
                        }
                        Err(_) => break,
                    }
                }
            });
        }
    }
    *guard = Some(child);
    Ok(())
}

#[cfg(unix)]
fn send_shutdown_signal(pid: u32) -> Result<(), String> {
    let result = unsafe { libc::kill(pid as libc::pid_t, libc::SIGINT) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

fn stop_process(process: &ApiProcess) -> Result<(), String> {
    let mut guard = process.0.lock().map_err(|e| e.to_string())?;

    let Some(child) = guard.as_mut() else {
        return Ok(());
    };

    if child.try_wait().map_err(|e| e.to_string())?.is_some() {
        *guard = None;
        return Ok(());
    }

    let pid = child.id();

    #[cfg(unix)]
    send_shutdown_signal(pid)?;

    #[cfg(windows)]
    {
        use std::io::Write;
        let stdin = child.stdin.as_mut().ok_or("API shutdown channel is unavailable")?;
        stdin.write_all(b"shutdown\n").map_err(|e| e.to_string())?;
        stdin.flush().map_err(|e| e.to_string())?;
    }

    let deadline = Instant::now() + Duration::from_secs(30);

    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            *guard = None;
            return Ok(());
        }

        if Instant::now() >= deadline {
            return Err(format!(
                "API process {pid} did not complete graceful shutdown within 30 seconds"
            ));
        }

        thread::sleep(Duration::from_millis(100));
    }
}

#[tauri::command]
fn api_stop(state: tauri::State<'_, ApiProcess>) -> Result<(), String> {
    stop_process(&state)
}

#[tauri::command]
fn api_restart(
    app: tauri::AppHandle,
    state: tauri::State<'_, ApiProcess>,
) -> Result<(), String> {
    stop_process(&state)?;
    api_start(app.clone(), app.state::<ApiProcess>())
}

#[tauri::command]
fn api_open(app: tauri::AppHandle) -> Result<(), String> {
    let path = api_path(&app)?;
    let root = path.parent().and_then(|dir| dir.parent())
        .ok_or("Invalid API installation root")?;
    let url = environment::api_environment(root)?.url();
    #[cfg(target_os = "windows")]
    let mut command = { let mut c = Command::new("cmd"); c.args(["/C", "start", "", &url]); c };
    #[cfg(target_os = "macos")]
    let mut command = { let mut c = Command::new("open"); c.arg(&url); c };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut command = { let mut c = Command::new("xdg-open"); c.arg(&url); c };
    command.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(ApiProcess::default())
        .invoke_handler(tauri::generate_handler![api_status, api_start, api_stop, api_restart, api_open])
        .run(tauri::generate_context!())
        .expect("failed to start mdev controller");
}
