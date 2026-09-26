//! WebSocket "sidecar" channel for keyboard/mouse input forwarding — the
//! spec's parallel control channel alongside the video stream.
//!
//! WebSocket, not UDP (unlike the video path): input events need reliable,
//! in-order delivery — losing or reordering a keypress is a much worse
//! user experience than losing a video frame — and TCP-backed WebSocket
//! gives that for free without hand-rolling retransmission.
//!
//! Untested against the real windows-rs API shapes for `SendInput`/`INPUT`
//! at the time this comment was written — same "write it, then let
//! `cargo check` correct the exact field names/types" approach used for
//! every other windows-rs call in this repo.

use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Deserialize;
use tungstenite::Message;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
    VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;

/// Where on the *virtual desktop* (absolute pixel coordinates) the
/// captured display sits. Normalized `(x, y)` in `[0, 1]` from the client
/// are mapped into this rectangle, so a click at the client's `(0, 0)`
/// lands at this specific display's top-left corner, not the whole
/// virtual desktop's — matters as soon as the display isn't at position
/// (0, 0), which `DesktopDuplicator` already knows from
/// `DXGI_OUTPUT_DESC.DesktopCoordinates`.
#[derive(Clone, Copy)]
pub struct DisplayBounds {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "camelCase")]
enum InputEvent {
    MouseMove { x: f64, y: f64 },
    MouseDown { button: MouseButton },
    MouseUp { button: MouseButton },
    Scroll { delta_x: f64, delta_y: f64 },
    KeyDown { key_code: u16 },
    KeyUp { key_code: u16 },
}

#[derive(Deserialize, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Runs the sidecar WebSocket server on `port`, accepting one connection
/// at a time (matches the video path's v1 single-client scope — see
/// `main.rs`'s `run_stream_session`), until `shutdown` fires. Blocks the
/// calling thread; spawn it on its own, same as `run_stream_session`.
pub fn run_sidecar_server(port: u16, bounds: DisplayBounds, shutdown: &AtomicBool) -> io::Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    listener.set_nonblocking(true)?;
    log::info!("Sidecar: listening for input connections on ws://0.0.0.0:{port}");

    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, addr)) => {
                log::info!("Sidecar: connection from {addr}");
                handle_connection(stream, bounds, shutdown);
                log::info!("Sidecar: connection from {addr} ended");
            }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => {
                log::warn!("Sidecar: accept failed: {e}");
                continue;
            }
        }
    }
    Ok(())
}

fn handle_connection(stream: TcpStream, bounds: DisplayBounds, shutdown: &AtomicBool) {
    // Handshake first, on a blocking stream; only afterward do we want a
    // short read timeout, so the message loop can still notice `shutdown`
    // without a slow/absent client wedging this thread forever.
    let mut socket = match tungstenite::accept(stream) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Sidecar: WebSocket handshake failed: {e}");
            return;
        }
    };

    if let Err(e) = socket.get_ref().set_read_timeout(Some(Duration::from_millis(200))) {
        log::warn!("Sidecar: couldn't set a read timeout, shutdown may be delayed: {e}");
    }

    while !shutdown.load(Ordering::Relaxed) {
        match socket.read() {
            Ok(Message::Text(text)) => match serde_json::from_str::<InputEvent>(&text) {
                Ok(event) => inject(event, bounds),
                Err(e) => log::warn!("Sidecar: bad message, ignoring ({e}): {text}"),
            },
            Ok(Message::Close(_)) => {
                log::info!("Sidecar: client closed the connection");
                break;
            }
            Ok(_) => {} // binary/ping/pong — not used, ignored
            Err(tungstenite::Error::Io(ref e))
                if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) =>
            {
                continue;
            }
            Err(e) => {
                log::warn!("Sidecar: connection error, closing: {e}");
                break;
            }
        }
    }
}

fn inject(event: InputEvent, bounds: DisplayBounds) {
    log::trace!("Sidecar: {event:?}");
    match event {
        InputEvent::MouseMove { x, y } => {
            let screen_x = bounds.left + (x * bounds.width as f64) as i32;
            let screen_y = bounds.top + (y * bounds.height as f64) as i32;
            unsafe {
                let _ = SetCursorPos(screen_x, screen_y);
            }
        }
        InputEvent::MouseDown { button } => send_mouse_button(button, true),
        InputEvent::MouseUp { button } => send_mouse_button(button, false),
        InputEvent::Scroll { delta_x, delta_y } => send_scroll(delta_x, delta_y),
        InputEvent::KeyDown { key_code } => send_key(key_code, false),
        InputEvent::KeyUp { key_code } => send_key(key_code, true),
    }
}

fn send_mouse_input(mi: MOUSEINPUT) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_mouse_button(button: MouseButton, down: bool) {
    let dw_flags = match (button, down) {
        (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
        (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
        (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
        (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
        (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
        (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
    };
    send_mouse_input(MOUSEINPUT {
        dx: 0,
        dy: 0,
        mouseData: 0,
        dwFlags: dw_flags,
        time: 0,
        dwExtraInfo: 0,
    });
}

fn send_scroll(delta_x: f64, delta_y: f64) {
    // WHEEL_DELTA = 120 "per click" is Win32's own convention for this field.
    if delta_y != 0.0 {
        send_mouse_input(MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: (delta_y * 120.0) as i32 as u32,
            dwFlags: MOUSEEVENTF_WHEEL,
            time: 0,
            dwExtraInfo: 0,
        });
    }
    if delta_x != 0.0 {
        send_mouse_input(MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: (delta_x * 120.0) as i32 as u32,
            dwFlags: MOUSEEVENTF_HWHEEL,
            time: 0,
            dwExtraInfo: 0,
        });
    }
}

fn send_key(vk_code: u16, key_up: bool) {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk_code),
                wScan: 0,
                dwFlags: if key_up { KEYEVENTF_KEYUP } else { Default::default() },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
    }
}
