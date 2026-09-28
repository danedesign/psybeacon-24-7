#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Combined Windows host and viewer. The existing console host remains
//! available for diagnostics; this app starts it as a managed child so its
//! display sessions can shut down cleanly when hosting is stopped.

use std::io::{self, BufRead, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CONTROL_PORT: u16 = 43701;
const VIDEO_PORT: u16 = 43702;
const DISCOVER: &str = "PSYBEACON_DISCOVER_V1";
const HOST_REPLY: &str = "PSYBEACON_HOST_V1:";
const START: &str = "PSYBEACON_START_STREAM_V1:";
const MANIFEST: &str = "PSYBEACON_STREAM_INFO_V1:";
const STOP: &str = "PSYBEACON_STOP_STREAM_V1";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UiState {
    device_name: String,
    hosting: bool,
    host_stopping: bool,
    host_status: String,
    connecting: bool,
    connected: bool,
    connection_status: String,
    connected_host: String,
    frame_url: String,
    sidecar_url: String,
    display_label: String,
    recent_host: String,
    peers: Vec<Peer>,
    log_lines: Vec<String>,
}

#[derive(Clone, Serialize)]
struct Peer {
    name: String,
    address: String,
}

impl UiState {
    fn new(frame_url: String) -> Self {
        Self {
            device_name: hostname::get()
                .ok()
                .and_then(|name| name.into_string().ok())
                .unwrap_or_else(|| "This PC".into()),
            hosting: false,
            host_stopping: false,
            host_status: "Ready to host".into(),
            connecting: false,
            connected: false,
            connection_status: "Enter a PC address to connect".into(),
            connected_host: String::new(),
            frame_url,
            sidecar_url: String::new(),
            display_label: String::new(),
            recent_host: recent_host_path()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .unwrap_or_default()
                .trim()
                .to_string(),
            peers: Vec::new(),
            log_lines: Vec::new(),
        }
    }

    fn log(&mut self, line: impl Into<String>) {
        self.log_lines.push(line.into());
        if self.log_lines.len() > 25 {
            self.log_lines.remove(0);
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisplayInfo {
    index: u16,
    width: u32,
    height: u32,
    sidecar_port: u16,
}

enum Message {
    Action(serde_json::Value),
    Connected(ClientSession, String, DisplayInfo),
    ConnectFailed(String),
}

struct ClientSession {
    target: SocketAddr,
    stop: Arc<AtomicBool>,
    decoder: Child,
    udp_thread: Option<thread::JoinHandle<()>>,
    jpeg_thread: Option<thread::JoinHandle<()>>,
}

impl ClientSession {
    fn stop(mut self, frame: &Arc<Mutex<Option<Vec<u8>>>>) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
            let _ = socket.send_to(STOP.as_bytes(), self.target);
        }
        let _ = self.decoder.kill();
        let _ = self.decoder.wait();
        if let Some(worker) = self.udp_thread.take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.jpeg_thread.take() {
            let _ = worker.join();
        }
        *frame.lock().unwrap() = None;
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let frame = Arc::new(Mutex::new(None));
    let frame_server_stop = Arc::new(AtomicBool::new(false));
    let (frame_url, frame_server) = start_frame_server(frame.clone(), frame_server_stop.clone())?;
    let mut frame_server = Some(frame_server);
    let state = Arc::new(Mutex::new(UiState::new(frame_url)));
    let discovery_worker = start_discovery(state.clone(), frame_server_stop.clone());
    let mut discovery_worker = Some(discovery_worker);
    let (tx, rx) = mpsc::channel::<Message>();

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("PsychBeacon")
        .with_inner_size(LogicalSize::new(1180.0, 780.0))
        .with_min_inner_size(LogicalSize::new(900.0, 620.0))
        .build(&event_loop)?;
    let ipc_tx = tx.clone();
    let webview = WebViewBuilder::new()
        .with_html(include_str!("ui.html"))
        .with_ipc_handler(move |request| {
            if let Ok(value) = serde_json::from_str(request.body()) {
                let _ = ipc_tx.send(Message::Action(value));
            }
        })
        .build(&window)?;

    let mut host: Option<Child> = None;
    let mut client: Option<ClientSession> = None;
    let mut pending_stop: Option<Arc<AtomicBool>> = None;
    let mut pending_worker: Option<thread::JoinHandle<()>> = None;
    let mut next_render = Instant::now();

    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(100));

        if matches!(event, Event::WindowEvent { event: WindowEvent::CloseRequested, .. }) {
            if let Some(stop) = pending_stop.take() {
                stop.store(true, Ordering::SeqCst);
            }
            if let Some(worker) = pending_worker.take() {
                let _ = worker.join();
            }
            if let Some(session) = client.take() {
                session.stop(&frame);
            }
            if let Some(mut child) = host.take() {
                child.stdin.take(); // EOF asks the managed host to unwind.
                let _ = child.wait();
            }
            frame_server_stop.store(true, Ordering::SeqCst);
            if let Some(worker) = frame_server.take() {
                let _ = worker.join();
            }
            if let Some(worker) = discovery_worker.take() {
                let _ = worker.join();
            }
            *flow = ControlFlow::Exit;
            return;
        }

        while let Ok(message) = rx.try_recv() {
            match message {
                Message::Action(action) => match action.get("type").and_then(|v| v.as_str()) {
                    Some("startHost") if host.is_none() && !state.lock().unwrap().host_stopping => {
                        match start_host(state.clone()) {
                            Ok(child) => {
                                host = Some(child);
                                let mut s = state.lock().unwrap();
                                s.hosting = true;
                                s.host_stopping = false;
                                s.host_status = "Starting host…".into();
                            }
                            Err(error) => {
                                state.lock().unwrap().host_status = format!("Could not start host: {error}");
                            }
                        }
                    }
                    Some("stopHost") => {
                        if let Some(mut child) = host.take() {
                            child.stdin.take();
                            let state = state.clone();
                            state.lock().unwrap().host_stopping = true;
                            thread::spawn(move || {
                                let _ = child.wait();
                                let mut s = state.lock().unwrap();
                                s.host_status = "Ready to host".into();
                                s.host_stopping = false;
                            });
                        }
                        state.lock().unwrap().hosting = false;
                    }
                    Some("connect") if client.is_none() && !state.lock().unwrap().connecting => {
                        let address = action.get("address").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                        if address.is_empty() {
                            state.lock().unwrap().connection_status = "Enter a hostname or IP address".into();
                        } else {
                            let mut s = state.lock().unwrap();
                            s.connecting = true;
                            s.connection_status = format!("Connecting to {address}…");
                            drop(s);
                            let tx = tx.clone();
                            let frame = frame.clone();
                            let stop = Arc::new(AtomicBool::new(false));
                            pending_stop = Some(stop.clone());
                            pending_worker = Some(thread::spawn(move || match connect_client(&address, frame, stop) {
                                Ok((session, info)) => { let _ = tx.send(Message::Connected(session, address, info)); }
                                Err(error) => { let _ = tx.send(Message::ConnectFailed(error.to_string())); }
                            }));
                        }
                    }
                    Some("disconnect") => {
                        if let Some(session) = client.take() {
                            session.stop(&frame);
                        }
                        let mut s = state.lock().unwrap();
                        s.connected = false;
                        s.connecting = false;
                        s.connection_status = "Disconnected".into();
                        s.sidecar_url.clear();
                    }
                    _ => {}
                },
                Message::Connected(session, address, info) => {
                    pending_stop = None;
                    if let Some(worker) = pending_worker.take() { let _ = worker.join(); }
                    client = Some(session);
                    let mut s = state.lock().unwrap();
                    s.connecting = false;
                    s.connected = true;
                    s.connected_host = address.clone();
                    s.recent_host = address.clone();
                    if let Some(path) = recent_host_path() {
                        if let Some(folder) = path.parent() {
                            let _ = std::fs::create_dir_all(folder);
                        }
                        let _ = std::fs::write(path, &address);
                    }
                    s.connection_status = "Live session".into();
                    s.display_label = format!("Display {} · {} × {}", info.index + 1, info.width, info.height);
                    s.sidecar_url = format!("ws://{}:{}", address, info.sidecar_port);
                }
                Message::ConnectFailed(error) => {
                    pending_stop = None;
                    if let Some(worker) = pending_worker.take() { let _ = worker.join(); }
                    let mut s = state.lock().unwrap();
                    s.connecting = false;
                    s.connection_status = error;
                }
            }
        }

        if let Some(child) = host.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                host = None;
                let mut s = state.lock().unwrap();
                s.hosting = false;
                s.host_stopping = false;
                s.host_status = format!("Host stopped ({status})");
            }
        }

        if let Some(session) = client.as_mut() {
            if matches!(session.decoder.try_wait(), Ok(Some(_))) {
                if let Some(session) = client.take() {
                    session.stop(&frame);
                }
                let mut s = state.lock().unwrap();
                s.connected = false;
                s.connection_status = "Video decoder stopped. Check FFmpeg and the host stream.".into();
                s.sidecar_url.clear();
            }
        }

        if Instant::now() >= next_render {
            if let Ok(json) = serde_json::to_string(&*state.lock().unwrap()) {
                let _ = webview.evaluate_script(&format!("window.renderState({json})"));
            }
            next_render = Instant::now() + Duration::from_millis(300);
        }
    });
}

