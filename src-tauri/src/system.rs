//! Read-only facts about the machine that launching and preparing need: how much RAM there is
//! (to pick the game's memory) and how much free disk space the Clade folder's drive has.
//! Nothing here reads the environment or the registry directly and nothing is stored. Two Windows
//! display preferences (transparency effects, High Contrast) are read through their OS APIs.

use std::path::Path;

/// Total physical memory in MiB, or `None` where it cannot be read.
#[cfg(windows)]
pub fn total_memory_mb() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: MEMORYSTATUSEX is plain data; the length field is set as the API requires and the
    // pointer is valid for the duration of the call.
    unsafe {
        let mut s: MEMORYSTATUSEX = std::mem::zeroed();
        s.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut s) != 0).then_some(s.ullTotalPhys / (1024 * 1024))
    }
}

#[cfg(not(windows))]
pub fn total_memory_mb() -> Option<u64> {
    None
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

/// Free space, in bytes, available to this user on the drive that holds `path`.
#[cfg(windows)]
pub fn free_disk_bytes(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    // Walk up to a folder that exists (the target may not have been created yet).
    let mut dir = path;
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

const MIB: u64 = 1024 * 1024;

/// The automatic game memory (MiB): a quarter of the RAM, never less than 2 GiB or more than
/// 8 GiB, never more than half the machine, and in steps of 256 MiB. Unknown RAM means 2 GiB.
pub fn auto_memory_mb(total_mb: Option<u64>) -> u32 {
    let Some(total) = total_mb else { return 2048 };
    let mb = (total / 4).clamp(2048, 8192).min(total / 2).max(1024);
    (mb / 256 * 256) as u32
}

/// The range (MiB) a person may choose for the game's memory on this machine: at least 1 GiB, at
/// most three quarters of the RAM (never above 32 GiB), in steps of 256 MiB. Unknown RAM allows
/// up to 16 GiB.
pub fn memory_limits_mb(total_mb: Option<u64>) -> (u32, u32) {
    const MIN: u32 = 1024;
    let ceiling = total_mb.map_or(16_384, |t| t * 3 / 4).min(32_768) as u32;
    (MIN, (ceiling / 256 * 256).max(MIN))
}

/// The memory to use: the profile's own setting (kept within sane bounds) or the automatic one.
pub fn memory_for(requested: Option<u32>, total_mb: Option<u64>) -> u32 {
    match requested {
        Some(r) => {
            let ceiling = total_mb.map_or(32_768, |t| (t * 3 / 4).clamp(1024, 32_768)) as u32;
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
    fn automatic_memory_scales_with_ram_within_bounds() {
        for (total, want) in [
            (None, 2048),
            (Some(2048), 1024), // never more than half of a small machine
            (Some(3072), 1536),
            (Some(4096), 2048),
            (Some(8192), 2048),
            (Some(16_384), 4096),
            (Some(32_768), 8192),
            (Some(131_072), 8192), // never excessive
        ] {
            assert_eq!(auto_memory_mb(total), want, "{total:?}");
        }
        for total in (1..=256).map(|g| g * 1024u64) {
            let m = u64::from(auto_memory_mb(Some(total)));
            assert!(
                m * 2 <= total.max(2048) && m >= 1024 && m % 256 == 0,
                "{total}"
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
            "at most three quarters of the machine"
        );
        assert_eq!(memory_for(Some(60_000), None), 32_768);
    }

    #[test]
    fn memory_limits_follow_the_machine_in_256_mib_steps() {
        assert_eq!(memory_limits_mb(Some(16_384)), (1024, 12_288));
        assert_eq!(memory_limits_mb(Some(8_192)), (1024, 6_144));
        assert_eq!(memory_limits_mb(Some(2_048)), (1024, 1_536));
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
            6_144,
            "a global value too big for this machine is still capped"
        );
    }

    #[test]
    fn space_estimates_leave_headroom_and_read_like_a_person_would() {
        assert!(space_needed(1000 * MIB, false) > 1000 * MIB + 384 * MIB);
        assert!(space_needed(1000 * MIB, true) > space_needed(1000 * MIB, false));
        assert_eq!(human(300 * MIB), "300 MB");
        assert_eq!(human(3 * 1024 * MIB), "3.0 GB");
    }

    #[test]
    fn the_machine_can_be_queried() {
        let d = tempfile::tempdir().unwrap();
        let free = free_disk_bytes(d.path()).expect("free space of the temp drive");
        assert!(free > 0);
        assert!(free_disk_bytes(&d.path().join("not/yet/created")).is_some());
        assert!(total_memory_mb().is_some_and(|m| m >= 512));
    }
}
