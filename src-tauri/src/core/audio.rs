//! Desktop (WASAPI loopback) + microphone capture → mix → AAC.
//!
//! Two shared-mode WASAPI capture clients run on one dedicated thread:
//! - **Loopback** of the default render endpoint (`eRender`) — everything the
//!   user hears: the game, Discord, music. We drive it by polling
//!   (`GetNextPacketSize`/`GetBuffer`/`ReleaseBuffer`) on a fixed cadence —
//!   simpler than event-driven and fine for a recorder. (Event-driven loopback is
//!   in fact supported on Windows 10 1703+; we just don't need it here.)
//! - **Microphone** of the default capture endpoint (`eCapture`). Optional —
//!   if it can't be opened we keep going with desktop audio only.
//!
//! Both are mixed into one 48 kHz stereo track and AAC-encoded, ready for
//! [`crate::core::mux`] to interleave alongside the H.264 video.
//!
//! ## Sync (the hard part)
//! Each `GetBuffer` reports the QPC time of its first sample (`pu64QPCPosition`).
//! WGC video frames carry `SystemRelativeTime`, also QPC-derived. Both are
//! placed on one **absolute 48 kHz sample timeline** keyed by QPC, so:
//! - **drift** between the two audio device clocks is absorbed (samples land at
//!   their true time, not at a naive running count), and
//! - **loopback silence** (which delivers *no packets*, not zeros) becomes a
//!   gap on the timeline that we fill with silence.
//!
//! The QPC unit (raw ticks vs 100 ns — the docs and the wild disagree) is
//! **auto-detected at runtime** ([`detect_qpc_scale`]) by comparing the first
//! reported position against `QueryPerformanceCounter`/`Frequency`, then
//! everything is converted to 100 ns ticks — the same unit as the video clock
//! (`clock::TICKS_PER_SECOND`) — so audio and video share one timeline.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use rusty_ffmpeg::ffi;
use windows::core::{Interface, GUID, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, AudioSessionStateActive, EDataFlow, IAudioCaptureClient,
    IAudioClient, IAudioSessionControl2, IAudioSessionManager2, IMMDevice, IMMDeviceCollection,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    DEVICE_STATE_ACTIVE, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantClear, PropVariantToStringAlloc, PROPVARIANT,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

use crate::core::capture::ClipBuffer;
use crate::core::clock::TICKS_PER_SECOND;
use crate::core::encode::{av_err, EncodedPacket};
use crate::settings::{AudioAppSel, AudioConfig, AUTO_DEVICE, GAME_SOURCE_ID};

/// Mixed-track sample rate. 48 kHz is the WASAPI shared-mode engine default and
/// the standard for AAC, so the common case needs no rate conversion.
const MIX_RATE: i32 = 48_000;
/// Mixed track is always stereo (game audio is; mic is upmixed if mono).
const MIX_CHANNELS: i32 = 2;
/// AAC target bitrate (stereo music/voice mix). Generous, like the video path.
const AAC_BITRATE: i64 = 160_000;
/// Idle backoff between empty polls. The shared-mode engine period is ~10 ms, so
/// it can't surface new data faster than that — polling at the period (vs the old
/// 5 ms) halves idle wakeups with no added latency that matters for a recorder
/// (the ≥20 ms / engine-default capture buffers leave ample headroom).
const POLL_MS: u64 = 10;
/// How far behind the newest sample we let the mixer lag before emitting a
/// block, so both sources have arrived (their packets land at slightly
/// different wall-clock instants). 200 ms is inaudible and well within sync.
const LATENCY_SAMPLES: i64 = MIX_RATE as i64 / 5;
/// A jump larger than this (on the absolute timeline) between a source's
/// expected position and where we last wrote is treated as a real gap →
/// resync + fill silence, rather than smoothed over (which would desync).
const GAP_SAMPLES: i64 = MIX_RATE as i64 / 20; // 50 ms

// FFmpeg error sentinels (mirrors encode.rs; AVERROR(e) == -e on every target).
const AVERROR_EAGAIN: i32 = -(ffi::EAGAIN as i32);
const AVERROR_EOF: i32 =
    -((b'E' as i32) | ((b'O' as i32) << 8) | ((b'F' as i32) << 16) | ((b' ' as i32) << 24));
/// `AV_CODEC_FLAG_GLOBAL_HEADER` — emit AudioSpecificConfig in `extradata`
/// (the MP4 `esds`) instead of in-band, so stream-copy muxing is decodable.
const AV_CODEC_FLAG_GLOBAL_HEADER: i32 = 1 << 22;

/// Everything the muxer needs to add the AAC stream that isn't per-packet:
/// built once when the AAC encoder opens. Mirrors [`crate::core::mux::ClipMeta`].
#[derive(Debug, Clone)]
pub struct AudioMeta {
    pub sample_rate: u32,
    pub channels: u32,
    /// AudioSpecificConfig from the encoder (`AV_CODEC_FLAG_GLOBAL_HEADER`).
    pub extradata: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Microphone selection + device enumeration
// ---------------------------------------------------------------------------

/// Which microphone the recorder mixes in alongside desktop audio. Parsed from
/// the persisted `settings.mic_source` string.
#[derive(Clone, Debug)]
pub enum MicSource {
    /// No microphone — desktop audio only.
    Off,
    /// The system default capture endpoint.
    Auto,
    /// A specific WASAPI capture endpoint, by its stable device id.
    Device(String),
}

impl MicSource {
    /// Map the persisted setting string onto a choice. Unknown/empty → `Off`.
    pub fn from_setting(s: &str) -> MicSource {
        match s {
            "auto" => MicSource::Auto,
            "" | "off" => MicSource::Off,
            id => MicSource::Device(id.to_string()),
        }
    }
}

/// A selectable capture endpoint (microphone / line-in) for the recorder UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioInputDevice {
    /// Stable WASAPI endpoint id — round-tripped back as `settings.mic_source`.
    pub id: String,
    /// Human-friendly name (e.g. "Microphone (USB Audio Device)").
    pub name: String,
}

/// A selectable render endpoint (speakers / headphones) for the "PC Audio"
/// multi-select in `all_pc_audio` mode. Same shape as [`AudioInputDevice`] but a
/// distinct type so the frontend picker can't confuse capture vs render ids.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioOutputDevice {
    /// Stable WASAPI render-endpoint id — stored in `AudioDeviceSel::id`.
    pub id: String,
    /// Human-friendly name (e.g. "Speakers (Realtek(R) Audio)").
    pub name: String,
}

/// An app currently playing audio (an active WASAPI render session), for the
/// `specific_apps` live source list. Mirrors Medal's `AudioSessionInfo`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioSession {
    /// Owning process id.
    pub pid: u32,
    /// Executable name (e.g. "Discord.exe") — also the persisted source id.
    pub process_name: String,
    /// Session display name when the app sets one, else the process name.
    pub display_name: String,
    /// The app's icon as a `data:image/png;base64,...` URL (extracted from the
    /// exe), or `None` if it couldn't be read — the UI then shows a generic icon.
    pub icon: Option<String>,
}

/// `PKEY_Device_FriendlyName` {a45c254e-df1c-4efd-8020-67d146a850e0},14 —
/// defined inline so we don't pull the FunctionDiscovery feature for one const.
const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c_254e_df1c_4efd_8020_67d1_46a8_50e0),
    pid: 14,
};

/// Enumerate active audio **capture** endpoints for the "Microphone Source"
/// picker. Self-contained COM init so it can be called straight from a Tauri
/// command thread. Best-effort: skips endpoints whose name can't be read.
pub fn enumerate_inputs() -> Vec<AudioInputDevice> {
    enumerate_endpoints(eCapture, "Microphone")
        .into_iter()
        .map(|(id, name)| AudioInputDevice { id, name })
        .collect()
}

/// Enumerate active audio **render** endpoints for the "PC Audio" multi-select
/// (`all_pc_audio` mode). Mirrors [`enumerate_inputs`] with `eRender`.
pub fn enumerate_outputs() -> Vec<AudioOutputDevice> {
    enumerate_endpoints(eRender, "Speakers")
        .into_iter()
        .map(|(id, name)| AudioOutputDevice { id, name })
        .collect()
}

/// Enumerate active endpoints of one data flow as `(id, friendly_name)` pairs.
/// Self-contained COM init so it can be called straight from a Tauri command
/// thread. Best-effort: skips endpoints whose id can't be read.
fn enumerate_endpoints(flow: EDataFlow, fallback_name: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    unsafe {
        // S_OK / S_FALSE (already initialized) → we own a ref to release; only a
        // genuine error (e.g. RPC_E_CHANGED_MODE) means we must not uninit.
        let inited = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        if let Err(e) = collect_endpoints(flow, fallback_name, &mut out) {
            tracing::warn!("enumerate audio endpoints failed: {e}");
        }
        if inited {
            CoUninitialize();
        }
    }
    out
}

unsafe fn collect_endpoints(
    flow: EDataFlow,
    fallback_name: &str,
    out: &mut Vec<(String, String)>,
) -> windows::core::Result<()> {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let collection: IMMDeviceCollection =
        enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
    let count = collection.GetCount()?;
    for i in 0..count {
        let Ok(device) = collection.Item(i) else {
            continue;
        };
        let Ok(id_pw) = device.GetId() else {
            continue;
        };
        let id = id_pw.to_string().unwrap_or_default();
        CoTaskMemFree(Some(id_pw.0 as *const _));
        if id.is_empty() {
            continue;
        }
        let name = read_friendly_name(&device).unwrap_or_else(|| fallback_name.to_string());
        out.push((id, name));
    }
    Ok(())
}

/// Process names never offered as a `specific_apps` source: Windows audio
/// plumbing the user can't meaningfully capture, plus Hako itself. (Matches
/// Medal's `AudioSessionManager` blacklist; the game PID is handled separately
/// as the dedicated "Game Audio" source.)
const SESSION_BLACKLIST: &[&str] = &["svchost.exe", "audiodg.exe", "hako.exe"];

