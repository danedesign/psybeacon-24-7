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
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Security::{
    AdjustTokenPrivileges, DuplicateTokenEx, LookupPrivilegeValueW, SecurityImpersonation,
    SetTokenInformation, TokenPrimary, TokenSessionId, LUID_AND_ATTRIBUTES,
    SE_ASSIGNPRIMARYTOKEN_NAME, SE_INCREASE_QUOTA_NAME, SE_PRIVILEGE_ENABLED, SE_TCB_NAME,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY, TOKEN_DUPLICATE,
    TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::RemoteDesktop::WTSGetActiveConsoleSessionId;
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken, WaitForSingleObject,
    CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
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

        let process_token = open_system_process_token()?;
        let access = TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ASSIGN_PRIMARY | TOKEN_ADJUST_SESSIONID;
        let mut primary_token = HANDLE::default();
        unsafe {
            DuplicateTokenEx(
                process_token.0,
                access,
                None,
                SecurityImpersonation,
                TokenPrimary,
                &mut primary_token,
            )
        }
        .map_err(windows_error)?;
        let primary_token = OwnedHandle(primary_token);

        unsafe {
            SetTokenInformation(
                primary_token.0,
                TokenSessionId,
                (&session_id as *const u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
        }
        .map_err(windows_error)?;

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
        .map_err(windows_error)?;

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
            TOKEN_QUERY
                | TOKEN_DUPLICATE
                | TOKEN_ASSIGN_PRIMARY
                | TOKEN_ADJUST_SESSIONID
                | TOKEN_ADJUST_PRIVILEGES,
            &mut token,
        )
    }
    .map_err(windows_error)?;
    let token = OwnedHandle(token);
    for name in [
        SE_TCB_NAME,
        SE_ASSIGNPRIMARYTOKEN_NAME,
        SE_INCREASE_QUOTA_NAME,
    ] {
        let mut luid = windows::Win32::Foundation::LUID::default();
        unsafe { LookupPrivilegeValueW(PCWSTR::null(), name, &mut luid) }.map_err(windows_error)?;
        let privileges = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        unsafe { AdjustTokenPrivileges(token.0, false, Some(&privileges), 0, None, None) }
            .map_err(windows_error)?;
    }
    Ok(token)
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
