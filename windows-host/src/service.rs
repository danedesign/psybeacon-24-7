//! LocalSystem service supervisor for the interactive desktop worker.
//!
//! The service remains in Session 0 and launches the stream worker with a
//! LocalSystem token assigned to the active console session. That lets the
//! worker use DXGI and input APIs against the interactive/logon desktop while
//! the service itself owns boot-time lifetime and restarts.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_NOT_ALL_ASSIGNED, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Security::{
    AdjustTokenPrivileges, DuplicateTokenEx, LookupPrivilegeValueW, SecurityImpersonation,
    TokenPrimary, LUID_AND_ATTRIBUTES, SE_ASSIGNPRIMARYTOKEN_NAME, SE_DEBUG_NAME,
    SE_INCREASE_QUOTA_NAME, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ASSIGN_PRIMARY,
    TOKEN_DUPLICATE, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, OpenProcess, OpenProcessToken, WaitForSingleObject,
    CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    STARTUPINFOW,
};
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    ServiceType, SessionChangeReason,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_dispatcher;

const SERVICE_NAME: &str = "PsychBeaconHost";
const NO_SESSION: u32 = u32::MAX;

windows_service::define_windows_service!(ffi_service_main, service_main);

enum ServiceCommand {
    Stop,
    SessionChanged(SessionChangeReason, u32),
}

pub fn run_dispatcher() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn service_main(_arguments: Vec<OsString>) {
    if let Err(error) = run_service() {
        log::error!("Service stopped with an error: {error}");
    }
}

fn run_service() -> windows_service::Result<()> {
    let (command_tx, command_rx) = mpsc::channel();
    let handler_tx = command_tx.clone();
    let status_handle =
        service_control_handler::register(SERVICE_NAME, move |event| match event {
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = handler_tx.send(ServiceCommand::Stop);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::SessionChange(change) => {
                let _ = handler_tx.send(ServiceCommand::SessionChanged(
                    change.reason,
                    change.notification.session_id,
                ));
                ServiceControlHandlerResult::NoError
            }
            _ => ServiceControlHandlerResult::NotImplemented,
        })?;

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP
            | ServiceControlAccept::SHUTDOWN
            | ServiceControlAccept::SESSION_CHANGE,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    log::info!("PsychBeacon system service started");
    let mut worker: Option<DesktopWorker> = None;
    let mut retry_at = Instant::now();
    let mut stopping = false;

    while !stopping {
        match command_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(ServiceCommand::Stop) => stopping = true,
            Ok(ServiceCommand::SessionChanged(reason, session_id)) => {
                log::info!("Console session event: {reason:?} for session {session_id}");
            }
            Err(RecvTimeoutError::Disconnected) => stopping = true,
            Err(RecvTimeoutError::Timeout) => {}
        }

        let active_session = unsafe { WTSGetActiveConsoleSessionId() };
        if active_session == NO_SESSION {
            if let Some(worker) = worker.take() {
                stop_worker(worker);
            }
            continue;
        }

        if worker
            .as_ref()
            .is_some_and(|running| running.session_id != active_session)
        {
            if let Some(worker) = worker.take() {
                log::info!("Active console moved to session {active_session}; restarting its desktop worker");
                stop_worker(worker);
            }
            retry_at = Instant::now();
        }

        if worker.as_ref().is_some_and(DesktopWorker::has_exited) {
            log::warn!("Desktop worker exited; scheduling a restart");
            worker.take();
            retry_at = Instant::now() + Duration::from_secs(3);
        }

        if worker.is_none() && Instant::now() >= retry_at {
            match DesktopWorker::launch(active_session) {
                Ok(running) => {
                    log::info!("Started SYSTEM desktop worker in console session {active_session}");
                    worker = Some(running);
                }
                Err(error) => {
                    log::error!(
                        "Couldn't launch desktop worker in session {active_session}: {error}"
                    );
                    retry_at = Instant::now() + Duration::from_secs(5);
                }
            }
        }
    }

    if let Some(worker) = worker {
        stop_worker(worker);
    }

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;
    log::info!("PsychBeacon system service stopped");
    Ok(())
}

struct DesktopWorker {
    process: OwnedHandle,
    session_id: u32,
}

impl DesktopWorker {
    fn launch(session_id: u32) -> io::Result<Self> {
        let executable = std::env::current_exe()?;
        let directory = executable.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "service executable has no parent directory",
            )
        })?;

        // Keep the CreateProcessAsUser privileges enabled on the service
        // token, but borrow a SYSTEM token that already belongs to the target
        // session. Mutating TokenSessionId on a duplicate of the Session 0
        // service token is denied on some Windows configurations even when
        // SeTcbPrivilege is present.
        let _service_token = open_system_process_token()
            .map_err(|error| stage_error("opening/enabling the LocalSystem token", error))?;
        let primary_token = open_session_system_token(session_id)?;

        let mut command_line = OsString::from("\"");
        command_line.push(&executable);
        command_line.push("\" --worker");
        let mut command_line: Vec<u16> = command_line.encode_wide().chain(Some(0)).collect();
        let mut desktop: Vec<u16> = "winsta0\\Default".encode_utf16().chain(Some(0)).collect();
        let mut working_directory: Vec<u16> =
            directory.as_os_str().encode_wide().chain(Some(0)).collect();
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop.as_mut_ptr()),
            ..Default::default()
        };
        let mut process_info = PROCESS_INFORMATION::default();
        unsafe {
            CreateProcessAsUserW(
                Some(primary_token.0),
                PCWSTR::null(),
                Some(PWSTR(command_line.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_UNICODE_ENVIRONMENT,
                None,
                PCWSTR(working_directory.as_mut_ptr()),
                &startup,
                &mut process_info,
            )
        }
        .map_err(|error| {
            stage_error("creating the desktop worker process", windows_error(error))
        })?;

        let _thread = OwnedHandle(process_info.hThread);
        Ok(Self {
            process: OwnedHandle(process_info.hProcess),
            session_id,
        })
    }

    fn has_exited(&self) -> bool {
        unsafe { WaitForSingleObject(self.process.0, 0) == WAIT_OBJECT_0 }
    }
}