/// Enumerate apps **currently playing audio** on the default render endpoint —
/// the live "additional apps appear here when they play audio" list for
/// `specific_apps` mode. Each active session is reported once per process id
/// (deduped), with the executable name resolved via `sysinfo`.
///
/// Best-effort and self-contained (own COM init), like [`enumerate_inputs`]:
/// any session we can't inspect is skipped. Icons (Medal sends base64 PNGs) are
/// deferred — names ship first.
pub fn enumerate_active_sessions() -> Vec<AudioSession> {
    let mut out = Vec::new();
    unsafe {
        let inited = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        if let Err(e) = collect_active_sessions(&mut out) {
            tracing::warn!("enumerate active audio sessions failed: {e}");
        }
        if inited {
            CoUninitialize();
        }
    }
    out
}

unsafe fn collect_active_sessions(out: &mut Vec<AudioSession>) -> windows::core::Result<()> {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device: IMMDevice = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
    let sessions = manager.GetSessionEnumerator()?;
    let count = sessions.GetCount()?;

    // Resolve pids → exe names in one process scan. Refresh only the process
    // list with no per-process detail (names come from the base enumeration),
    // matching the cheap scan `valorant::service` does for game detection.
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );

    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for i in 0..count {
        let Ok(ctrl) = sessions.GetSession(i) else {
            continue;
        };
        // IAudioSessionControl → IAudioSessionControl2 for the process id.
        let Ok(ctrl2) = ctrl.cast::<IAudioSessionControl2>() else {
            continue;
        };
        // Only sessions actively rendering audio (Medal's filter).
        if !matches!(ctrl2.GetState(), Ok(s) if s == AudioSessionStateActive) {
            continue;
        }
        let pid = ctrl2.GetProcessId().unwrap_or(0);
        if pid == 0 || !seen.insert(pid) {
            continue; // skip the system mix session (pid 0) and dupes
        }
        let process_name = sys
            .process(sysinfo::Pid::from_u32(pid))
            .and_then(|p| p.name().to_str().map(|s| s.to_string()))
            .unwrap_or_default();
        if process_name.is_empty()
            || SESSION_BLACKLIST
                .iter()
                .any(|b| process_name.eq_ignore_ascii_case(b))
        {
            continue;
        }
        // Session display name is usually empty → fall back to the process name.
        let display_name = read_session_display_name(&ctrl2)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| process_name.clone());
        // Best-effort real app icon (cached by exe path so the 3 s UI poll
        // doesn't re-extract). Falls back to a generic icon in the UI on None.
        let icon = sys
            .process(sysinfo::Pid::from_u32(pid))
            .and_then(|p| p.exe().map(|e| e.to_path_buf()))
            .and_then(|exe| cached_exe_icon(&exe));
        out.push(AudioSession {
            pid,
            process_name,
            display_name,
            icon,
        });
    }
    Ok(())
}

/// Read an audio session's display name (`IAudioSessionControl::GetDisplayName`),
/// freeing the returned COM string. `None` if unset/unreadable.
unsafe fn read_session_display_name(ctrl: &IAudioSessionControl2) -> Option<String> {
    let pw = ctrl.GetDisplayName().ok()?;
    let s = pw.to_string().ok();
    CoTaskMemFree(Some(pw.0 as *const _));
    s
}

/// Read an endpoint's `PKEY_Device_FriendlyName` as a `String`.
unsafe fn read_friendly_name(device: &IMMDevice) -> Option<String> {
    let store: IPropertyStore = device.OpenPropertyStore(STGM_READ).ok()?;
    let mut pv: PROPVARIANT = store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME).ok()?;
    let name = PropVariantToStringAlloc(&pv).ok().and_then(|pw| {
        let s = pw.to_string().ok();
        CoTaskMemFree(Some(pw.0 as *const _));
        s
    });
    let _ = PropVariantClear(&mut pv);
    name.filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// App-icon extraction (audio-source list)
// ---------------------------------------------------------------------------

/// Cache of exe path → its icon `data:` URL (or `None` if it has none), so the
/// UI's 3 s active-sessions poll doesn't re-extract icons every tick. Icons
/// effectively never change for a given binary path.
static ICON_CACHE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Option<String>>>,
> = std::sync::OnceLock::new();

/// The exe's icon as a PNG `data:` URL, memoized by path. `None` if the file has
/// no icon or extraction failed.
fn cached_exe_icon(exe: &std::path::Path) -> Option<String> {
    let cache = ICON_CACHE.get_or_init(Default::default);
    if let Ok(map) = cache.lock() {
        if let Some(hit) = map.get(exe) {
            return hit.clone();
        }
    }
    let icon = unsafe { extract_exe_icon_png(exe) };
    if let Ok(mut map) = cache.lock() {
        map.insert(exe.to_path_buf(), icon.clone());
    }
    icon
}