fn recent_host_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("PsychBeacon").join("recent-host.txt"))
}

fn start_discovery(state: Arc<Mutex<UiState>>, stop: Arc<AtomicBool>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
                let _ = socket.set_broadcast(true);
                let _ = socket.set_read_timeout(Some(Duration::from_millis(200)));
                let _ = socket.send_to(DISCOVER.as_bytes(), ("255.255.255.255", CONTROL_PORT));
                let until = Instant::now() + Duration::from_millis(900);
                let mut peers: Vec<Peer> = Vec::new();
                let mut buf = [0u8; 512];
                while Instant::now() < until && !stop.load(Ordering::Relaxed) {
                    if let Ok((size, source)) = socket.recv_from(&mut buf) {
                        if let Ok(message) = std::str::from_utf8(&buf[..size]) {
                            if let Some(name) = message.strip_prefix(HOST_REPLY) {
                                let address = source.ip().to_string();
                                if !peers.iter().any(|peer| peer.address == address) {
                                    peers.push(Peer { name: name.trim().to_string(), address });
                                }
                            }
                        }
                    }
                }
                state.lock().unwrap().peers = peers;
            }
            for _ in 0..100 {
                if stop.load(Ordering::Relaxed) { break; }
                thread::sleep(Duration::from_millis(100));
            }
        }
    })
}

