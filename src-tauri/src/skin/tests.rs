#[cfg(test)]
use super::*;
use crate::net::testing::FakeFetcher;
use std::time::Duration;

const UUID: &str = "00000000-0000-4000-8000-00000000a001";
const OTHER: &str = "00000000-0000-4000-8000-00000000b002";
const ID_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ID_B: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

fn url(id: &str) -> String {
    format!("http://textures.minecraft.net/texture/{id}")
}
fn https(id: &str) -> String {
    format!("https://textures.minecraft.net/texture/{id}")
}

/// A header-only PNG of the given size: all this module ever looks at.
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut b = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13];
    b.extend_from_slice(b"IHDR");
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]); // depth, colour, ..., CRC (not checked)
    b.extend_from_slice(b"fixture pixel data");
    b
}

fn cache() -> (tempfile::TempDir, SkinCache) {
    let d = tempfile::tempdir().unwrap();
    let c = SkinCache::new(d.path().join("cache").join("skins"));
    (d, c)
}

fn run<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(f)
}

#[test]
fn only_the_exact_mojang_texture_address_is_accepted_and_upgraded_to_https() {
    let t = parse_texture(&url(ID_A)).unwrap();
    assert_eq!(t.url.as_str(), https(ID_A));
    assert_eq!(t.id, ID_A);
    assert!(parse_texture(&https(&ID_A.to_uppercase())).is_some());
    for bad in [
        "https://textures.minecraft.net.evil.example/texture/0123456789abcdef0123".to_owned(),
        "https://example.com/texture/0123456789abcdef0123".to_owned(),
        "https://user@textures.minecraft.net/texture/0123456789abcdef0123".to_owned(),
        "https://textures.minecraft.net:8443/texture/0123456789abcdef0123".to_owned(),
        format!("{}?token=SECRET", https(ID_A)),
        format!("{}#frag", https(ID_A)),
        "https://textures.minecraft.net/texture/short".to_owned(),
        "https://textures.minecraft.net/texture/zzzzzzzzzzzzzzzzzzzzzzzz".to_owned(),
        "https://textures.minecraft.net/other/0123456789abcdef0123".to_owned(),
        "ftp://textures.minecraft.net/texture/0123456789abcdef0123".to_owned(),
        "not a url".to_owned(),
    ] {
        assert!(parse_texture(&bad).is_none(), "{bad}");
    }
}

#[test]
fn only_skin_shaped_pngs_are_accepted() {
    assert_eq!(skin_size(&png(64, 64)), Some((64, 64)));
    assert_eq!(skin_size(&png(64, 32)), Some((64, 32)));
    assert_eq!(skin_size(&png(128, 128)), Some((128, 128)));
    assert_eq!(skin_size(&png(1024, 512)), Some((1024, 512)));
    for (w, h) in [
        (0, 0),
        (63, 63),
        (64, 48),
        (96, 96),
        (2048, 2048),
        (64, 128),
    ] {
        assert_eq!(skin_size(&png(w, h)), None, "{w}x{h}");
    }
    assert_eq!(skin_size(b"<html>not an image</html>"), None);
    assert_eq!(skin_size(&[]), None);
    let mut wrong_chunk = png(64, 64);
    wrong_chunk[12..16].copy_from_slice(b"IDAT");
    assert_eq!(skin_size(&wrong_chunk), None);
    let mut huge = png(64, 64);
    huge.resize(MAX_BYTES + 1, 0);
    assert_eq!(skin_size(&huge), None);
}

#[test]
fn extreme_heights_never_overflow_or_wrap_around_into_accepted_skin_dimensions() {
    assert_eq!(skin_size(&png(64, 0x8000_0020)), None);
    assert_eq!(skin_size(&png(64, u32::MAX)), None);
    assert_eq!(skin_size(&png(MAX_EDGE, 0x8000_0200)), None);
}