/// Extract `exe`'s associated icon and encode it as a `data:image/png;base64,…`
/// URL. Best-effort: returns `None` on any failure. Uses `SHGetFileInfoW` to get
/// the `HICON`, then GDI (`GetIconInfo`/`GetDIBits`) to read its 32-bit pixels.
unsafe fn extract_exe_icon_png(exe: &std::path::Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
    use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;

    let wide: Vec<u16> = exe.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut shfi = SHFILEINFOW::default();
    let ok = SHGetFileInfoW(
        PCWSTR(wide.as_ptr()),
        FILE_FLAGS_AND_ATTRIBUTES(0),
        Some(&mut shfi),
        std::mem::size_of::<SHFILEINFOW>() as u32,
        SHGFI_ICON | SHGFI_LARGEICON,
    );
    if ok == 0 || shfi.hIcon.is_invalid() {
        return None;
    }
    let png = hicon_to_png(shfi.hIcon);
    let _ = DestroyIcon(shfi.hIcon);
    let bytes = png?;
    use base64::Engine;
    Some(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

/// Render an `HICON` to RGBA and encode it as PNG bytes. Reads the color bitmap
/// as a top-down 32-bit DIB; when the icon carries no per-pixel alpha, derives
/// transparency from its AND mask.
unsafe fn hicon_to_png(hicon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<Vec<u8>> {
    use std::ffi::c_void;
    use windows::Win32::Graphics::Gdi::{
        DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

    let mut ii = ICONINFO::default();
    GetIconInfo(hicon, &mut ii).ok()?;
    let del = |h: windows::Win32::Graphics::Gdi::HBITMAP| {
        if !h.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(h.0));
        }
    };

    let mut bmp = BITMAP::default();
    let got = GetObjectW(
        HGDIOBJ(ii.hbmColor.0),
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bmp as *mut _ as *mut c_void),
    );
    let (w, h) = (bmp.bmWidth, bmp.bmHeight);
    if got == 0 || w <= 0 || h <= 0 || w > 512 || h > 512 {
        del(ii.hbmColor);
        del(ii.hbmMask);
        return None;
    }

    // Top-down (negative height) 32-bit BGRA via GetDIBits.
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: 0, // BI_RGB
        ..Default::default()
    };
    let mut bi = BITMAPINFO {
        bmiHeader: header,
        ..Default::default()
    };
    let n = (w * h * 4) as usize;
    let mut buf = vec![0u8; n];
    let hdc = GetDC(None);
    let lines = GetDIBits(
        hdc,
        ii.hbmColor,
        0,
        h as u32,
        Some(buf.as_mut_ptr() as *mut c_void),
        &mut bi,
        DIB_RGB_COLORS,
    );

    // BGRA → RGBA; if the color bitmap has no alpha at all, fall back to the mask.
    let any_alpha = buf.chunks_exact(4).any(|px| px[3] != 0);
    let mask = if lines != 0 && !any_alpha {
        let mut mbuf = vec![0u8; n];
        let mut mbi = BITMAPINFO {
            bmiHeader: header,
            ..Default::default()
        };
        let ml = GetDIBits(
            hdc,
            ii.hbmMask,
            0,
            h as u32,
            Some(mbuf.as_mut_ptr() as *mut c_void),
            &mut mbi,
            DIB_RGB_COLORS,
        );
        (ml != 0).then_some(mbuf)
    } else {
        None
    };
    ReleaseDC(None, hdc);
    del(ii.hbmColor);
    del(ii.hbmMask);
    if lines == 0 {
        return None;
    }

    for (i, px) in buf.chunks_exact_mut(4).enumerate() {
        px.swap(0, 2); // B,G,R,A → R,G,B,A
        if !any_alpha {
            // AND-mask: a non-zero (white) pixel is transparent.
            let transparent = mask.as_ref().map(|m| m[i * 4] != 0).unwrap_or(false);
            px[3] = if transparent { 0 } else { 255 };
        }
    }

    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w as u32, h as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().ok()?;
        writer.write_image_data(&buf).ok()?;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Public handle
// ---------------------------------------------------------------------------

/// A running audio-capture session. Drop or [`stop`](Self::stop) to tear down.
pub struct AudioCapture {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioCapture {
    /// Start multi-source audio capture per `cfg`, pushing each output track's AAC
    /// packets into the matching [`ClipBuffer`] audio track and publishing its
    /// [`AudioMeta`] once the encoder opens. `game_pid` is the capture target's
    /// process id (for the `specific_apps` "Game Audio" source). Returns `None`
    /// if the thread couldn't be spawned (caller proceeds video-only).
    ///
    /// Never blocks the caller meaningfully: setup happens on the audio thread
    /// and failures are logged — audio is best-effort relative to the recorder.
    pub fn start(
        clip: Arc<ClipBuffer>,
        cfg: AudioConfig,
        game_pid: Option<u32>,
    ) -> Option<AudioCapture> {
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = stop.clone();
            std::thread::Builder::new()
                .name("hako-audio".into())
                .spawn(move || audio_thread(clip, stop, cfg, game_pid))
                .ok()?
        };
        Some(AudioCapture {
            stop,
            thread: Some(thread),
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// Input → output-track planning (the multi-track model)
// ---------------------------------------------------------------------------

/// How to open one capture **input** (a WASAPI source), derived from
/// [`AudioConfig`]. The plan's input order is deterministic from the config, so
/// [`TrackSpec`] indices stay valid even if an input fails to open (it just
/// contributes silence).
enum InputSpec {
    /// Loopback of a render endpoint: `None` = the default (Medal's `"Auto"`).
    Loopback { id: Option<String> },
    /// Microphone capture endpoint, optionally down-mixed to mono.
    Mic { source: MicSource, mono: bool },
    /// Per-process loopback of `pid` (and its child processes). `pid == 0` when
    /// the target app isn't running — opens as silence until the editor cares.
    Process { pid: u32, name: String },
}

/// One **output track** written to the clip: a named mix of input contributions
/// (each `(input_index, relative_gain)`), encoded to its own AAC stream.
struct TrackSpec {
    name: String,
    sources: Vec<(usize, f32)>,
}

/// The full capture plan: the inputs to open + the output tracks to write.
struct AudioPlan {
    inputs: Vec<InputSpec>,
    tracks: Vec<TrackSpec>,
}

impl AudioPlan {
    /// Track names in order — the layout [`ClipBuffer`] is built with so the
    /// muxer/session can declare the streams. Pure function of the config.
    fn track_names(&self) -> Vec<String> {
        self.tracks.iter().map(|t| t.name.clone()).collect()
    }
}

/// Resolve a `specific_apps` source id to a target process id: the capture
/// target for [`GAME_SOURCE_ID`], else the first running process whose name
/// matches `id`. `0` when nothing matches (the input opens as silence).
fn resolve_app_pid(id: &str, game_pid: Option<u32>) -> u32 {
    if id.eq_ignore_ascii_case(GAME_SOURCE_ID) {
        let pid = game_pid.unwrap_or(0);
        if pid == 0 {
            tracing::warn!(
                "Game Audio source has no target PID (game window not resolved); \
                 it will record as silence"
            );
        }
        return pid;
    }
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    // Every process whose executable name matches the source id. Electron/
    // Chromium apps (Discord, browsers, etc.) run *several* identically-named
    // processes — and the one that actually renders audio is a child utility
    // process (`--type=utility --utility-sub-type=audio.mojom.AudioService`),
    // NOT the main window process the user thinks of.
    let matching: Vec<&sysinfo::Process> = sys
        .processes()
        .values()
        .filter(|p| {
            p.name()
                .to_str()
                .map(|n| n.eq_ignore_ascii_case(id))
                .unwrap_or(false)
        })
        .collect();
    if matching.is_empty() {
        return 0;
    }
    let pids: std::collections::HashSet<sysinfo::Pid> =
        matching.iter().map(|p| p.pid()).collect();
    // Capture the *root* of the same-named group: the matching process whose
    // parent is not itself in the group. `PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_
    // PROCESS_TREE` then sweeps in every descendant (including the audio-service
    // utility), so we record the app's sound no matter which child emits it.
    // The old `.find()` returned an arbitrary match (HashMap order) — usually a
    // leaf (gpu/renderer/utility) whose subtree excludes the audio renderer, so
    // the track came out silent. See `process_loopback::open`.
    matching
        .iter()
        .find(|p| p.parent().map_or(true, |pp| !pids.contains(&pp)))
        .or_else(|| matching.first())
        .map(|p| p.pid().as_u32())
        .unwrap_or(0)
}

/// Build the input→output-track plan from the persisted [`AudioConfig`].
///
/// Mirrors Medal's `RecordingSession.UpdateAudioConfig*`: every enabled source
/// becomes an input; **track 0 is always the master "All Audio" mix** (so clip
/// playback and back-compat are trivial); with `separate_tracks`, named stems
/// follow — one "All PC Audio" + "Microphone" for `all_pc_audio`, or one per app
/// + "Microphone" for `specific_apps`. Stems are raw (gain 1.0) so the editor
/// can re-mix/mute them on export; only the master applies the configured
/// volumes. Falls back to all-PC loopback when `specific_apps` is unsupported.
fn plan(cfg: &AudioConfig, game_pid: Option<u32>) -> AudioPlan {
    let master_gain = cfg.master_volume.min(100) as f32 / 100.0;
    let mut inputs: Vec<InputSpec> = Vec::new();
    // (input_index, master-mix gain, stem name) for each non-mic source.
    let mut media: Vec<(usize, f32, String)> = Vec::new();

    let specific = cfg.mode == "specific_apps" && is_process_loopback_supported();
    if cfg.mode == "specific_apps" && !specific {
        tracing::info!(
            "specific-apps audio unsupported on this Windows build (<20348); \
             falling back to all-PC loopback"
        );
    }

    if specific {
        // The UI shows "Game Audio" enabled by default — it's the headline source
        // and renders checked from a fallback even when no "game" entry has ever
        // been persisted (see `recording-audio.tsx`). Mirror that here: if there's
        // no explicit game entry, synthesize an enabled one so a config that never
        // toggled Game Audio still captures it. An explicit disabled "game" entry
        // (user turned it off) is respected.
        let mut apps: Vec<AudioAppSel> = cfg.apps.clone();
        if !apps.iter().any(|a| a.id.eq_ignore_ascii_case(GAME_SOURCE_ID)) {
            apps.insert(
                0,
                AudioAppSel {
                    id: GAME_SOURCE_ID.into(),
                    name: "Game Audio".into(),
                    enabled: true,
                    volume: 100,
                },
            );
        }
        for app in apps.iter().filter(|a| a.enabled) {
            let pid = resolve_app_pid(&app.id, game_pid);
            let idx = inputs.len();
            inputs.push(InputSpec::Process {
                pid,
                name: app.name.clone(),
            });
            let gain = (app.volume.min(100) as f32 / 100.0) * master_gain;
            media.push((idx, gain, app.name.clone()));
        }
    } else {
        // all_pc_audio (or the specific-apps fallback): one loopback per enabled
        // render endpoint, "auto" → the default endpoint.
        for dev in cfg.pc_audio.iter().filter(|d| d.enabled) {
            let id = (!dev.id.eq_ignore_ascii_case(AUTO_DEVICE)).then(|| dev.id.clone());
            let idx = inputs.len();
            inputs.push(InputSpec::Loopback { id });
            let gain = (dev.volume.min(100) as f32 / 100.0) * master_gain;
            media.push((idx, gain, dev.name.clone()));
        }
    }

    // Microphone (both modes), mixed last.
    let mic_source = MicSource::from_setting(&cfg.mic_source);
    let mic_idx = if cfg.mic_enabled && !matches!(mic_source, MicSource::Off) {
        let idx = inputs.len();
        inputs.push(InputSpec::Mic {
            source: mic_source,
            mono: cfg.mic_mono,
        });
        Some(idx)
    } else {
        None
    };
    let mic_gain = cfg.mic_volume.min(100) as f32 / 100.0;

    let mut tracks: Vec<TrackSpec> = Vec::new();
    if inputs.is_empty() {
        return AudioPlan { inputs, tracks };
    }

    // Track 0: the master mix (every media input at its volume + mic at its gain).
    let mut master = TrackSpec {
        name: "All Audio".into(),
        sources: media.iter().map(|(i, g, _)| (*i, *g)).collect(),
    };
    if let Some(mi) = mic_idx {
        master.sources.push((mi, mic_gain));
    }
    tracks.push(master);

    if cfg.separate_tracks {
        if specific {
            // One raw stem per app source.
            for (i, _, name) in &media {
                tracks.push(TrackSpec {
                    name: stem_name(name, "App Audio"),
                    sources: vec![(*i, 1.0)],
                });
            }
        } else {
            // One combined "All PC Audio" stem from every loopback input.
            tracks.push(TrackSpec {
                name: "All PC Audio".into(),
                sources: media.iter().map(|(i, _, _)| (*i, 1.0)).collect(),
            });
        }
        if let Some(mi) = mic_idx {
            tracks.push(TrackSpec {
                name: "Microphone".into(),
                sources: vec![(mi, 1.0)],
            });
        }
    }

    AudioPlan { inputs, tracks }
}

/// A stem track name, falling back to `default` when the source has no label.
fn stem_name(name: &str, default: &str) -> String {
    let n = name.trim();
    if n.is_empty() {
        default.to_string()
    } else {
        n.to_string()
    }
}

/// The names of the output audio tracks that [`AudioConfig`] will produce, in
/// order (track 0 = master "All Audio"). Empty when no source is enabled. Pure
/// function of the config — used by the capture path to size [`ClipBuffer`]
/// before the audio thread runs.
pub fn planned_track_names(cfg: &AudioConfig, game_pid: Option<u32>) -> Vec<String> {
    plan(cfg, game_pid).track_names()
}

// ---------------------------------------------------------------------------
// Thread entry
// ---------------------------------------------------------------------------

fn audio_thread(clip: Arc<ClipBuffer>, stop: Arc<AtomicBool>, cfg: AudioConfig, game_pid: Option<u32>) {
    // Keep audio glitch-free even while the game saturates the CPU.
    crate::core::boost_current_thread_priority("audio");
    // Exempt from any process-level EcoQoS set while hidden to tray, so WASAPI
    // draining + mixing + AAC encode is never parked on an efficiency core.
    crate::core::protect_thread_high_qos("audio");
    // Audio runs on its own COM apartment (MTA), independent of the capture
    // thread's WinRT init.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    if let Err(e) = run_audio(&clip, &stop, &cfg, game_pid) {
        tracing::warn!("audio capture disabled: {e}");
    }
    unsafe {
        CoUninitialize();
    }
}

/// One output track at runtime: its mixer accumulator + AAC encoder. The audio
/// thread feeds every input that references this track into `mixer`, then drains
/// ready blocks through `encoder` into [`ClipBuffer`] audio track `index`.
struct OutputTrack {
    index: usize,
    mixer: TrackMixer,
    encoder: AacEncoder,
}

fn run_audio(
    clip: &Arc<ClipBuffer>,
    stop: &Arc<AtomicBool>,
    cfg: &AudioConfig,
    game_pid: Option<u32>,
) -> Result<(), String> {
    unsafe {
        let plan = plan(cfg, game_pid);
        if plan.inputs.is_empty() || plan.tracks.is_empty() {
            return Err("no audio sources enabled".into());
        }

        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("MMDeviceEnumerator: {e}"))?;

        // Open every planned input. Failures keep their slot (as `None`) so the
        // output tracks' input indices stay valid — a missing input is silence.
        let mut sources: Vec<Option<Source>> = Vec::with_capacity(plan.inputs.len());
        for spec in &plan.inputs {
            match open_input(spec, &enumerator) {
                Ok(s) => sources.push(Some(s)),
                Err(e) => {
                    tracing::warn!("audio input unavailable (continuing without): {e}");
                    sources.push(None);
                }
            }
        }
        if sources.iter().all(|s| s.is_none()) {
            return Err("no audio capture devices could be opened".into());
        }

        // Build the output tracks (one mixer + encoder each) and publish each
        // track's AAC metadata. Encoders are device-independent, so this always
        // succeeds once the first one does.
        let mut tracks: Vec<OutputTrack> = Vec::with_capacity(plan.tracks.len());
        for (idx, _spec) in plan.tracks.iter().enumerate() {
            let encoder = AacEncoder::new()?;
            let block = encoder.frame_size();
            clip.set_audio_track_meta(
                idx,
                AudioMeta {
                    sample_rate: MIX_RATE as u32,
                    channels: MIX_CHANNELS as u32,
                    extradata: encoder.extradata(),
                },
            );
            tracks.push(OutputTrack {
                index: idx,
                mixer: TrackMixer::new(block),
                encoder,
            });
        }

        // Invert the track→inputs map into input→(track, gain) routing so each
        // drained input fans out to every track that mixes it.
        let mut routing: Vec<Vec<(usize, f32)>> = vec![Vec::new(); plan.inputs.len()];
        for (t, spec) in plan.tracks.iter().enumerate() {
            for &(input_idx, gain) in &spec.sources {
                if let Some(r) = routing.get_mut(input_idx) {
                    r.push((t, gain));
                }
            }
        }

        for s in sources.iter().flatten() {
            s.start()?;
        }

        let qpc_freq = {
            let mut f = 0i64;
            QueryPerformanceFrequency(&mut f).ok();
            f.max(1)
        };

        let mut timeline = Timeline::new();
        let mut scratch = Vec::<f32>::new();
        let mut zero = Vec::<u8>::new();

        while !stop.load(Ordering::Acquire) {
            let mut got_any = false;
            for i in 0..sources.len() {
                let Some(src) = sources[i].as_mut() else {
                    continue;
                };
                let routes = &routing[i];
                got_any |= src.drain(&mut timeline, qpc_freq, &mut scratch, &mut zero, |at, s| {
                    for &(t, gain) in routes {
                        tracks[t].mixer.add_scaled(at, s, gain);
                    }
                });
            }
            if let Some(epoch) = timeline.epoch() {
                for track in tracks.iter_mut() {
                    for (samples, pts_ticks) in track.mixer.drain_ready(false, epoch) {
                        for p in track.encoder.encode_block(&samples, pts_ticks)? {
                            clip.push_audio(track.index, p);
                        }
                    }
                }
            }
            if !got_any {
                std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
            }
        }

        // Stop devices, flush each track's mixed tail, then flush its encoder.
        for s in sources.iter().flatten() {
            let _ = s.audio_client.Stop();
        }
        if let Some(epoch) = timeline.epoch() {
            for track in tracks.iter_mut() {
                for (samples, pts_ticks) in track.mixer.drain_ready(true, epoch) {
                    for p in track.encoder.encode_block(&samples, pts_ticks)? {
                        clip.push_audio(track.index, p);
                    }
                }
            }
        }
        for track in tracks.iter_mut() {
            for p in track.encoder.flush()? {
                clip.push_audio(track.index, p);
            }
        }
        Ok(())
    }
}

/// Open one planned input as a [`Source`]. Each variant is best-effort; the
/// caller turns an `Err` into a silent slot.
unsafe fn open_input(spec: &InputSpec, enumerator: &IMMDeviceEnumerator) -> Result<Source, String> {
    match spec {
        InputSpec::Loopback { id: None } => Source::open_loopback(enumerator),
        InputSpec::Loopback { id: Some(id) } => Source::open_loopback_by_id(enumerator, id),
        InputSpec::Mic { source, mono } => {
            let mut s = match source {
                MicSource::Auto => Source::open_mic(enumerator)?,
                MicSource::Device(id) => Source::open_mic_by_id(enumerator, id)?,
                MicSource::Off => return Err("microphone source is off".into()),
            };
            s.mono = *mono;
            Ok(s)
        }
        InputSpec::Process { pid, name } => Source::open_process_loopback(*pid, name),
    }
}

// ---------------------------------------------------------------------------
// One WASAPI capture source (loopback or mic)
// ---------------------------------------------------------------------------

/// Sample layout of a source's `GetBuffer` data, parsed from its mix format.
#[derive(Clone, Copy)]
struct SrcFormat {
    rate: i32,
    channels: u16,
    /// `AVSampleFormat` for swr input (packed: FLT / S16 / S32). Shared mode is
    /// almost always float, but we detect to be safe.
    av_fmt: i32,
    /// Bytes per audio frame (`nBlockAlign`) — to size the silence scratch.
    block_align: u16,
}

struct Source {
    audio_client: IAudioClient,
    capture: IAudioCaptureClient,
    fmt: SrcFormat,
    swr: *mut ffi::SwrContext,
    /// Next absolute 48 kHz sample index this source will write to the mix.
    next_idx: i64,
    started: bool,
    /// Down-mix this source to mono before mixing (mic `MonoMicAudio`).
    mono: bool,
    label: &'static str,
}

impl Source {
    unsafe fn open_loopback(enumerator: &IMMDeviceEnumerator) -> Result<Source, String> {
        let device: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("default render endpoint: {e}"))?;
        Source::activate(device, AUDCLNT_STREAMFLAGS_LOOPBACK, "desktop")
    }

    /// Open loopback of a specific render endpoint by its WASAPI device id (the
    /// "PC Audio" multi-select in `all_pc_audio` mode).
    unsafe fn open_loopback_by_id(
        enumerator: &IMMDeviceEnumerator,
        id: &str,
    ) -> Result<Source, String> {
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        let device: IMMDevice = enumerator
            .GetDevice(PCWSTR(wide.as_ptr()))
            .map_err(|e| format!("render endpoint by id: {e}"))?;
        Source::activate(device, AUDCLNT_STREAMFLAGS_LOOPBACK, "desktop")
    }

    unsafe fn open_mic(enumerator: &IMMDeviceEnumerator) -> Result<Source, String> {
        let device: IMMDevice = enumerator
            .GetDefaultAudioEndpoint(eCapture, eConsole)
            .map_err(|e| format!("default capture endpoint: {e}"))?;
        Source::activate(device, 0, "mic")
    }

    /// Open a specific capture endpoint by its WASAPI device id (the value the
    /// "Microphone Source" picker persists).
    unsafe fn open_mic_by_id(
        enumerator: &IMMDeviceEnumerator,
        id: &str,
    ) -> Result<Source, String> {
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        let device: IMMDevice = enumerator
            .GetDevice(PCWSTR(wide.as_ptr()))
            .map_err(|e| format!("capture endpoint by id: {e}"))?;
        Source::activate(device, 0, "mic")
    }

    /// Activate an `IAudioClient` in shared mode (optionally loopback), parse its
    /// mix format and build the resampler to the mixed 48 kHz stereo float track.
    unsafe fn activate(
        device: IMMDevice,
        stream_flags: u32,
        label: &'static str,
    ) -> Result<Source, String> {
        let audio_client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("Activate IAudioClient ({label}): {e}"))?;

        let wf = audio_client
            .GetMixFormat()
            .map_err(|e| format!("GetMixFormat ({label}): {e}"))?;
        if wf.is_null() {
            return Err(format!("GetMixFormat returned null ({label})"));
        }
        let fmt = parse_format(wf);

        // 0 buffer duration/periodicity → engine default. We poll rather than use
        // EVENTCALLBACK (simpler; event-driven loopback works on Win10 1703+ but
        // we don't need it).
        let init = audio_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            stream_flags,
            0,
            0,
            wf,
            None,
        );
        CoTaskMemFree(Some(wf as *const _));
        init.map_err(|e| format!("IAudioClient::Initialize ({label}): {e}"))?;

        let capture: IAudioCaptureClient = audio_client
            .GetService()
            .map_err(|e| format!("GetService(IAudioCaptureClient) ({label}): {e}"))?;

        let swr = build_resampler(&fmt)
            .map_err(|e| format!("resampler ({label}): {e}"))?;

        Ok(Source {
            audio_client,
            capture,
            fmt,
            swr,
            next_idx: 0,
            started: false,
            mono: false,
            label,
        })
    }

    /// Open **per-process loopback** of `pid` (and its child process tree) via the
    /// virtual `Process_Loopback` device + `ActivateAudioInterfaceAsync`. Used by
    /// `specific_apps` mode for Game Audio / Discord / browser, etc.
    ///
    /// Gated by the caller on [`is_process_loopback_supported`] (Windows build
    /// ≥ 20348). The virtual device has no `GetMixFormat`, so we feed a fixed
    /// 48 kHz stereo float `WAVEFORMATEX`; capture then drains exactly like a
    /// normal loopback source.
    unsafe fn open_process_loopback(pid: u32, name: &str) -> Result<Source, String> {
        if !is_process_loopback_supported() {
            return Err("process loopback needs Windows build ≥ 20348".into());
        }
        if pid == 0 {
            return Err(format!("target process not running ({name})"));
        }
        process_loopback::open(pid, name)
    }

    unsafe fn start(&self) -> Result<(), String> {
        self.audio_client
            .Start()
            .map_err(|e| format!("IAudioClient::Start ({}): {e}", self.label))
    }

    /// Drain all currently-available packets, handing each resampled stereo run
    /// to `sink(at_idx, samples)` so the caller can fan it into every output
    /// track's mixer. Returns whether any packet was processed (so the loop can
    /// sleep when all sources are idle). `timeline` is the shared QPC/epoch
    /// anchor so every track lands samples on one 48 kHz timeline.
    unsafe fn drain(
        &mut self,
        timeline: &mut Timeline,
        qpc_freq: i64,
        scratch: &mut Vec<f32>,
        zero: &mut Vec<u8>,
        mut sink: impl FnMut(i64, &[f32]),
    ) -> bool {
        let mut any = false;
        loop {
            let avail = match self.capture.GetNextPacketSize() {
                Ok(n) => n,
                Err(_) => break,
            };
            if avail == 0 {
                break;
            }

            let mut data: *mut u8 = ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            let mut qpc_pos: u64 = 0;
            if self
                .capture
                .GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    None,
                    Some(&mut qpc_pos),
                )
                .is_err()
            {
                break;
            }

            if frames > 0 {
                let tick = timeline.qpc_to_ticks(qpc_pos, qpc_freq);
                let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
                let discontinuity =
                    (flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32) != 0;

                // Resample this packet (or an equivalent run of silence) to
                // 48 kHz stereo interleaved float.
                if silent || data.is_null() {
                    let bytes = frames as usize * self.fmt.block_align as usize;
                    if zero.len() < bytes {
                        zero.resize(bytes, 0);
                    }
                    self.resample(zero.as_ptr(), frames as i32, scratch);
                } else {
                    self.resample(data, frames as i32, scratch);
                }
                // Optional mono fold (mic MonoMicAudio): average L/R into both.
                if self.mono {
                    for f in scratch.chunks_exact_mut(2) {
                        let m = (f[0] + f[1]) * 0.5;
                        f[0] = m;
                        f[1] = m;
                    }
                }
                let out_frames = (scratch.len() / 2) as i64;

                // Place on the absolute timeline by QPC; resync on a real gap or
                // a driver-flagged discontinuity, else keep contiguous (avoids
                // pops from sub-millisecond jitter). Each track's mixer trims
                // anything that lands before what it has already drained.
                let expected = timeline.tick_to_idx(tick);
                if !self.started
                    || discontinuity
                    || (expected - self.next_idx).abs() > GAP_SAMPLES
                {
                    self.next_idx = expected;
                }
                self.started = true;

                sink(self.next_idx, scratch);
                self.next_idx += out_frames;
                any = true;
            }

            if self.capture.ReleaseBuffer(frames).is_err() {
                break;
            }
        }
        any
    }

    /// swr_convert `in_frames` of source-format interleaved audio at `data` into
    /// `out` as 48 kHz stereo interleaved f32.
    unsafe fn resample(&self, data: *const u8, in_frames: i32, out: &mut Vec<f32>) {
        out.clear();
        if in_frames <= 0 {
            return;
        }
        // Upper bound on output frames (+slack for swr's internal buffering).
        let max_out =
            (in_frames as i64 * MIX_RATE as i64 / self.fmt.rate.max(1) as i64 + 1024) as i32;
        out.resize(max_out as usize * 2, 0.0);
        let out_planes: [*mut u8; 1] = [out.as_mut_ptr() as *mut u8];
        let in_planes: [*const u8; 1] = [data];
        let n = ffi::swr_convert(
            self.swr,
            out_planes.as_ptr(),
            max_out,
            in_planes.as_ptr(),
            in_frames,
        );
        if n < 0 {
            out.clear();
            return;
        }
        out.truncate(n as usize * 2);
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        unsafe {
            if !self.swr.is_null() {
                ffi::swr_free(&mut self.swr);
            }
        }
    }
}

