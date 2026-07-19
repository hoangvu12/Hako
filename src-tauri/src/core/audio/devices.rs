//! Audio endpoint, session, and icon enumeration for the settings UI.
//!
//! Everything here answers "what can the user pick?" for the recorder's audio
//! pickers — capture/render endpoints, the live list of apps currently playing
//! audio, and the exe icons shown beside them. It is called from Tauri command
//! threads (each entry point does its own COM init) and is **not** touched by
//! the capture path in [`super`], which only ever consumes an already-resolved
//! [`crate::settings::AudioConfig`].

use windows::core::{Interface, GUID, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, AudioSessionStateActive, EDataFlow, IAudioSessionControl2,
    IAudioSessionManager2, IMMDevice, IMMDeviceCollection, IMMDeviceEnumerator, MMDeviceEnumerator,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::{
    PropVariantClear, PropVariantToStringAlloc, PROPVARIANT,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;

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
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
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
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device: IMMDevice = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
    let sessions = manager.GetSessionEnumerator()?;
    let count = sessions.GetCount()?;

    // Resolve pids → exe names + paths in one process scan. Process *names* come
    // free with the base enumeration, but the exe *path* (needed to extract the
    // app's real icon) is only populated when we explicitly ask via `with_exe` —
    // otherwise `Process::exe()` is always `None` and every app falls back to the
    // generic icon. `OnlyIfNotSet` keeps it cheap (resolve each path once).
    let mut sys = sysinfo::System::new();
    sys.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::All,
        true,
        sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::OnlyIfNotSet),
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

/// The exe's icon as a PNG `data:` URL (memoized by path), for callers outside the
/// audio-source list — e.g. the "record any game" custom-games list, which stores
/// it on the row so the game's real icon shows even when it isn't running. Best-
/// effort: `None` if the file has no icon or extraction failed.
pub(crate) fn exe_icon_data_url(exe: &std::path::Path) -> Option<String> {
    cached_exe_icon(exe)
}

/// Extract `exe`'s associated icon and encode it as a `data:image/png;base64,…`
/// URL. Best-effort: returns `None` on any failure. Uses `SHGetFileInfoW` to get
/// the `HICON`, then GDI (`GetIconInfo`/`GetDIBits`) to read its 32-bit pixels.
unsafe fn extract_exe_icon_png(exe: &std::path::Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
    use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;

    let wide: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
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