fn start_host(state: Arc<Mutex<UiState>>) -> io::Result<Child> {
    let executable = std::env::current_exe()?.with_file_name("psybeacon-host.exe");
    if !executable.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "psybeacon-host.exe is missing; build both binaries with cargo build --bins"));
    }
    let mut child = Command::new(executable)
        .arg("--managed")
        .env("RUST_LOG", "info")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(stderr) = child.stderr.take() {
        thread::spawn(move || {
            for line in io::BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut s = state.lock().unwrap();
                if line.contains("Listening for discovery") {
                    s.host_status = "Online · waiting for a connection".into();
                } else if line.contains("Multi-stream: request") {
                    s.host_status = "Preparing remote display…".into();
                } else if line.contains("Multi-stream: display") && line.contains("up:") {
                    s.host_status = "Streaming to a client".into();
                } else if line.contains("Multi-stream: session") && line.contains("ended") {
                    s.host_status = "Online · waiting for a connection".into();
                }
                s.log(line);
            }
        });
    }
    Ok(child)
}

fn connect_client(address: &str, frame: Arc<Mutex<Option<Vec<u8>>>>, stop: Arc<AtomicBool>) -> io::Result<(ClientSession, DisplayInfo)> {
    let target = (address, CONTROL_PORT)
        .to_socket_addrs()?
        .find(|address| address.is_ipv4())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "No IPv4 address found for that PC"))?;
    let video = UdpSocket::bind(("0.0.0.0", VIDEO_PORT))?;
    video.set_read_timeout(Some(Duration::from_millis(250)))?;
    let control = UdpSocket::bind("0.0.0.0:0")?;
    control.set_read_timeout(Some(Duration::from_millis(250)))?;

    let mut decoder = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-fflags", "nobuffer", "-flags", "low_delay", "-f", "h264", "-i", "pipe:0", "-an", "-pix_fmt", "yuvj420p", "-f", "image2pipe", "-vcodec", "mjpeg", "-q:v", "5", "pipe:1"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = decoder.stdin.take().unwrap();
    let udp_stop = stop.clone();
    let udp_thread = thread::spawn(move || {
        let mut packet = [0u8; 65536];
        while !udp_stop.load(Ordering::Relaxed) {
            match video.recv_from(&mut packet) {
                Ok((size, _)) => if input.write_all(&packet[..size]).is_err() { break; },
                Err(error) if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => {}
                Err(_) => break,
            }
        }
    });
    let mut output = decoder.stdout.take().unwrap();
    let jpeg_stop = stop.clone();
    let jpeg_thread = thread::spawn(move || {
        let mut chunk = [0u8; 64 * 1024];
        let mut pending = Vec::new();
        while !jpeg_stop.load(Ordering::Relaxed) {
            match output.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(size) => {
                    pending.extend_from_slice(&chunk[..size]);
                    while let Some(end) = pending.windows(2).position(|pair| pair == [0xff, 0xd9]) {
                        let end = end + 2;
                        if let Some(start) = pending[..end].windows(2).position(|pair| pair == [0xff, 0xd8]) {
                            *frame.lock().unwrap() = Some(pending[start..end].to_vec());
                        }
                        pending.drain(..end);
                    }
                    if pending.len() > 16 * 1024 * 1024 { pending.clear(); }
                }
            }
        }
    });

    let cleanup = |mut decoder: Child, stop: Arc<AtomicBool>, udp_thread: thread::JoinHandle<()>, jpeg_thread: thread::JoinHandle<()>| {
        stop.store(true, Ordering::SeqCst);
        let _ = decoder.kill();
        let _ = decoder.wait();
        let _ = udp_thread.join();
        let _ = jpeg_thread.join();
    };
    if let Err(error) = control.send_to(format!("{START}{VIDEO_PORT}:1").as_bytes(), target) {
        cleanup(decoder, stop, udp_thread, jpeg_thread);
        return Err(error);
    }
    let mut response = [0u8; 4096];
    let deadline = Instant::now() + Duration::from_secs(14);
    let (size, source) = loop {
        if stop.load(Ordering::Relaxed) {
            cleanup(decoder, stop, udp_thread, jpeg_thread);
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Connection cancelled"));
        }
        match control.recv_from(&mut response) {
            Ok(reply) => break reply,
            Err(error) if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) && Instant::now() < deadline => {}
            Err(error) => {
                cleanup(decoder, stop, udp_thread, jpeg_thread);
                return Err(io::Error::new(error.kind(), "Host did not return a display manifest. Check its capture log and administrator permissions."));
            }
        }
    };
    if source.ip() != target.ip() {
        cleanup(decoder, stop, udp_thread, jpeg_thread);
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Reply came from another PC"));
    }
    let message = String::from_utf8_lossy(&response[..size]);
    let Some(json) = message.strip_prefix(MANIFEST) else {
        cleanup(decoder, stop, udp_thread, jpeg_thread);
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Host returned an unknown response"));
    };
    let info: Vec<DisplayInfo> = serde_json::from_str(json).map_err(io::Error::other)?;
    let Some(info) = info.into_iter().next() else {
        cleanup(decoder, stop, udp_thread, jpeg_thread);
        return Err(io::Error::new(io::ErrorKind::InvalidData, "Host returned no displays"));
    };
    Ok((ClientSession { target, stop, decoder, udp_thread: Some(udp_thread), jpeg_thread: Some(jpeg_thread) }, info))
}

fn start_frame_server(frame: Arc<Mutex<Option<Vec<u8>>>>, stop: Arc<AtomicBool>) -> io::Result<(String, thread::JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    listener.set_nonblocking(true)?;
    let worker = thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => serve_frame(&mut stream, &frame),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(25)),
                Err(_) => break,
            }
        }
    });
    Ok((format!("http://127.0.0.1:{port}/frame.jpg"), worker))
}

fn serve_frame(stream: &mut TcpStream, frame: &Arc<Mutex<Option<Vec<u8>>>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let mut request = [0u8; 512];
    let _ = stream.read(&mut request);
    let image = frame.lock().unwrap().clone();
    if let Some(image) = image {
        let header = format!("HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n", image.len());
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(&image);
    } else {
        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n");
    }
}