/// Parse a `WAVEFORMATEX`(`EXTENSIBLE`) into the fields swr needs. Shared-mode
/// WASAPI is virtually always 32-bit float; PCM is handled defensively.
unsafe fn parse_format(wf: *const WAVEFORMATEX) -> SrcFormat {
    const WAVE_FORMAT_PCM: u16 = 1;
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
    // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {00000003-0000-0010-8000-00aa00389b71}.
    // Hardcoded so we don't depend on the KernelStreaming feature.
    const SUBTYPE_FLOAT_D1: u32 = 0x0000_0003;
    const SUBTYPE_PCM_D1: u32 = 0x0000_0001;

    let w = &*wf;
    let channels = w.nChannels;
    let rate = w.nSamplesPerSec as i32;
    let bits = w.wBitsPerSample;
    let block_align = w.nBlockAlign;

    let is_float = if w.wFormatTag == WAVE_FORMAT_EXTENSIBLE {
        let ext = &*(wf as *const WAVEFORMATEXTENSIBLE);
        match ext.SubFormat.data1 {
            SUBTYPE_FLOAT_D1 => true,
            SUBTYPE_PCM_D1 => false,
            _ => bits == 32, // unknown subtype: assume float if 32-bit
        }
    } else {
        w.wFormatTag == WAVE_FORMAT_IEEE_FLOAT
    };

    let av_fmt = if is_float {
        ffi::AV_SAMPLE_FMT_FLT
    } else if bits == 16 {
        ffi::AV_SAMPLE_FMT_S16
    } else {
        ffi::AV_SAMPLE_FMT_S32
    };

    SrcFormat {
        rate,
        channels,
        av_fmt,
        block_align,
    }
}

