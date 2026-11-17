//! Read-only facts about the machine that launching and preparing need: how much RAM there is
//! (to pick the game's memory) and how much free disk space the Clade folder's drive has.
//! Nothing here reads the environment or the registry directly and nothing is stored. Two Windows
//! display preferences (transparency effects, High Contrast) are read through their OS APIs.

use std::path::Path;

const MIB: u64 = 1024 * 1024;

/// Point-in-time physical RAM and virtual commit-charge availability in MiB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub total_mb: u64,
    pub avail_phys_mb: u64,
    pub avail_commit_mb: u64,
}

/// Physical memory and commit availability in MiB, or `None` where they cannot be read.
#[cfg(windows)]
pub fn memory_snapshot() -> Option<MemorySnapshot> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: MEMORYSTATUSEX is plain data; the length field is set as the API requires and the
    // pointer is valid for the duration of the call.
    unsafe {
        let mut s: MEMORYSTATUSEX = std::mem::zeroed();
        s.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut s) != 0).then_some(MemorySnapshot {
            total_mb: s.ullTotalPhys / MIB,
            avail_phys_mb: s.ullAvailPhys / MIB,
            avail_commit_mb: s.ullAvailPageFile / MIB,
        })
    }
}

#[cfg(not(windows))]
pub fn memory_snapshot() -> Option<MemorySnapshot> {
    None
}

/// Total physical memory in MiB, or `None` where it cannot be read.
pub fn total_memory_mb() -> Option<u64> {
    memory_snapshot().map(|s| s.total_mb)
}

/// The Windows display language as a locale name (`en-US`, `ru-RU`), or `None` where it cannot be
/// read. Used only for the first-run language default; it is never stored.
#[cfg(windows)]
pub fn system_locale() -> Option<String> {
    use windows_sys::Win32::Globalization::{GetUserDefaultUILanguage, LCIDToLocaleName};
    // SAFETY: the buffer is valid for `len` UTF-16 units for the duration of the call, which
    // reports how many it wrote (including the terminating NUL).
    unsafe {
        let mut buf = [0u16; 86];
        let n = LCIDToLocaleName(
            u32::from(GetUserDefaultUILanguage()),
            buf.as_mut_ptr(),
            buf.len() as i32,
            0,
        );
        (n > 1).then(|| String::from_utf16_lossy(&buf[..(n - 1) as usize]))
    }
}

#[cfg(not(windows))]
pub fn system_locale() -> Option<String> {
    None
}

/// The Windows "Transparency effects" preference, read through the documented
/// `UISettings.AdvancedEffectsEnabled` property (read-only; nothing is written anywhere). `None`
/// where it cannot be read.
#[cfg(windows)]
pub fn transparency_effects_enabled() -> Option<bool> {
    use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
    use windows::UI::ViewManagement::UISettings;
    // WinRT needs an initialised thread; a short-lived one keeps this independent of the caller's.
    std::thread::spawn(|| {
        // SAFETY: paired with the RoUninitialize below on the same thread.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.ok()?;
        let value = UISettings::new()
            .and_then(|u| u.AdvancedEffectsEnabled())
            .ok();
        // SAFETY: balances the successful RoInitialize above.
        unsafe { RoUninitialize() };
        value
    })
    .join()
    .ok()
    .flatten()
}

#[cfg(not(windows))]
pub fn transparency_effects_enabled() -> Option<bool> {
    None
}

/// Whether Windows High Contrast is on (glass is never used then).
#[cfg(windows)]
pub fn high_contrast() -> bool {
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_GETHIGHCONTRAST};
    // SAFETY: HIGHCONTRASTW is plain data; its size field is set as the API requires and the
    // pointer is valid for the duration of the call.
    unsafe {
        let mut hc: HIGHCONTRASTW = std::mem::zeroed();
        hc.cbSize = std::mem::size_of::<HIGHCONTRASTW>() as u32;
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            hc.cbSize,
            std::ptr::from_mut(&mut hc).cast(),
            0,
        ) != 0
            && hc.dwFlags & HCF_HIGHCONTRASTON != 0
    }
}

#[cfg(not(windows))]
pub fn high_contrast() -> bool {
    false
}