fn open_system_process_token() -> io::Result<OwnedHandle> {
    let mut token = HANDLE::default();
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_PRIVILEGES,
            &mut token,
        )
    }
    .map_err(|error| stage_error("opening the service process token", windows_error(error)))?;
    let token = OwnedHandle(token);
    for (name, display_name) in [
        (SE_ASSIGNPRIMARYTOKEN_NAME, "SeAssignPrimaryTokenPrivilege"),
        (SE_INCREASE_QUOTA_NAME, "SeIncreaseQuotaPrivilege"),
        (SE_DEBUG_NAME, "SeDebugPrivilege"),
    ] {
        let mut luid = windows::Win32::Foundation::LUID::default();
        unsafe { LookupPrivilegeValueW(PCWSTR::null(), name, &mut luid) }.map_err(|error| {
            stage_error(
                "looking up a required token privilege",
                windows_error(error),
            )
        })?;
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        unsafe { AdjustTokenPrivileges(token.0, false, Some(&privileges), 0, None, None) }
            .map_err(|error| {
                stage_error("enabling a required token privilege", windows_error(error))
            })?;
        let last_error = unsafe { GetLastError() };
        if last_error == ERROR_NOT_ALL_ASSIGNED {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("the LocalSystem service token does not contain {display_name}"),
            ));
        }
    }
    Ok(token)
}

fn open_session_system_token(session_id: u32) -> io::Result<OwnedHandle> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|error| stage_error("enumerating Windows processes", windows_error(error)))?;
    let snapshot = OwnedHandle(snapshot);
    let mut entry = PROCESSENTRY32W::default();
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    unsafe { Process32FirstW(snapshot.0, &mut entry) }
        .map_err(|error| stage_error("reading the Windows process list", windows_error(error)))?;

    loop {
        let executable = String::from_utf16_lossy(
            &entry.szExeFile[..entry
                .szExeFile
                .iter()
                .position(|unit| *unit == 0)
                .unwrap_or(entry.szExeFile.len())],
        );
        if executable.eq_ignore_ascii_case("winlogon.exe") {
            let mut process_session = 0;
            unsafe {
                windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(
                    entry.th32ProcessID,
                    &mut process_session,
                )
            }
            .map_err(|error| stage_error("checking the Winlogon session", windows_error(error)))?;
            if process_session == session_id {
                let process = unsafe {
                    OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION,
                        false,
                        entry.th32ProcessID,
                    )
                }
                .map_err(|error| {
                    stage_error(
                        "opening the active session's Winlogon process",
                        windows_error(error),
                    )
                })?;
                let process = OwnedHandle(process);
                let mut source_token = HANDLE::default();
                unsafe {
                    OpenProcessToken(
                        process.0,
                        TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY,
                        &mut source_token,
                    )
                }
                .map_err(|error| {
                    stage_error(
                        "opening the active session's SYSTEM token",
                        windows_error(error),
                    )
                })?;
                let source_token = OwnedHandle(source_token);
                let access = TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY;
                let mut primary_token = HANDLE::default();
                unsafe {
                    DuplicateTokenEx(
                        source_token.0,
                        access,
                        None,
                        SecurityImpersonation,
                        TokenPrimary,
                        &mut primary_token,
                    )
                }
                .map_err(|error| {
                    stage_error(
                        "duplicating the active session's SYSTEM token",
                        windows_error(error),
                    )
                })?;
                return Ok(OwnedHandle(primary_token));
            }
        }

        if unsafe { Process32NextW(snapshot.0, &mut entry) }.is_err() {
            break;
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("couldn't find winlogon.exe in active console session {session_id}"),
    ))
}

fn stop_worker(worker: DesktopWorker) {
    log::info!(
        "Requesting graceful worker shutdown in session {}",
        worker.session_id
    );
    if let Err(error) = crate::request_stop_and_exit() {
        log::warn!(
            "Worker did not acknowledge local shutdown: {error}; waiting for its process to exit"
        );
    }
    loop {
        let result = unsafe { WaitForSingleObject(worker.process.0, 10_000) };
        if result == WAIT_OBJECT_0 {
            break;
        }
        if result == WAIT_TIMEOUT {
            log::warn!(
                "Worker is still cleaning up; continuing to wait rather than force-killing it"
            );
        } else {
            log::error!(
                "WaitForSingleObject returned an unexpected result while stopping the worker"
            );
            break;
        }
    }
}

fn windows_error(error: windows::core::Error) -> io::Error {
    io::Error::other(error.to_string())
}

fn stage_error(stage: &str, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{stage}: {error}"))
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}