/// swr context: source (its rate/channels/format) → 48 kHz stereo interleaved
/// f32 (packed FLT, easy additive mixing).
unsafe fn build_resampler(fmt: &SrcFormat) -> Result<*mut ffi::SwrContext, String> {
    let mut out_layout: ffi::AVChannelLayout = std::mem::zeroed();
    let mut in_layout: ffi::AVChannelLayout = std::mem::zeroed();
    ffi::av_channel_layout_default(&mut out_layout, MIX_CHANNELS);
    ffi::av_channel_layout_default(&mut in_layout, fmt.channels.max(1) as i32);

    let mut swr: *mut ffi::SwrContext = ptr::null_mut();
    let r = ffi::swr_alloc_set_opts2(
        &mut swr,
        &out_layout,
        ffi::AV_SAMPLE_FMT_FLT,
        MIX_RATE,
        &in_layout,
        fmt.av_fmt,
        fmt.rate.max(1),
        0,
        ptr::null_mut(),
    );
    ffi::av_channel_layout_uninit(&mut out_layout);
    ffi::av_channel_layout_uninit(&mut in_layout);
    if r < 0 || swr.is_null() {
        return Err(format!("swr_alloc_set_opts2: {}", av_err(r)));
    }
    let r = ffi::swr_init(swr);
    if r < 0 {
        ffi::swr_free(&mut swr);
        return Err(format!("swr_init: {}", av_err(r)));
    }
    Ok(swr)
}