#[cfg(windows)]
fn to_verbatim_win32(path: &Path) -> std::path::PathBuf {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .map(|ch| {
            if ch == u16::from(b'/') {
                u16::from(b'\\')
            } else {
                ch
            }
        })
        .collect();
    const BS: u16 = b'\\' as u16;
    const Q: u16 = b'?' as u16;
    const U: u16 = b'U' as u16;
    const N: u16 = b'N' as u16;
    const C: u16 = b'C' as u16;
    if wide.starts_with(&[BS, BS, Q, BS]) {
        return std::path::PathBuf::from(OsString::from_wide(&wide));
    }
    let mut out = Vec::with_capacity(wide.len() + 8);
    if wide.starts_with(&[BS, BS]) {
        out.extend_from_slice(&[BS, BS, Q, BS, U, N, C, BS]);
        out.extend_from_slice(&wide[2..]);
    } else {
        out.extend_from_slice(&[BS, BS, Q, BS]);
        out.extend_from_slice(&wide);
    }
    std::path::PathBuf::from(OsString::from_wide(&out))
}

/// Free space, in bytes, available to this user on the drive that holds `path`.
#[cfg(windows)]
pub fn free_disk_bytes(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    // Resolve to an extended-length verbatim path first so paths > MAX_PATH (260 chars) work,
    // then walk up to a folder that exists (the target may not have been created yet).
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let verbatim = to_verbatim_win32(&abs);
    let mut dir: &Path = &verbatim;
    while !dir.exists() {
        dir = dir.parent()?;
    }
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated and outlives the call; the out pointer is valid; the two
    // optional out parameters are allowed to be null.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

#[cfg(not(windows))]
pub fn free_disk_bytes(_path: &Path) -> Option<u64> {
    None
}

/// Safe maximum JVM heap ceiling (MiB) for a machine with `total_mb` physical RAM, reserving
/// headroom for Windows, integrated graphics and JVM off-heap allocations on 4–8 GB PCs.
fn safe_ceiling_mb(total_mb: u64) -> u32 {
    let capped = if total_mb <= 4403 {
        total_mb.saturating_sub(1920)
    } else if total_mb <= 8704 {
        total_mb.saturating_sub(2816)
    } else {
        (total_mb.saturating_mul(3) / 4).min(32_768)
    };
    (capped / 256 * 256).max(1024) as u32
}

/// The automatic game memory (MiB): conservative on 4 GiB iGPU machines (`1536 MiB`, or `1024 MiB`
/// on `<=3 GiB`), otherwise a quarter of RAM clamped to `2048..=8192` and kept within the machine's
/// safe ceiling and half its RAM, snapped to 256 MiB steps. Unknown RAM means 2 GiB.
pub fn auto_memory_mb(total_mb: Option<u64>) -> u32 {
    let Some(total) = total_mb else { return 2048 };
    if total <= 4403 {
        (total.saturating_sub(1920).min(1536) / 256 * 256).max(1024) as u32
    } else {
        ((total / 4)
            .clamp(2048, 8192)
            .min(u64::from(safe_ceiling_mb(total)))
            .min(total / 2)
            / 256
            * 256)
            .max(1024) as u32
    }
}

/// The range (MiB) a person may choose for the game's memory on this machine: at least 1 GiB, up
/// to the machine's safe ceiling (never above 32 GiB), in steps of 256 MiB. Unknown RAM allows up
/// to 16 GiB.
pub fn memory_limits_mb(total_mb: Option<u64>) -> (u32, u32) {
    (1024, total_mb.map_or(16_384, safe_ceiling_mb))
}

/// The memory to use: the profile's own setting (kept within sane bounds) or the automatic one.
pub fn memory_for(requested: Option<u32>, total_mb: Option<u64>) -> u32 {
    match requested {
        Some(r) => {
            let ceiling = total_mb.map_or(32_768, safe_ceiling_mb);
            r.clamp(512, ceiling)
        }
        None => auto_memory_mb(total_mb),
    }
}

/// The memory for a launch: the profile's own setting, else the global one from Settings, else the
/// automatic policy; always kept within sane bounds.
pub fn effective_memory_mb(
    profile: Option<u32>,
    global: Option<u32>,
    total_mb: Option<u64>,
) -> u32 {
    memory_for(profile.or(global), total_mb)
}

/// Live launch-time heap size (MiB): starts from [`effective_memory_mb`] and clamps down if current
/// Windows commit charge or available physical RAM cannot safely hold the heap plus JVM/OpenGL
/// off-heap overhead.
pub fn launch_memory_mb(
    profile: Option<u32>,
    global: Option<u32>,
    snap: Option<MemorySnapshot>,
) -> u32 {
    let base = effective_memory_mb(profile, global, snap.map(|s| s.total_mb));
    if let Some(s) = snap {
        let commit_cap = s.avail_commit_mb.saturating_sub(512);
        let phys_cap = s.avail_phys_mb.saturating_sub(384);
        let live_cap = commit_cap.min(phys_cap);
        if live_cap < u64::from(base) {
            return ((live_cap / 64 * 64).max(512) as u32).min(base);
        }
    }
    base
}

/// Bytes needed beyond the download itself: staging, extraction and a safety margin.
pub fn space_needed(download_bytes: u64, runs_loader_installer: bool) -> u64 {
    download_bytes
        + download_bytes / 10
        + 384 * MIB
        + if runs_loader_installer { 768 * MIB } else { 0 }
}

pub fn human(bytes: u64) -> String {
    if bytes >= 1024 * MIB {
        format!("{:.1} GB", bytes as f64 / (1024.0 * MIB as f64))
    } else {
        format!("{} MB", bytes.div_ceil(MIB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_memory_scales_with_ram_and_protects_weak_machines() {
        for (total, want) in [
            (None, 2048),
            (Some(2048), 1024),
            (Some(3072), 1024), // <=3 GiB defaults to 1024 MiB
            (Some(3840), 1536), // 4 GB laptop with hardware-reserved iGPU RAM
            (Some(4096), 1536), // 4 GB machine leaves room for OS + iGPU + JVM off-heap
            (Some(4403), 1536),
            (Some(8192), 2048),
            (Some(16_384), 4096),
            (Some(32_768), 8192),
            (Some(131_072), 8192), // never excessive
        ] {
            assert_eq!(auto_memory_mb(total), want, "{total:?}");
        }
        for total in (1..=256).map(|g| g * 1024u64) {
            let m = u64::from(auto_memory_mb(Some(total)));
            let (_, max) = memory_limits_mb(Some(total));
            assert!(
                m <= u64::from(max) && m * 2 <= total.max(2048) && m >= 1024 && m % 256 == 0,
                "{total}: auto={m}, max={max}"
            );
        }
    }

    #[test]
    fn a_profile_can_override_memory_but_only_within_bounds() {
        assert_eq!(memory_for(None, Some(16_384)), 4096);
        assert_eq!(memory_for(Some(6144), Some(16_384)), 6144);
        assert_eq!(memory_for(Some(100), Some(16_384)), 512);
        assert_eq!(
            memory_for(Some(60_000), Some(16_384)),
            12_288,
            "at most three quarters of a 16 GiB machine"
        );
        assert_eq!(memory_for(Some(8192), Some(4096)), 2048);
        assert_eq!(memory_for(Some(8192), Some(3840)), 1792);
        assert_eq!(memory_for(Some(8192), Some(8192)), 5376);
        assert_eq!(memory_for(Some(60_000), None), 32_768);
    }

    #[test]
    fn memory_limits_reserve_os_and_igpu_headroom_on_4_to_8_gb_pcs() {
        assert_eq!(memory_limits_mb(Some(16_384)), (1024, 12_288));
        assert_eq!(memory_limits_mb(Some(8_192)), (1024, 5_376));
        assert_eq!(memory_limits_mb(Some(7_936)), (1024, 5_120));
        assert_eq!(memory_limits_mb(Some(4_096)), (1024, 2_048));
        assert_eq!(memory_limits_mb(Some(3_840)), (1024, 1_792));
        assert_eq!(memory_limits_mb(Some(2_048)), (1024, 1_024));
        assert_eq!(
            memory_limits_mb(Some(1_024)),
            (1024, 1_024),
            "never below the minimum"
        );
        assert_eq!(
            memory_limits_mb(Some(262_144)).1,
            32_768,
            "never above 32 GiB"
        );
        assert_eq!(memory_limits_mb(None), (1024, 16_384));
    }

    #[test]
    fn a_profile_setting_beats_the_global_one_which_beats_automatic() {
        assert_eq!(effective_memory_mb(None, None, Some(16_384)), 4096);
        assert_eq!(effective_memory_mb(None, Some(8192), Some(16_384)), 8192);
        assert_eq!(
            effective_memory_mb(Some(3072), Some(8192), Some(16_384)),
            3072
        );
        assert_eq!(
            effective_memory_mb(None, Some(30_000), Some(8_192)),
            5_376,
            "a global value too big for an 8 GiB machine is capped to its safe ceiling"
        );
    }

    #[test]
    fn launch_memory_clamps_under_low_commit_or_physical_ram_pressure() {
        // Plenty of free RAM and commit: uses normal effective memory.
        let roomy = MemorySnapshot {
            total_mb: 8192,
            avail_phys_mb: 5000,
            avail_commit_mb: 7000,
        };
        assert_eq!(launch_memory_mb(None, Some(4096), Some(roomy)), 4096);
        assert_eq!(launch_memory_mb(None, None, None), 2048);

        // Tight Windows commit limit (e.g. small/disabled pagefile with browser open):
        // avail_commit_mb = 1820 -> commit_cap = 1308 -> snapped to 64 MiB = 1280.
        let low_commit = MemorySnapshot {
            total_mb: 8192,
            avail_phys_mb: 4096,
            avail_commit_mb: 1820,
        };
        assert_eq!(launch_memory_mb(None, Some(4096), Some(low_commit)), 1280);

        // Tight physical RAM on a 4 GB laptop:
        // avail_phys_mb = 1450 -> phys_cap = 1066 -> snapped to 64 MiB = 1024.
        let low_phys = MemorySnapshot {
            total_mb: 4096,
            avail_phys_mb: 1450,
            avail_commit_mb: 4096,
        };
        assert_eq!(launch_memory_mb(None, None, Some(low_phys)), 1024);

        // Extreme pressure never drops below 512 MiB and never exceeds base.
        let starved = MemorySnapshot {
            total_mb: 4096,
            avail_phys_mb: 500,
            avail_commit_mb: 600,
        };
        assert_eq!(launch_memory_mb(None, None, Some(starved)), 512);
        assert_eq!(launch_memory_mb(Some(512), None, Some(roomy)), 512);
    }

    #[test]
    fn space_estimates_leave_headroom_and_read_like_a_person_would() {
        assert!(space_needed(1000 * MIB, false) > 1000 * MIB + 384 * MIB);
        assert!(space_needed(1000 * MIB, true) > space_needed(1000 * MIB, false));
        assert_eq!(human(300 * MIB), "300 MB");
        assert_eq!(human(3 * 1024 * MIB), "3.0 GB");
    }

    #[test]
    fn the_machine_can_be_queried_including_deep_long_paths() {
        let d = tempfile::tempdir().unwrap();
        let free = free_disk_bytes(d.path()).expect("free space of the temp drive");
        assert!(free > 0);
        assert!(free_disk_bytes(&d.path().join("not/yet/created")).is_some());

        let seg = "a".repeat(55);
        let mut deep = d.path().to_path_buf();
        for _ in 0..6 {
            deep.push(&seg);
        }
        assert!(deep.as_os_str().len() > 280);
        // Non-existent >260-char path walks up cleanly after verbatim conversion.
        assert!(free_disk_bytes(&deep).is_some());

        #[cfg(windows)]
        {
            assert_eq!(
                to_verbatim_win32(Path::new(r"\\?\D:\Portable\Clade")),
                std::path::PathBuf::from(r"\\?\D:\Portable\Clade")
            );
            assert_eq!(
                to_verbatim_win32(Path::new(r"\\server\share\Clade")),
                std::path::PathBuf::from(r"\\?\UNC\server\share\Clade")
            );
            assert_eq!(
                to_verbatim_win32(Path::new(r"D:\Portable\Clade")),
                std::path::PathBuf::from(r"\\?\D:\Portable\Clade")
            );

            // Existing >260-char directory passed without `\\?\` prefix still succeeds.
            let verbatim_deep = to_verbatim_win32(&deep);
            std::fs::create_dir_all(&verbatim_deep).unwrap();
            assert!(free_disk_bytes(&deep).is_some());
            assert!(free_disk_bytes(&deep.join("missing-leaf")).is_some());
            let _ = std::fs::remove_dir_all(to_verbatim_win32(&d.path().join(&seg)));

            let snap = memory_snapshot().expect("Windows memory snapshot");
            assert!(snap.total_mb >= 512);
            assert!(snap.avail_phys_mb > 0 && snap.avail_commit_mb > 0);
        }
        assert!(total_memory_mb().is_some_and(|m| m >= 512));
    }
}