#[test]
fn oversized_cached_files_are_rejected_without_unbounded_reads() {
    let (_d, c) = cache();
    let f = FakeFetcher::new();
    f.ok(&https(ID_A), png(64, 64));
    c.record(UUID, Some(&url(ID_A)));
    run(c.sync(&f, UUID));
    assert!(c.data_uri(UUID).is_some());
    let mut oversized = png(64, 64);
    oversized.resize(MAX_BYTES + 1, 0);
    fs::write(c.file(UUID, "png").unwrap(), oversized).unwrap();
    assert!(c.data_uri(UUID).is_none());
}

#[test]
fn base64_matches_the_rfc_vectors() {
    for (raw, enc) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64(raw.as_bytes()), enc);
    }
}

#[test]
fn a_reported_skin_is_downloaded_cached_and_served_as_an_inline_png() {
    let (_d, c) = cache();
    let f = FakeFetcher::new();
    f.ok(&https(ID_A), png(64, 64));
    assert!(c.data_uri(UUID).is_none(), "nothing before the first sync");
    c.record(UUID, Some(&url(ID_A)));
    assert!(c.data_uri(UUID).is_none(), "noted, not downloaded yet");
    run(c.sync(&f, UUID));
    let uri = c.data_uri(UUID).unwrap();
    assert!(uri.starts_with("data:image/png;base64,iVBORw0KGgo"));
    assert_eq!(f.calls(), vec![https(ID_A)]);
    // Same skin again: served from the cache, no second request.
    c.record(UUID, Some(&url(ID_A)));
    run(c.sync(&f, UUID));
    assert_eq!(f.calls().len(), 1);
    assert_eq!(c.data_uri(UUID), Some(uri));
    // Nothing leaks into other accounts.
    assert!(c.data_uri(OTHER).is_none());
}

#[test]
fn a_changed_skin_replaces_the_cache_and_a_failed_download_keeps_the_old_one_for_retry() {
    let (_d, c) = cache();
    let f = FakeFetcher::new();
    f.ok(&https(ID_A), png(64, 64));
    f.ok(&https(ID_B), png(128, 128));
    c.record(UUID, Some(&url(ID_A)));
    run(c.sync(&f, UUID));
    let first = c.data_uri(UUID).unwrap();

    // The profile now reports another skin: until it is downloaded the old image is not shown.
    c.record(UUID, Some(&url(ID_B)));
    assert!(c.data_uri(UUID).is_none());
    run(c.sync(&f, UUID));
    let second = c.data_uri(UUID).unwrap();
    assert_ne!(first, second);

    // Offline: the previous texture stays on disk, the next sync retries.
    let g = FakeFetcher::new();
    c.record(UUID, Some(&url(ID_A)));
    run(c.sync(&g, UUID));
    assert!(c.data_uri(UUID).is_none());
    g.ok(&https(ID_A), png(64, 64));
    run(c.sync(&g, UUID));
    assert_eq!(c.data_uri(UUID), Some(first));
}

#[test]
fn unusable_data_never_becomes_an_avatar_and_never_panics() {
    let (_d, c) = cache();
    let f = FakeFetcher::new();
    c.record(UUID, Some(&url(ID_A)));
    // unreachable
    run(c.sync(&f, UUID));
    assert!(c.data_uri(UUID).is_none());
    // malformed / unsupported
    f.ok(&https(ID_A), b"<html>captcha</html>".as_slice());
    run(c.sync(&f, UUID));
    assert!(c.data_uri(UUID).is_none());
    f.ok(&https(ID_A), png(63, 63));
    run(c.sync(&f, UUID));
    assert!(c.data_uri(UUID).is_none());
    // a damaged file in the cache is ignored
    f.ok(&https(ID_A), png(64, 64));
    run(c.sync(&f, UUID));
    assert!(c.data_uri(UUID).is_some());
    fs::write(c.file(UUID, "png").unwrap(), b"garbage").unwrap();
    assert!(c.data_uri(UUID).is_none());
    // an address that is not a texture address is not even requested
    let g = FakeFetcher::new();
    c.record(UUID, Some("https://example.com/skin.png"));
    run(c.sync(&g, UUID));
    assert!(g.calls().is_empty());
    // a uuid that is not a uuid never touches the disk
    c.record("../../evil", Some(&url(ID_A)));
    run(c.sync(&f, "../../evil"));
    assert!(c.data_uri("../../evil").is_none());
}