/// Whether Windows supports per-process loopback capture (the `specific_apps`
/// recording mode). Process loopback needs **build ≥ 20348** (Win11 / Server
/// 2022); on older builds the capture plan falls back to all-PC loopback, the
/// same way Medal's GAO check disables game-audio-only capture.
pub fn is_process_loopback_supported() -> bool {
    // On Windows `sysinfo`'s kernel version is the OS build number (e.g. 22631).
    sysinfo::System::kernel_version()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .map(|build| build >= 20348)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Per-process loopback (specific_apps) via ActivateAudioInterfaceAsync
// ---------------------------------------------------------------------------

/// Per-process loopback capture (Windows ≥ 20348). Activating the virtual
/// `Process_Loopback` device is **asynchronous** — it requires a COM completion
/// handler and a blocking wait — so it's isolated here; a failure can't touch
/// the all-PC loopback path.
mod process_loopback {
    use super::{build_resampler, parse_format, Source, MIX_CHANNELS, MIX_RATE};
    use windows::core::{implement, Interface, IUnknown, HRESULT, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Media::Audio::{
        ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
        IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
        IAudioCaptureClient, IAudioClient, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
        AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
        AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
        PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
        WAVEFORMATEX,
    };
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::BLOB;
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
    use windows::Win32::System::Variant::VT_BLOB;

    /// `WAVE_FORMAT_IEEE_FLOAT` (the virtual device has no `GetMixFormat`, so we
    /// hand it an explicit 48 kHz / 2ch / 32-bit float format).
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    /// Max wait for the async activation to complete (ms). It's effectively
    /// immediate; this is just a safety bound so a stuck activation can't hang
    /// the audio thread's startup.
    const ACTIVATE_TIMEOUT_MS: u32 = 2000;

    /// COM completion handler for `ActivateAudioInterfaceAsync`: signals the
    /// Win32 event so [`open`] can unblock and fetch the activation result.
    #[implement(IActivateAudioInterfaceCompletionHandler)]
    struct Handler {
        event: HANDLE,
    }

    impl IActivateAudioInterfaceCompletionHandler_Impl for Handler_Impl {
        fn ActivateCompleted(
            &self,
            _op: windows::core::Ref<'_, IActivateAudioInterfaceAsyncOperation>,
        ) -> windows::core::Result<()> {
            unsafe {
                let _ = SetEvent(self.event);
            }
            Ok(())
        }
    }

    /// Open per-process loopback of `pid` (and its child tree). Returns a
    /// [`Source`] that drains exactly like a normal loopback source.
    pub(super) unsafe fn open(pid: u32, name: &str) -> Result<Source, String> {
        let event = CreateEventW(None, true, false, PCWSTR::null())
            .map_err(|e| format!("CreateEvent (process loopback {name}): {e}"))?;
        // Ensure the event is always closed, even on the error paths below.
        let _guard = EventGuard(event);

        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        // Wrap the params struct in a VT_BLOB PROPVARIANT (the activation API's
        // documented contract). Write through an explicit deref of the
        // `ManuallyDrop` union field.
        let mut pv = PROPVARIANT::default();
        {
            let inner = &mut *pv.Anonymous.Anonymous;
            inner.vt = VT_BLOB;
            inner.Anonymous.blob = BLOB {
                cbSize: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                pBlobData: &mut params as *mut _ as *mut u8,
            };
        }
        // CRITICAL: `PROPVARIANT`'s `Drop` calls `PropVariantClear`, which for a
        // `VT_BLOB` does `CoTaskMemFree(blob.pBlobData)`. Our `pBlobData` points at
        // the *stack* local `params`, NOT COM-allocated memory — so letting the
        // destructor run frees a stack pointer through the COM heap and corrupts it
        // (STATUS_HEAP_CORRUPTION 0xc0000374, surfacing later at an unrelated
        // alloc/free). Suppress the destructor with `ManuallyDrop`: nothing in this
        // PROPVARIANT is heap-owned, so this leaks nothing, and `params` stays alive
        // on the stack for the whole (synchronously-awaited) activation call below.
        let pv = std::mem::ManuallyDrop::new(pv);

        let handler: IActivateAudioInterfaceCompletionHandler = Handler { event }.into();
        let op: IActivateAudioInterfaceAsyncOperation = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&*pv),
            &handler,
        )
        .map_err(|e| format!("ActivateAudioInterfaceAsync ({name}): {e}"))?;

        // Block until ActivateCompleted signals the event, then fetch the result.
        WaitForSingleObject(event, ACTIVATE_TIMEOUT_MS);
        let mut activate_hr = HRESULT(0);
        let mut unknown: Option<IUnknown> = None;
        op.GetActivateResult(&mut activate_hr, &mut unknown)
            .map_err(|e| format!("GetActivateResult ({name}): {e}"))?;
        activate_hr
            .ok()
            .map_err(|e| format!("process loopback activation failed ({name}): {e}"))?;
        let audio_client: IAudioClient = unknown
            .ok_or_else(|| format!("process loopback returned no interface ({name})"))?
            .cast()
            .map_err(|e| format!("cast IAudioClient ({name}): {e}"))?;

        // Fixed 48 kHz / 2ch / 32-bit float format — there's no mix format to
        // query on the virtual device.
        let block_align: u16 = (MIX_CHANNELS as u16) * 4;
        let wfx = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: MIX_CHANNELS as u16,
            nSamplesPerSec: MIX_RATE as u32,
            nAvgBytesPerSec: MIX_RATE as u32 * block_align as u32,
            nBlockAlign: block_align,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        // Shared loopback, polled like the other sources (no event callback). A
        // ~20 ms engine buffer is plenty for our ~10 ms poll cadence.
        const REFTIMES_20MS: i64 = 200_000; // 20 ms in 100 ns units
        audio_client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                REFTIMES_20MS,
                0,
                &wfx,
                None,
            )
            .map_err(|e| format!("Initialize process loopback ({name}): {e}"))?;

        let capture: IAudioCaptureClient = audio_client
            .GetService()
            .map_err(|e| format!("GetService(IAudioCaptureClient) ({name}): {e}"))?;

        let fmt = parse_format(&wfx);
        let swr =
            build_resampler(&fmt).map_err(|e| format!("resampler (process loopback {name}): {e}"))?;

        Ok(Source {
            audio_client,
            capture,
            fmt,
            swr,
            next_idx: 0,
            started: false,
            mono: false,
            label: "process",
        })
    }

    /// Closes the activation event when [`open`] returns (success or error).
    struct EventGuard(HANDLE);
    impl Drop for EventGuard {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Timeline + per-track mixer on an absolute 48 kHz sample timeline
// ---------------------------------------------------------------------------

/// The shared 48 kHz sample timeline: QPC-unit calibration + the epoch anchor.
///
/// One `Timeline` is shared across all output-track mixers so every track (and
/// the video) places samples on the *same* axis — tracks stay sample-aligned
/// with each other and the existing video clock. (Previously this state lived
/// inside the single `Mixer`; splitting it out is what lets there be N mixers.)
struct Timeline {
    /// QPC time (100 ns ticks) of absolute frame index 0. Set by the first
    /// packet of any source.
    epoch_ticks: Option<i64>,
    /// Detected conversion from raw `QPCPosition` to 100 ns ticks (see
    /// [`detect_qpc_scale`]). `None` until the first packet calibrates it.
    qpc_is_100ns: Option<bool>,
}

impl Timeline {
    fn new() -> Timeline {
        Timeline {
            epoch_ticks: None,
            qpc_is_100ns: None,
        }
    }

    /// The epoch tick once anchored (the 100 ns time of frame index 0).
    fn epoch(&self) -> Option<i64> {
        self.epoch_ticks
    }

    /// Convert a raw `QPCPosition` to 100 ns ticks, calibrating the unit on first
    /// use against `QueryPerformanceCounter`/`Frequency` (so we don't depend on
    /// whether the driver reports raw ticks or pre-converted 100 ns).
    fn qpc_to_ticks(&mut self, qpc_pos: u64, qpc_freq: i64) -> i64 {
        let is_100ns = *self
            .qpc_is_100ns
            .get_or_insert_with(|| detect_qpc_scale(qpc_pos, qpc_freq));
        if is_100ns {
            qpc_pos as i64
        } else {
            (qpc_pos as i128 * TICKS_PER_SECOND as i128 / qpc_freq.max(1) as i128) as i64
        }
    }

    /// Absolute 48 kHz frame index for a 100 ns tick (anchoring the epoch on
    /// first call).
    fn tick_to_idx(&mut self, tick: i64) -> i64 {
        let epoch = *self.epoch_ticks.get_or_insert(tick);
        ((tick - epoch) as i128 * MIX_RATE as i128 / TICKS_PER_SECOND as i128) as i64
    }
}

/// Additive accumulator for **one** output track on the shared [`Timeline`].
/// `mix` holds interleaved L,R from absolute frame index `mix_base`; the inputs
/// routed to this track add (gain-scaled) into it, and full `block`-sized frames
/// drain out once the inputs have caught up.
struct TrackMixer {
    mix: VecDeque<f32>,
    mix_base: i64,
    /// Highest absolute index any input has written into this track — the mixer
    /// never emits past this minus latency, so late inputs still get mixed in.
    high_water: i64,
    block: usize,
}

impl TrackMixer {
    fn new(block: usize) -> TrackMixer {
        TrackMixer {
            mix: VecDeque::new(),
            mix_base: 0,
            high_water: 0,
            block: block.max(1),
        }
    }

    /// Additively mix `samples` (interleaved stereo f32) starting at absolute
    /// frame index `at`, each sample scaled by `gain` (the input's relative
    /// volume into this track).
    fn add_scaled(&mut self, at: i64, samples: &[f32], gain: f32) {
        if samples.is_empty() {
            return;
        }
        let n = samples.len() / 2;
        // Trim anything already drained (older than mix_base).
        let (at, skip) = if at < self.mix_base {
            (self.mix_base, (self.mix_base - at) as usize)
        } else {
            (at, 0)
        };
        if skip >= n {
            return;
        }
        let start = (at - self.mix_base) as usize; // in frames
        let needed = (start + (n - skip)) * 2;
        if self.mix.len() < needed {
            self.mix.resize(needed, 0.0);
        }
        for i in skip..n {
            let di = (start + (i - skip)) * 2;
            if let Some(v) = self.mix.get_mut(di) {
                *v += samples[i * 2] * gain;
            }
            if let Some(v) = self.mix.get_mut(di + 1) {
                *v += samples[i * 2 + 1] * gain;
            }
        }
        self.high_water = self.high_water.max(at + (n - skip) as i64);
    }

    /// Pull out every full `block`-frame chunk this track's inputs have caught up
    /// past (or all remaining when `final_flush`). Each chunk is paired with the
    /// 100 ns tick of its first sample (off the shared `epoch`) for muxing.
    fn drain_ready(&mut self, final_flush: bool, epoch: i64) -> Vec<(Vec<f32>, i64)> {
        let mut out = Vec::new();
        let limit = if final_flush {
            self.high_water
        } else {
            self.high_water - LATENCY_SAMPLES
        };
        while self.mix_base + self.block as i64 <= limit && self.mix.len() >= self.block * 2 {
            let mut chunk = Vec::with_capacity(self.block * 2);
            for _ in 0..self.block * 2 {
                chunk.push(self.mix.pop_front().unwrap_or(0.0));
            }
            let pts_ticks =
                epoch + (self.mix_base as i128 * TICKS_PER_SECOND as i128 / MIX_RATE as i128) as i64;
            out.push((chunk, pts_ticks));
            self.mix_base += self.block as i64;
        }
        out
    }
}

/// Decide whether `qpc_pos` is already in 100 ns units or raw QPC ticks by
/// comparing both interpretations of "now" against the live performance counter.
/// On modern hardware `qpc_freq` is often exactly 10 MHz, making the two
/// identical — but on TSC-derived counters it isn't, so we must check.
fn detect_qpc_scale(qpc_pos: u64, qpc_freq: i64) -> bool {
    let mut now_raw = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut now_raw);
    }
    let now_secs = now_raw as f64 / qpc_freq.max(1) as f64;
    let as_100ns = qpc_pos as f64 / TICKS_PER_SECOND as f64;
    let as_raw = qpc_pos as f64 / qpc_freq.max(1) as f64;
    // The packet time must be very close to "now" (it's the just-read position).
    (as_100ns - now_secs).abs() <= (as_raw - now_secs).abs()
}

// ---------------------------------------------------------------------------
// AAC encoder
// ---------------------------------------------------------------------------

