//! Spawns FFmpeg as a subprocess and feeds it raw BGRA frames over stdin for
//! NVENC hardware encoding. This is the "pipe frames into FFmpeg targeting
//! NVENC" half of module 3 — capture.rs is the other half.
//!
//! Not zero-copy: frames arrive here already read back into system memory
//! (see capture.rs's module doc). True zero-copy would hand NVENC the D3D11
//! texture directly and skip this subprocess/pipe entirely.
//!
//! The original spec's `preset=ultrafast`/`tune=zerolatency` are x264 flag
//! *names* that don't exist for `h264_nvenc` — verified against this
//! machine's real `ffmpeg -h encoder=h264_nvenc` output rather than assumed.
//! NVENC's equivalents, used below: `-preset p1` (fastest), `-tune ull`
//! (ultra-low-latency), plus a distinct boolean `-zerolatency 1` (disables
//! frame-reordering delay) that has no x264 counterpart at all.
//!
//! Also discovered by testing an actual encode on this machine rather than
//! trusting `ffmpeg -version`: the FFmpeg build installed first (Gyan
//! 9.0.2) was compiled against NVENC SDK 13.1, but this machine's driver
//! (560.94) only exposes 12.2 — `h264_nvenc` failed outright with "Driver
//! does not support the required nvenc API version". Fixed by installing
//! an older build (BtbN 7.1.5) compiled against a compatible SDK version,
//! rather than upgrading the driver itself — this machine's driver is not
//! something to touch casually, see the module 3 incident notes in
//! capture.rs. If NVENC init fails here with a similar version-mismatch
//! message, that's almost certainly the same class of issue, not a bug in
//! this code — check `ffmpeg -h encoder=h264_nvenc` actually runs first.

use std::io::{self, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

pub struct NvencConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u64,
    /// Keyframe interval, in frames. Shorter recovers faster from packet
    /// loss (relevant once this is actually going out over UDP/SRT) at the
    /// cost of bandwidth; longer is more efficient on a reliable link.
    pub gop: u32,
}

impl NvencConfig {
    /// 1080p-appropriate defaults at the given fps — halve/double
    /// `bitrate_bps` by eye for other resolutions until this has an actual
    /// quality-tuning pass.
    pub fn for_1080p(fps: u32) -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps,
            bitrate_bps: 20_000_000,
            gop: fps * 2,
        }
    }
}

/// A running FFmpeg encode process. Write raw BGRA frames to it with
/// [`write_frame`](Self::write_frame) in capture order, then
/// [`finish`](Self::finish) to flush and wait for it to exit cleanly.
pub struct NvencEncoder {
    child: Child,
    stdin: ChildStdin,
}

impl NvencEncoder {
    /// `output` is any FFmpeg output target: a local file path (elementary
    /// H.264, for local verification) or a `udp://`/`srt://` URL (muxed as
    /// MPEG-TS, the standard container for both — chosen automatically from
    /// the URL scheme).
    pub fn spawn(config: &NvencConfig, output: &str) -> io::Result<Self> {
        let container = if output.starts_with("udp://") || output.starts_with("srt://") {
            "mpegts"
        } else {
            "h264"
        };

        let mut child = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-y",
                "-loglevel",
                "warning",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "bgra",
                "-s",
                &format!("{}x{}", config.width, config.height),
                "-r",
                &config.fps.to_string(),
                "-i",
                "pipe:0",
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p1",
                "-tune",
                "ull",
                "-zerolatency",
                "1",
                "-rc",
                "cbr",
                "-b:v",
                &config.bitrate_bps.to_string(),
                "-bf",
                "0",
                "-delay",
                "0",
                "-g",
                &config.gop.to_string(),
                "-f",
                container,
                output,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()?;

        let stdin = child
            .stdin
            .take()
            .expect("stdin was just configured as piped");

        Ok(Self { child, stdin })
    }

    /// `data` must be exactly `width * height * 4` bytes of tightly-packed
    /// BGRA8 — a [`capture::CapturedFrame`](crate::capture::CapturedFrame)'s
    /// `data` matches this directly.
    pub fn write_frame(&mut self, data: &[u8]) -> io::Result<()> {
        self.stdin.write_all(data)
    }

    /// Closes stdin (so FFmpeg knows the input ended and flushes/finalizes
    /// its output) and waits for it to exit. Dropping an `NvencEncoder`
    /// without calling this leaves stdin open and FFmpeg running, waiting
    /// for more frames that will never come.
    pub fn finish(mut self) -> io::Result<()> {
        // `self.stdin`'s Drop closes the pipe; scoping it explicitly here
        // (rather than relying on `self` going out of scope after `wait`)
        // is what actually signals EOF to FFmpeg before we wait on it.
        drop(self.stdin);
        let status = self.child.wait()?;
        if !status.success() {
            return Err(io::Error::other(format!("ffmpeg exited with {status}")));
        }
        Ok(())
    }
}