#[test]
fn the_default_skin_or_a_removed_account_clears_the_cache_and_nothing_secret_is_stored() {
    let (d, c) = cache();
    let f = FakeFetcher::new();
    f.ok(&https(ID_A), png(64, 64));
    c.record(UUID, Some(&url(ID_A)));
    run(c.sync(&f, UUID));
    assert!(c.data_uri(UUID).is_some());
    // Everything lives under cache/skins, as plain public ids.
    let names: Vec<String> = fs::read_dir(d.path().join("cache").join("skins"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 3);
    assert!(names.iter().all(|n| n.starts_with(UUID)));
    // Only the validated texture id is stored, never an address.
    let want = fs::read_to_string(c.file(UUID, "want").unwrap()).unwrap();
    assert_eq!(want, ID_A);
    // Profile without a custom skin.
    c.record(UUID, None);
    assert!(c.data_uri(UUID).is_none());
    assert_eq!(
        fs::read_dir(d.path().join("cache").join("skins"))
            .unwrap()
            .count(),
        0
    );
    // Removal.
    c.record(UUID, Some(&url(ID_A)));
    run(c.sync(&f, UUID));
    c.forget(UUID);
    assert!(c.data_uri(UUID).is_none());
}

/// A fetcher that answers every request after `delay` (or never), to prove how waits add up.
struct Slow {
    barrier: Option<std::sync::Arc<tokio::sync::Barrier>>,
    entered: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    png: Vec<u8>,
}
impl Fetcher for Slow {
    fn get(
        &self,
        _url: &Url,
        _max: usize,
    ) -> impl std::future::Future<Output = Result<Vec<u8>, crate::net::FetchError>> + Send {
        let (barrier, entered, png) =
            (self.barrier.clone(), self.entered.clone(), self.png.clone());
        async move {
            entered.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match barrier {
                Some(b) => {
                    b.wait().await;
                    Ok(png)
                }
                None => std::future::pending().await,
            }
        }
    }
}

fn many(c: &SkinCache, n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let uuid = format!("00000000-0000-4000-8000-{i:012}");
            c.record(&uuid, Some(&url(&format!("{i:0>32}"))));
            uuid
        })
        .collect()
}

#[test]
fn waiting_skins_download_together_so_the_wait_never_scales_with_the_account_count() {
    let (_d, c) = cache();
    let uuids = many(&c, 5);
    // A 5-party barrier only releases when all 5 skin downloads are in flight concurrently;
    // sequential execution would deadlock until the budget expires.
    let slow = Slow {
        barrier: Some(std::sync::Arc::new(tokio::sync::Barrier::new(5))),
        entered: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        png: png(64, 64),
    };
    assert!(run(c.sync_all(&slow, &uuids, Duration::from_secs(5))));
    assert_eq!(slow.entered.load(std::sync::atomic::Ordering::SeqCst), 5);
    assert!(uuids.iter().all(|u| c.data_uri(u).is_some()));
    // Nothing is waiting any more: the next round returns at once without a request.
    assert!(!run(c.sync_all(&slow, &uuids, Duration::from_secs(5))));
}

#[test]
fn a_hung_server_costs_one_budget_in_total_and_leaves_every_skin_pending_and_intact() {
    let (_d, c) = cache();
    let uuids = many(&c, 6);
    let hung = Slow {
        barrier: None,
        entered: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        png: Vec::new(),
    };
    let t0 = std::time::Instant::now();
    assert!(run(c.sync_all(&hung, &uuids, Duration::from_millis(100))));
    assert_eq!(hung.entered.load(std::sync::atomic::Ordering::SeqCst), 6);
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    assert!(uuids
        .iter()
        .all(|u| c.pending(u) && c.data_uri(u).is_none()));
    // No half-written files from the abandoned downloads.
    let skins = c
        .file(&uuids[0], "png")
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    assert!(fs::read_dir(skins).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".png")));
}