/// FFmpeg native AAC encoder, fed exactly `frame_size` stereo samples per call.
struct AacEncoder {
    ctx: *mut ffi::AVCodecContext,
    frame: *mut ffi::AVFrame,
    packet: *mut ffi::AVPacket,
    /// Monotonic sample-count PTS handed to the encoder (it wants sample units).
    sample_pts: i64,
    /// Ticks of each fed block, popped in order to stamp output packets — the
    /// encoder's small constant delay preserves order, so FIFO mapping keeps the
    /// real wall-clock tick on every packet.
    pending_ticks: VecDeque<i64>,
    /// Last tick assigned to an output packet, so a trailing flush/padding packet
    /// (emitted after the queue drains) continues the timeline instead of
    /// jumping back to the encoder's raw sample pts.
    last_tick: i64,
    /// Wall-clock span of one encoded frame, in 100 ns ticks.
    block_ticks: i64,
}

impl AacEncoder {
    fn new() -> Result<AacEncoder, String> {
        unsafe {
            let codec = ffi::avcodec_find_encoder(ffi::AV_CODEC_ID_AAC);
            if codec.is_null() {
                return Err("AAC encoder not found in linked FFmpeg".into());
            }
            let ctx = ffi::avcodec_alloc_context3(codec);
            if ctx.is_null() {
                return Err("avcodec_alloc_context3(aac) failed".into());
            }
            (*ctx).sample_fmt = ffi::AV_SAMPLE_FMT_FLTP;
            (*ctx).sample_rate = MIX_RATE;
            (*ctx).bit_rate = AAC_BITRATE;
            ffi::av_channel_layout_default(&mut (*ctx).ch_layout, MIX_CHANNELS);
            (*ctx).time_base = ffi::AVRational {
                num: 1,
                den: MIX_RATE,
            };
            (*ctx).flags |= AV_CODEC_FLAG_GLOBAL_HEADER;

            let r = ffi::avcodec_open2(ctx, codec, ptr::null_mut());
            if r < 0 {
                let mut c = ctx;
                ffi::avcodec_free_context(&mut c);
                return Err(format!("avcodec_open2(aac): {}", av_err(r)));
            }

            let mut frame_size = (*ctx).frame_size;
            if frame_size <= 0 {
                frame_size = 1024; // AAC LC default if the encoder didn't set it
            }
            (*ctx).frame_size = frame_size;

            let frame = ffi::av_frame_alloc();
            if frame.is_null() {
                let mut c = ctx;
                ffi::avcodec_free_context(&mut c);
                return Err("av_frame_alloc(aac) failed".into());
            }
            (*frame).nb_samples = frame_size;
            (*frame).format = ffi::AV_SAMPLE_FMT_FLTP;
            ffi::av_channel_layout_default(&mut (*frame).ch_layout, MIX_CHANNELS);
            let r = ffi::av_frame_get_buffer(frame, 0);
            if r < 0 {
                let mut f = frame;
                ffi::av_frame_free(&mut f);
                let mut c = ctx;
                ffi::avcodec_free_context(&mut c);
                return Err(format!("av_frame_get_buffer(aac): {}", av_err(r)));
            }

            let packet = ffi::av_packet_alloc();
            if packet.is_null() {
                let mut f = frame;
                ffi::av_frame_free(&mut f);
                let mut c = ctx;
                ffi::avcodec_free_context(&mut c);
                return Err("av_packet_alloc(aac) failed".into());
            }

            Ok(AacEncoder {
                ctx,
                frame,
                packet,
                sample_pts: 0,
                pending_ticks: VecDeque::new(),
                last_tick: 0,
                block_ticks: frame_size as i64 * TICKS_PER_SECOND / MIX_RATE as i64,
            })
        }
    }

    fn frame_size(&self) -> usize {
        unsafe { (*self.ctx).frame_size.max(1) as usize }
    }

    fn extradata(&self) -> Vec<u8> {
        unsafe {
            let c = &*self.ctx;
            if c.extradata.is_null() || c.extradata_size <= 0 {
                return Vec::new();
            }
            std::slice::from_raw_parts(c.extradata, c.extradata_size as usize).to_vec()
        }
    }

    /// Encode one block of `frame_size` interleaved-stereo samples whose first
    /// sample is at `pts_ticks` (100 ns). Returns packets the encoder produced.
    fn encode_block(&mut self, block: &[f32], pts_ticks: i64) -> Result<Vec<EncodedPacket>, String> {
        let n = self.frame_size();
        unsafe {
            let r = ffi::av_frame_make_writable(self.frame);
            if r < 0 {
                return Err(format!("av_frame_make_writable(aac): {}", av_err(r)));
            }
            // De-interleave into the FLTP planes (data[0]=L, data[1]=R).
            let l = (*self.frame).data[0] as *mut f32;
            let rch = (*self.frame).data[1] as *mut f32;
            for i in 0..n {
                let (lv, rv) = match (block.get(i * 2), block.get(i * 2 + 1)) {
                    (Some(&a), Some(&b)) => (a, b),
                    _ => (0.0, 0.0),
                };
                *l.add(i) = lv;
                *rch.add(i) = rv;
            }
            (*self.frame).pts = self.sample_pts;
            self.sample_pts += n as i64;
            self.pending_ticks.push_back(pts_ticks);

            let r = ffi::avcodec_send_frame(self.ctx, self.frame);
            if r < 0 {
                return Err(format!("avcodec_send_frame(aac): {}", av_err(r)));
            }
            self.drain()
        }
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, String> {
        unsafe {
            let r = ffi::avcodec_send_frame(self.ctx, ptr::null());
            if r < 0 && r != AVERROR_EOF {
                return Err(format!("avcodec_send_frame(aac flush): {}", av_err(r)));
            }
            self.drain()
        }
    }

    unsafe fn drain(&mut self) -> Result<Vec<EncodedPacket>, String> {
        let mut out = Vec::new();
        loop {
            let r = ffi::avcodec_receive_packet(self.ctx, self.packet);
            if r == AVERROR_EAGAIN || r == AVERROR_EOF {
                break;
            }
            if r < 0 {
                return Err(format!("avcodec_receive_packet(aac): {}", av_err(r)));
            }
            let pkt = &*self.packet;
            let data = std::slice::from_raw_parts(pkt.data, pkt.size as usize).to_vec();
            // Stamp with the real wall-clock tick of the matching input block; a
            // trailing flush packet (queue empty) continues from the last tick.
            let tick = self
                .pending_ticks
                .pop_front()
                .unwrap_or(self.last_tick + self.block_ticks);
            self.last_tick = tick;
            out.push(EncodedPacket {
                data,
                pts: tick,
                dts: tick,
                keyframe: true, // every AAC frame is independently decodable
            });
            ffi::av_packet_unref(self.packet);
        }
        Ok(out)
    }
}

impl Drop for AacEncoder {
    fn drop(&mut self) {
        unsafe {
            if !self.packet.is_null() {
                ffi::av_packet_free(&mut self.packet);
            }
            if !self.frame.is_null() {
                ffi::av_frame_free(&mut self.frame);
            }
            if !self.ctx.is_null() {
                ffi::avcodec_free_context(&mut self.ctx);
            }
        }
    }
}

/// Encode an interleaved-stereo **48 kHz f32** buffer to AAC packets, with PTS in
/// 100 ns ticks starting at 0. Used by the editor export re-mux
/// ([`crate::library::remux`]) to write a freshly-mixed master track from the
/// clip's decoded stems. The final partial block is padded with silence.
/// Returns the stream meta (ASC `extradata`) + the encoded packets.
pub fn encode_pcm_to_aac(samples: &[f32]) -> Result<(AudioMeta, Vec<EncodedPacket>), String> {
    let mut enc = AacEncoder::new()?;
    let meta = AudioMeta {
        sample_rate: MIX_RATE as u32,
        channels: MIX_CHANNELS as u32,
        extradata: enc.extradata(),
    };
    let block = enc.frame_size();
    let total_frames = samples.len() / 2;
    let mut packets = Vec::new();
    let mut i = 0usize;
    while i < total_frames {
        let from = i * 2;
        let to = ((i + block).min(total_frames)) * 2;
        let mut chunk = samples[from..to].to_vec();
        chunk.resize(block * 2, 0.0); // pad the trailing partial block with silence
        let pts_ticks = i as i64 * TICKS_PER_SECOND / MIX_RATE as i64;
        packets.extend(enc.encode_block(&chunk, pts_ticks)?);
        i += block;
    }
    packets.extend(enc.flush()?);
    Ok((meta, packets))
}

/// Test helper: encode `secs` of silence to real AAC packets (ticks starting at
/// 0). Deterministic and device-free, so the muxer's audio path can be tested
/// headlessly. Used by `mux.rs` tests.
#[cfg(test)]
pub(crate) fn encode_silence_aac(secs: f64) -> (AudioMeta, Vec<EncodedPacket>) {
    let mut enc = AacEncoder::new().expect("aac encoder");
    let meta = AudioMeta {
        sample_rate: MIX_RATE as u32,
        channels: MIX_CHANNELS as u32,
        extradata: enc.extradata(),
    };
    let block = enc.frame_size();
    let nblocks = (secs * MIX_RATE as f64 / block as f64) as i64;
    let silence = vec![0f32; block * 2];
    let mut packets = Vec::new();
    for i in 0..nblocks {
        let pts_ticks = i * block as i64 * TICKS_PER_SECOND / MIX_RATE as i64;
        packets.extend(enc.encode_block(&silence, pts_ticks).expect("encode_block"));
    }
    packets.extend(enc.flush().expect("flush"));
    (meta, packets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// ADTS sampling-frequency index for the standard rates.
    fn freq_index(rate: u32) -> u8 {
        match rate {
            96000 => 0,
            88200 => 1,
            64000 => 2,
            48000 => 3,
            44100 => 4,
            32000 => 5,
            24000 => 6,
            22050 => 7,
            16000 => 8,
            12000 => 9,
            11025 => 10,
            8000 => 11,
            _ => 3,
        }
    }

    /// Prepend a 7-byte ADTS header to each raw AAC-LC frame so the concatenated
    /// bytes are a playable `.aac` file (our encoder emits raw frames for MP4, no
    /// ADTS). Lets a human verify the *sound* — the part no assertion can check.
    fn write_adts(path: &std::path::Path, meta: &AudioMeta, packets: &[EncodedPacket]) {
        let fi = freq_index(meta.sample_rate);
        let ch = meta.channels as u8;
        let mut out = Vec::new();
        for p in packets {
            let frame_len = (p.data.len() + 7) as u32;
            // profile=AAC LC (object type 2 → 1), protection absent.
            out.push(0xFF);
            out.push(0xF1);
            out.push(((1 << 6) | (fi << 2) | ((ch >> 2) & 1)) as u8);
            out.push((((ch & 3) << 6) as u32 | (frame_len >> 11)) as u8);
            out.push(((frame_len >> 3) & 0xFF) as u8);
            out.push((((frame_len & 7) << 5) | 0x1F) as u8);
            out.push(0xFC);
            out.extend_from_slice(&p.data);
        }
        std::fs::write(path, out).expect("write aac");
    }

    /// Live capture of desktop audio (+ mic) for a few seconds → AAC. **Play
    /// something audible while running this**, then listen to the printed `.aac`.
    /// In silence WASAPI loopback delivers no packets, so we don't assert a
    /// packet count — we assert the pipeline is sound (encoder opens, produces
    /// valid ASC extradata, and any packets carry monotonic ticks).
    #[test]
    fn captures_desktop_audio_to_aac() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let result = (|| -> Result<(), String> {
            unsafe {
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                        .map_err(|e| format!("enumerator: {e}"))?;
                let mut sources = Vec::new();
                match Source::open_loopback(&enumerator) {
                    Ok(s) => sources.push(s),
                    Err(e) => println!("loopback unavailable: {e}"),
                }
                match Source::open_mic(&enumerator) {
                    Ok(s) => sources.push(s),
                    Err(e) => println!("mic unavailable: {e}"),
                }
                if sources.is_empty() {
                    return Err("no audio devices".into());
                }

                let mut encoder = AacEncoder::new()?;
                let meta = AudioMeta {
                    sample_rate: MIX_RATE as u32,
                    channels: MIX_CHANNELS as u32,
                    extradata: encoder.extradata(),
                };
                assert!(
                    !meta.extradata.is_empty(),
                    "AAC encoder produced no AudioSpecificConfig (GLOBAL_HEADER off?)"
                );
                let block = encoder.frame_size();
                println!("aac frame_size={block}, asc={} bytes", meta.extradata.len());

                for s in &sources {
                    s.start()?;
                }
                let qpc_freq = {
                    let mut f = 0i64;
                    QueryPerformanceFrequency(&mut f).ok();
                    f.max(1)
                };

                // One shared timeline + a single output-track mixer (every source
                // at unity gain) — the single-track configuration.
                let mut timeline = Timeline::new();
                let mut mixer = TrackMixer::new(block);
                let mut scratch = Vec::new();
                let mut zero = Vec::new();
                let mut packets: Vec<EncodedPacket> = Vec::new();

                let start = Instant::now();
                while start.elapsed().as_secs_f64() < 4.0 {
                    let mut any = false;
                    for src in &mut sources {
                        any |= src.drain(&mut timeline, qpc_freq, &mut scratch, &mut zero, |at, s| {
                            mixer.add_scaled(at, s, 1.0)
                        });
                    }
                    if let Some(epoch) = timeline.epoch() {
                        for (samples, pts) in mixer.drain_ready(false, epoch) {
                            packets.extend(encoder.encode_block(&samples, pts)?);
                        }
                    }
                    if !any {
                        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
                    }
                }
                for s in &sources {
                    let _ = s.audio_client.Stop();
                }
                if let Some(epoch) = timeline.epoch() {
                    for (samples, pts) in mixer.drain_ready(true, epoch) {
                        packets.extend(encoder.encode_block(&samples, pts)?);
                    }
                }
                packets.extend(encoder.flush()?);

                println!("captured {} AAC packets", packets.len());
                // Ticks must be non-decreasing (FIFO mapping preserves order).
                for w in packets.windows(2) {
                    assert!(w[1].pts >= w[0].pts, "audio ticks went backwards");
                }
                if !packets.is_empty() {
                    let out = std::env::temp_dir().join("hako_audio_test.aac");
                    write_adts(&out, &meta, &packets);
                    println!("WROTE PLAYABLE AUDIO → {} (play it to verify sound)", out.display());
                } else {
                    println!("no audio captured (was anything playing?) — pipeline still OK");
                }
                Ok(())
            }
        })();
        unsafe {
            CoUninitialize();
        }
        result.expect("audio capture pipeline");
    }

    /// Capture the **microphone alone** for 5 s and report packet count + peak
    /// amplitude — the isolated check the combined `captures_desktop_audio_to_aac`
    /// test can't give (there mic + loopback land in one track, so a silent mic is
    /// masked by desktop audio). **Speak into the mic while this runs**, then play
    /// the printed `.aac`. Fails loudly if the default capture endpoint can't be
    /// opened; warns (doesn't fail) if it opens but delivers pure silence, since a
    /// truly muted/disconnected mic is an environment issue, not a code bug.
    #[test]
    fn captures_microphone_alone() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let result = (|| -> Result<(), String> {
            unsafe {
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                        .map_err(|e| format!("enumerator: {e}"))?;
                let mut mic = Source::open_mic(&enumerator)
                    .map_err(|e| format!("open default mic: {e}"))?;
                println!(
                    "mic mix format: {} Hz, {} ch, block_align={}",
                    mic.fmt.rate, mic.fmt.channels, mic.fmt.block_align
                );

                let mut encoder = AacEncoder::new()?;
                let meta = AudioMeta {
                    sample_rate: MIX_RATE as u32,
                    channels: MIX_CHANNELS as u32,
                    extradata: encoder.extradata(),
                };
                let block = encoder.frame_size();
                mic.start()?;
                let qpc_freq = {
                    let mut f = 0i64;
                    QueryPerformanceFrequency(&mut f).ok();
                    f.max(1)
                };

                let mut timeline = Timeline::new();
                let mut mixer = TrackMixer::new(block);
                let mut scratch = Vec::new();
                let mut zero = Vec::new();
                let mut packets: Vec<EncodedPacket> = Vec::new();
                let mut peak = 0f32;
                let mut sink_runs = 0u64;

                let start = Instant::now();
                while start.elapsed().as_secs_f64() < 5.0 {
                    let any = mic.drain(&mut timeline, qpc_freq, &mut scratch, &mut zero, |at, s| {
                        sink_runs += 1;
                        for &v in s {
                            peak = peak.max(v.abs());
                        }
                        mixer.add_scaled(at, s, 1.0);
                    });
                    if let Some(epoch) = timeline.epoch() {
                        for (samples, pts) in mixer.drain_ready(false, epoch) {
                            packets.extend(encoder.encode_block(&samples, pts)?);
                        }
                    }
                    if !any {
                        std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
                    }
                }
                let _ = mic.audio_client.Stop();
                if let Some(epoch) = timeline.epoch() {
                    for (samples, pts) in mixer.drain_ready(true, epoch) {
                        packets.extend(encoder.encode_block(&samples, pts)?);
                    }
                }
                packets.extend(encoder.flush()?);

                let peak_db = if peak > 0.0 { 20.0 * peak.log10() } else { f32::NEG_INFINITY };
                println!(
                    "MIC: {sink_runs} buffer(s) drained, {} AAC packet(s), peak amplitude {peak:.4} ({peak_db:.1} dBFS)",
                    packets.len()
                );

                // The mic MUST produce captured buffers — if it opens and starts,
                // WASAPI shared capture delivers data continuously. Zero buffers
                // means a real capture bug (the thing the user reported).
                assert!(
                    sink_runs > 0 && !packets.is_empty(),
                    "MIC PRODUCED NO DATA after opening+starting — real capture bug"
                );
                if peak < 1e-4 {
                    println!(
                        "WARNING: mic delivered only silence (peak ~{peak:.6}). Check the \
                         default recording device / mic level — code path is OK."
                    );
                }
                let out = std::env::temp_dir().join("hako_mic_test.aac");
                write_adts(&out, &meta, &packets);
                println!("WROTE MIC AUDIO → {} (play it to verify your voice)", out.display());
                Ok(())
            }
        })();
        unsafe {
            CoUninitialize();
        }
        result.expect("microphone capture");
    }

    /// Regression test for the process-loopback STATUS_HEAP_CORRUPTION: the
    /// `open()` builds a VT_BLOB PROPVARIANT pointing at a stack local; PROPVARIANT's
    /// Drop calls PropVariantClear → CoTaskMemFree(stack ptr) → heap corruption.
    /// We then churn the heap so the corrupted metadata is hit and the process
    /// fast-fails. If this test survives, the bug is fixed.
    #[test]
    fn process_loopback_open_does_not_corrupt_heap() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        // Our own pid is a valid, live target; we don't care whether activation
        // ultimately succeeds — the bug is the PROPVARIANT drop on `open()` return.
        let pid = std::process::id();
        let _ = unsafe { Source::open_process_loopback(pid, "self-repro") };
        // Hammer the heap: a corrupted free-list/header now faults here.
        let mut sink: Vec<Vec<u8>> = Vec::new();
        for i in 0..5000 {
            sink.push(vec![(i % 251) as u8; 1024 + (i % 4096)]);
            if sink.len() > 64 {
                sink.drain(0..32);
            }
        }
        unsafe {
            CoUninitialize();
        }
        println!("survived process_loopback open + {} heap ops", sink.len());
    }
}
