#[cfg(test)]
use super::*;
use crate::accounts::{AccountsFile, ReauthReason};
use crate::download_policy::{DownloadPolicy, DEFAULT_ALLOWED_HOSTS};
use crate::ms_auth::ENDPOINTS;
use crate::net::testing_transport::FakeTransport;
use crate::vault::{AccountSecrets, KdfParams};

const FAST: KdfParams = KdfParams {
    m_cost_kib: 8,
    t_cost: 1,
    p_cost: 1,
};
const NOW: i64 = 1_000_000;
const UUID_A: &str = "00000000-0000-4000-8000-00000000a001";
const UUID_B: &str = "00000000-0000-4000-8000-00000000b002";
const RAW_A: &str = "0000000000004000800000000000a001";
const CAPE_1: &str = "11111111-2222-4333-8444-555555555555";
const CAPE_2: &str = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
/// A recognisable fake token: it must never show up anywhere but the Authorization header.
const TOKEN: &str = "FAKE-MC-ACCESS-TOKEN-do-not-leak";

const PROFILE: &str = "https://api.minecraftservices.com/minecraft/profile";
const SKINS: &str = "https://api.minecraftservices.com/minecraft/profile/skins";
const SKIN_ACTIVE: &str = "https://api.minecraftservices.com/minecraft/profile/skins/active";
const CAPE_ACTIVE: &str = "https://api.minecraftservices.com/minecraft/profile/capes/active";

const PNG_IEND: &[u8; 12] = b"\0\0\0\0IEND\xae\x42\x60\x82";

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    b.extend(w.to_be_bytes());
    b.extend(h.to_be_bytes());
    // bit_depth=8, color_type=6 (RGBA), compression=0, filter=0, interlace=0, 4-byte CRC
    b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    b.extend_from_slice(b"\0\0\0\0IDAT\0\0\0\0");
    b.extend_from_slice(PNG_IEND);
    b
}

fn profile_json(raw_id: &str, skin: &str, capes: &str) -> String {
    format!(
        r#"{{"id":"{raw_id}","name":"Alex_1","skins":[{skin}],"capes":[{capes}],"profileActions":{{}}}}"#
    )
}

const SKIN_URL: &str =
    "http://textures.minecraft.net/texture/0123456789abcdef0123456789abcdef0123456789abcdef";

fn classic() -> String {
    format!(r#"{{"id":"s1","state":"ACTIVE","url":"{SKIN_URL}","variant":"CLASSIC"}}"#)
}

fn slim() -> String {
    format!(r#"{{"id":"s1","state":"ACTIVE","url":"{SKIN_URL}","variant":"SLIM"}}"#)
}

struct Env {
    _dir: tempfile::TempDir,
    accounts: AccountsState,
    vault: Mutex<Option<Vault>>,
    t: FakeTransport,
}

impl Env {
    /// Two accounts, both signed in with a fresh cached token: `a` (the one under test) and `b`.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        let meta = |id: &str, uuid: &str| AccountMeta {
            id: id.into(),
            username: format!("Name_{id}"),
            minecraft_uuid: uuid.into(),
            added_at: 1,
            last_used_at: None,
            status: AccountStatus::Ready,
        };
        AccountsFile {
            schema: 1,
            active: Some("a".into()),
            accounts: vec![meta("a", UUID_A), meta("b", UUID_B)],
        }
        .save(&path)
        .unwrap();
        let mut vault =
            Vault::create_with(&dir.path().join("vault.bin"), "correct horse battery", FAST)
                .unwrap();
        for id in ["a", "b"] {
            vault
                .set_account(
                    id,
                    AccountSecrets {
                        ms_refresh_token: Some("REFRESH".into()),
                        minecraft_access_token: Some(if id == "a" {
                            TOKEN.into()
                        } else {
                            "FAKE-TOKEN-OF-THE-OTHER-ACCOUNT".into()
                        }),
                        minecraft_token_expires_at: Some(NOW + 86_400),
                    },
                )
                .unwrap();
        }
        Self {
            accounts: AccountsState::new(path),
            vault: Mutex::new(Some(vault)),
            t: FakeTransport::new(),
            _dir: dir,
        }
    }

    fn service(&self) -> MinecraftServices<'_, FakeTransport> {
        MinecraftServices {
            transport: &self.t,
            vault: &self.vault,
            accounts: &self.accounts,
            now: NOW,
        }
    }

    fn mark_reauth(&self, id: &str) {
        self.accounts
            .update(|f| f.mark_needs_reauth(id, ReauthReason::Revoked, NOW))
            .unwrap();
    }

    fn calls(&self) -> Vec<(Method, String)> {
        self.t.calls().into_iter().map(|c| (c.0, c.1)).collect()
    }
}

// ------------------------------------------------------------------------------------ parsing

#[test]
fn an_active_classic_skin_is_read_with_its_model_and_texture() {
    let p = parse_profile(profile_json(RAW_A, &classic(), "").as_bytes()).unwrap();
    assert_eq!(p.uuid, UUID_A);
    assert_eq!(p.name, "Alex_1");
    assert_eq!(p.skin_variant, Some(SkinVariant::Classic));
    assert_eq!(p.skin_url.as_deref(), Some(SKIN_URL));
    assert!(p.capes.is_empty());
}

#[test]
fn an_active_slim_skin_is_read_as_slim() {
    let p = parse_profile(profile_json(RAW_A, &slim(), "").as_bytes()).unwrap();
    assert_eq!(p.skin_variant, Some(SkinVariant::Slim));
}

#[test]
fn no_custom_skin_means_no_variant_and_no_texture_is_invented() {
    let p = parse_profile(profile_json(RAW_A, "", "").as_bytes()).unwrap();
    assert_eq!((p.skin_variant, p.skin_url), (None, None));
    // only an INACTIVE skin is listed: it is not the current one
    let inactive = format!(r#"{{"state":"INACTIVE","url":"{SKIN_URL}","variant":"SLIM"}}"#);
    let p = parse_profile(profile_json(RAW_A, &inactive, "").as_bytes()).unwrap();
    assert_eq!((p.skin_variant, p.skin_url), (None, None));
}

#[test]
fn capes_are_listed_with_their_names_and_the_active_one_is_marked() {
    let capes = format!(
        r#"{{"id":"{CAPE_1}","state":"ACTIVE","url":"http://textures.minecraft.net/texture/aa","alias":"Migrator"}},
           {{"id":"{CAPE_2}","state":"INACTIVE","alias":"  Pan\u0007 "}},
           {{"id":"{CAPE_2}x","state":"INACTIVE","alias":"bad id"}},
           {{"state":"INACTIVE"}}"#
    );
    let p = parse_profile(profile_json(RAW_A, &classic(), &capes).as_bytes()).unwrap();
    // the two with unusable ids are left out instead of failing the whole profile
    assert_eq!(p.capes.len(), 2);
    assert_eq!(p.capes[0].id.as_str(), CAPE_1);
    assert_eq!(p.capes[0].alias.as_deref(), Some("Migrator"));
    assert!(p.capes[0].active);
    assert_eq!(
        p.capes[1].alias.as_deref(),
        Some("Pan"),
        "control characters are dropped"
    );
    assert!(!p.capes[1].active);
}

#[test]
fn unknown_fields_are_ignored_and_bad_profiles_are_refused() {
    assert!(parse_profile(
        br#"{"id":"0000000000004000800000000000a001","name":"A","future":{"x":1}}"#
    )
    .is_ok());
    for bad in [
        "not json",
        r#"{"name":"A"}"#,
        r#"{"id":"zz","name":"A"}"#,
        r#"{"id":"0000000000004000800000000000a001","name":"has space"}"#,
        r#"{"id":"0000000000004000800000000000a001","name":""}"#,
        "[]",
    ] {
        assert!(
            matches!(
                parse_profile(bad.as_bytes()),
                Err(ProfileServiceError::BadResponse(_))
            ),
            "{bad}"
        );
    }
}

#[test]
fn cape_ids_are_uuids_only() {
    assert!(CapeId::new("2340C0E0-3A8C-4F5B-9F2E-0123456789AB").is_ok());
    assert_eq!(
        CapeId::new("../../etc").unwrap_err(),
        ProfileServiceError::CapeNotOwned
    );
    assert!(CapeId::new("{\"capeId\":1}").is_err());
    assert!(CapeId::new("").is_err());
}

// ---------------------------------------------------------------------------- skin validation

#[test]
fn skin_files_are_checked_before_use() {
    assert!(SkinPng::validate(png(64, 64)).is_ok());
    assert!(SkinPng::validate(png(64, 32)).is_ok());
    for (bytes, problem) in [
        (png(128, 128), SkinProblem::WrongSize),
        (png(64, 48), SkinProblem::WrongSize),
        (png(0, 0), SkinProblem::WrongSize),
        (
            b"GIF89a-not-a-png-at-all-really".to_vec(),
            SkinProblem::NotPng,
        ),
        (
            b"\xff\xd8\xff\xe0JFIF-jpeg-pretending-to-be-a-skin".to_vec(),
            SkinProblem::NotPng,
        ),
        (
            b"<svg xmlns='http://www.w3.org/2000/svg'/>-------".to_vec(),
            SkinProblem::NotPng,
        ),
        (Vec::new(), SkinProblem::NotPng),
        (vec![0; MAX_SKIN_FILE_BYTES + 1], SkinProblem::TooLarge),
    ] {
        assert_eq!(
            SkinPng::validate(bytes).unwrap_err(),
            ProfileServiceError::InvalidSkin(problem)
        );
    }
    // a wrong chunk length or type is not a PNG header either
    let mut bad = png(64, 64);
    bad[8..12].copy_from_slice(&[0, 0, 0, 14]);
    assert!(SkinPng::validate(bad).is_err());
    // the minimal 45-byte PNG (8 sig + 25 IHDR + 12 IEND) passes; 44 bytes is rejected
    let mut min_valid = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    min_valid.extend(64u32.to_be_bytes());
    min_valid.extend(64u32.to_be_bytes());
    min_valid.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
    min_valid.extend_from_slice(PNG_IEND);
    assert_eq!(min_valid.len(), 45);
    assert!(SkinPng::validate(min_valid.clone()).is_ok());
    assert_eq!(
        SkinPng::validate(min_valid[..44].to_vec()).unwrap_err(),
        ProfileServiceError::InvalidSkin(SkinProblem::NotPng)
    );
    // missing or damaged IEND trailer is rejected
    let mut no_iend = png(64, 64);
    *no_iend.last_mut().unwrap() ^= 0xff;
    assert_eq!(
        SkinPng::validate(no_iend).unwrap_err(),
        ProfileServiceError::InvalidSkin(SkinProblem::NotPng)
    );
    // bad IHDR metadata fields (invalid bit depth/color type, compression, filter, interlace)
    for (idx, val) in [
        (24usize, 3u8), // invalid bit depth
        (24, 16),       // 16-bit indexed color (color_type=3) is invalid
        (25, 1),        // invalid color type
        (26, 1),        // non-zero compression
        (27, 1),        // non-zero filter
        (28, 2),        // invalid interlace (> 1)
    ] {
        let mut corrupt = png(64, 64);
        if idx == 24 && val == 16 {
            corrupt[25] = 3; // indexed color cannot have 16-bit depth
        }
        corrupt[idx] = val;
        assert_eq!(
            SkinPng::validate(corrupt).unwrap_err(),
            ProfileServiceError::InvalidSkin(SkinProblem::NotPng),
            "field {idx}={val}"
        );
    }
    // the largest allowed size passes the size check (with IEND preserved at the end)
    let mut max = png(64, 64);
    max.truncate(max.len() - PNG_IEND.len());
    max.resize(MAX_SKIN_FILE_BYTES - PNG_IEND.len(), 0);
    max.extend_from_slice(PNG_IEND);
    assert_eq!(max.len(), MAX_SKIN_FILE_BYTES);
    assert!(SkinPng::validate(max).is_ok());
}

#[test]
fn the_form_names_the_model_in_the_services_spelling_and_carries_the_png_untouched() {
    let skin = SkinPng::validate(png(64, 64)).unwrap();
    for (variant, word) in [
        (SkinVariant::Classic, "classic"),
        (SkinVariant::Slim, "slim"),
    ] {
        let (body, content_type) = skin_form(variant, &skin).unwrap();
        let boundary = content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        assert!(boundary.starts_with("clade-") && boundary.len() == 6 + 32);
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains(&format!("name=\"variant\"\r\n\r\n{word}\r\n--{boundary}")));
        assert!(text
            .contains("name=\"file\"; filename=\"skin.png\"\r\nContent-Type: image/png\r\n\r\n"));
        assert!(text.ends_with(&format!("\r\n--{boundary}--\r\n")));
        assert!(body.windows(skin.bytes().len()).any(|w| w == skin.bytes()));
    }
    // two forms never share a boundary
    let a = skin_form(SkinVariant::Slim, &skin).unwrap().1;
    let b = skin_form(SkinVariant::Slim, &skin).unwrap().1;
    assert_ne!(a, b);
}

#[test]
fn variants_use_the_services_spelling() {
    assert_eq!(SkinVariant::Classic.form_value(), "classic");
    assert_eq!(SkinVariant::Slim.form_value(), "slim");
}

#[test]
fn every_official_endpoint_is_https_on_the_minecraft_services_host_and_allowed() {
    let policy = DownloadPolicy::new(DEFAULT_ALLOWED_HOSTS);
    for path in [PROFILE_PATH, SKINS_PATH, ACTIVE_SKIN_PATH, ACTIVE_CAPE_PATH] {
        let url = Url::parse(&format!("https://{SERVICE_HOST}{path}")).unwrap();
        assert!(policy.check(&url).is_ok(), "{path}");
    }
    assert_eq!(SERVICE_HOST, "api.minecraftservices.com");
}

// -------------------------------------------------------------------------------- the service

#[tokio::test]
async fn reading_the_profile_is_one_authenticated_get_to_the_fixed_host() {
    let env = Env::new();
    env.t.reply(PROFILE, 200, profile_json(RAW_A, &slim(), ""));
    let p = env.service().profile("a").await.unwrap();
    assert_eq!(p.skin_variant, Some(SkinVariant::Slim));
    let calls = env.t.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!((calls[0].0, calls[0].1.as_str()), (Method::Get, PROFILE));
    let auth = calls[0].2.iter().find(|h| h.0 == "Authorization").unwrap();
    assert_eq!(auth.1, format!("Bearer {TOKEN}"));
}

#[tokio::test]
async fn a_skin_upload_is_one_multipart_post_with_the_chosen_model() {
    let env = Env::new();
    env.t.reply(SKINS, 200, profile_json(RAW_A, &slim(), ""));
    let skin = SkinPng::validate(png(64, 64)).unwrap();
    let p = env
        .service()
        .upload_skin("a", SkinVariant::Slim, skin)
        .await
        .unwrap();
    assert_eq!(p.skin_variant, Some(SkinVariant::Slim));
    let calls = env.t.calls();
    assert_eq!(
        calls.len(),
        1,
        "no extra read when the answer is the profile"
    );
    assert_eq!((calls[0].0, calls[0].1.as_str()), (Method::Post, SKINS));
    let ct = &calls[0].2.iter().find(|h| h.0 == "Content-Type").unwrap().1;
    assert!(ct.starts_with("multipart/form-data; boundary=clade-"));
    assert!(calls[0].3.contains("name=\"variant\"\r\n\r\nslim\r\n"));
}

#[tokio::test]
async fn a_bad_skin_never_reaches_the_network() {
    // validation happens before a request exists; the service takes only a validated SkinPng
    let env = Env::new();
    assert!(SkinPng::validate(png(100, 100)).is_err());
    assert!(env.t.calls().is_empty());
}

#[tokio::test]
async fn resetting_the_skin_is_one_delete_without_a_body() {
    let env = Env::new();
    env.t
        .reply(SKIN_ACTIVE, 200, profile_json(RAW_A, &classic(), ""));
    env.service().reset_skin("a").await.unwrap();
    let calls = env.t.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        (calls[0].0, calls[0].1.as_str()),
        (Method::Delete, SKIN_ACTIVE)
    );
    assert_eq!(calls[0].3, "");
}

#[tokio::test]
async fn a_bare_success_is_followed_by_one_read_never_a_second_change() {
    let env = Env::new();
    env.t.reply(SKIN_ACTIVE, 204, "");
    env.t.reply(PROFILE, 200, profile_json(RAW_A, "", ""));
    let p = env.service().reset_skin("a").await.unwrap();
    assert_eq!(p.skin_variant, None);
    assert_eq!(
        env.calls(),
        vec![
            (Method::Delete, SKIN_ACTIVE.to_owned()),
            (Method::Get, PROFILE.to_owned())
        ]
    );
}

#[tokio::test]
async fn equipping_a_cape_checks_the_accounts_own_list_then_puts_its_id() {
    let env = Env::new();
    let capes = format!(
        r#"{{"id":"{CAPE_1}","state":"INACTIVE","alias":"A"}},{{"id":"{CAPE_2}","state":"INACTIVE","alias":"B"}}"#
    );
    env.t
        .reply(PROFILE, 200, profile_json(RAW_A, &classic(), &capes));
    let after = format!(r#"{{"id":"{CAPE_2}","state":"ACTIVE","alias":"B"}}"#);
    env.t
        .reply(CAPE_ACTIVE, 200, profile_json(RAW_A, &classic(), &after));
    let p = env
        .service()
        .set_cape("a", Some(CapeId::new(CAPE_2).unwrap()))
        .await
        .unwrap();
    assert!(p.capes[0].active);
    let calls = env.t.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!((calls[0].0, calls[0].1.as_str()), (Method::Get, PROFILE));
    assert_eq!(
        (calls[1].0, calls[1].1.as_str()),
        (Method::Put, CAPE_ACTIVE)
    );
    assert_eq!(calls[1].3, format!(r#"{{"capeId":"{CAPE_2}"}}"#));
    let ct = &calls[1].2.iter().find(|h| h.0 == "Content-Type").unwrap().1;
    assert_eq!(ct, "application/json");
}

#[tokio::test]
async fn a_cape_the_account_does_not_own_is_refused_without_sending_anything() {
    let env = Env::new();
    let capes = format!(r#"{{"id":"{CAPE_1}","state":"INACTIVE"}}"#);
    env.t
        .reply(PROFILE, 200, profile_json(RAW_A, &classic(), &capes));
    let e = env
        .service()
        .set_cape("a", Some(CapeId::new(CAPE_2).unwrap()))
        .await
        .unwrap_err();
    assert_eq!(e, ProfileServiceError::CapeNotOwned);
    assert_eq!(
        env.calls(),
        vec![(Method::Get, PROFILE.to_owned())],
        "no PUT"
    );
}

#[tokio::test]
async fn the_server_saying_the_cape_is_not_owned_becomes_a_safe_error() {
    let env = Env::new();
    let capes = format!(r#"{{"id":"{CAPE_1}","state":"INACTIVE"}}"#);
    env.t
        .reply(PROFILE, 200, profile_json(RAW_A, &classic(), &capes));
    env.t.reply(
        CAPE_ACTIVE,
        400,
        r#"{"errorMessage":"Profile does not own cape"}"#,
    );
    let e = env
        .service()
        .set_cape("a", Some(CapeId::new(CAPE_1).unwrap()))
        .await
        .unwrap_err();
    assert_eq!(e, ProfileServiceError::CapeNotOwned);
    assert!(
        !format!("{e:?} {e}").contains("errorMessage"),
        "the body is never exposed"
    );
}

#[tokio::test]
async fn hiding_the_cape_is_one_delete_and_needs_no_cape_id() {
    let env = Env::new();
    env.t
        .reply(CAPE_ACTIVE, 200, profile_json(RAW_A, &classic(), ""));
    env.service().set_cape("a", None).await.unwrap();
    assert_eq!(env.calls(), vec![(Method::Delete, CAPE_ACTIVE.to_owned())]);
    assert_eq!(env.t.calls()[0].3, "");
}

fn script_refresh(t: &FakeTransport, fresh_mc_token: &str, raw_uuid: &str) {
    t.reply(
        ENDPOINTS[1],
        200,
        r#"{"access_token":"MS-ACCESS","refresh_token":"REFRESH-2","expires_in":3600}"#,
    );
    t.reply(
        ENDPOINTS[2],
        200,
        r#"{"Token":"XBL","DisplayClaims":{"xui":[{"uhs":"99"}]}}"#,
    );
    t.reply(
        ENDPOINTS[3],
        200,
        r#"{"Token":"XSTS","DisplayClaims":{"xui":[{"uhs":"99"}]}}"#,
    );
    t.reply(
        ENDPOINTS[4],
        200,
        format!(r#"{{"access_token":"{fresh_mc_token}","expires_in":86400}}"#),
    );
    t.reply(
        ENDPOINTS[6],
        200,
        format!(r#"{{"id":"{raw_uuid}","name":"Alex_1"}}"#),
    );
}

#[tokio::test]
async fn failures_are_mapped_to_stable_codes_and_changes_are_never_retried() {
    for (status, code) in [
        (403, "forbidden"),
        (404, "no_profile"),
        (429, "rate_limited"),
        (500, "service_unavailable"),
        (503, "service_unavailable"),
        (418, "bad_response"),
    ] {
        let env = Env::new();
        env.t.reply(SKINS, status, r#"{"error":"secret detail"}"#);
        let skin = SkinPng::validate(png(64, 64)).unwrap();
        let e = env
            .service()
            .upload_skin("a", SkinVariant::Classic, skin)
            .await
            .unwrap_err();
        assert_eq!(e.code(), code, "status {status}");
        assert_eq!(
            env.t.calls().len(),
            1,
            "status {status} must not be retried"
        );
        assert!(!format!("{e:?} {e}").contains("secret detail"));
    }
    // a 400 to a skin upload means the image was refused
    let env = Env::new();
    env.t.reply(SKINS, 400, "{}");
    let skin = SkinPng::validate(png(64, 64)).unwrap();
    let e = env
        .service()
        .upload_skin("a", SkinVariant::Slim, skin)
        .await
        .unwrap_err();
    assert_eq!(e.code(), "invalid_skin");
    assert_eq!(env.t.calls().len(), 1);
}

#[tokio::test]
async fn a_rejected_cached_token_forces_one_refresh_and_retries_once_with_the_fresh_token() {
    // 1. Read (`profile`) gets 401 on cached token, refreshes once, succeeds with fresh token
    let env = Env::new();
    env.t.reply(PROFILE, 401, r#"{"error":"expired"}"#);
    script_refresh(&env.t, "FRESH-MC-TOKEN", RAW_A);
    env.t.reply(PROFILE, 200, profile_json(RAW_A, &slim(), ""));
    let p = env.service().profile("a").await.unwrap();
    assert_eq!(p.skin_variant, Some(SkinVariant::Slim));
    let profile_calls: Vec<_> = env
        .t
        .calls()
        .into_iter()
        .filter(|(_, u, _, _)| u == PROFILE)
        .collect();
    // 1st: initial 401 with cached TOKEN; 2nd: verify_java_profile inside ensure_fresh_forced; 3rd: retried read
    assert_eq!(profile_calls.len(), 3);
    assert!(profile_calls[0]
        .2
        .iter()
        .any(|(k, v)| k == "Authorization" && v == &format!("Bearer {TOKEN}")));
    assert!(profile_calls[2]
        .2
        .iter()
        .any(|(k, v)| k == "Authorization" && v == "Bearer FRESH-MC-TOKEN"));

    // 2. Change (`upload_skin`) gets 401 on cached token, refreshes once, succeeds with fresh token
    let env = Env::new();
    env.t.reply(SKINS, 401, r#"{"error":"expired"}"#);
    script_refresh(&env.t, "FRESH-SKIN-TOKEN", RAW_A);
    env.t.reply(SKINS, 200, profile_json(RAW_A, &slim(), ""));
    let skin = SkinPng::validate(png(64, 64)).unwrap();
    let p = env
        .service()
        .upload_skin("a", SkinVariant::Slim, skin)
        .await
        .unwrap();
    assert_eq!(p.skin_variant, Some(SkinVariant::Slim));
    let skin_calls: Vec<_> = env
        .t
        .calls()
        .into_iter()
        .filter(|(_, u, _, _)| u == SKINS)
        .collect();
    assert_eq!(skin_calls.len(), 2);
    assert!(skin_calls[1]
        .2
        .iter()
        .any(|(k, v)| k == "Authorization" && v == "Bearer FRESH-SKIN-TOKEN"));

    // 3. When the fresh token ALSO gets 401, `Unauthorized` is returned and no second refresh runs
    let env = Env::new();
    env.t.reply(SKINS, 401, r#"{"error":"secret detail"}"#);
    script_refresh(&env.t, "FRESH-STILL-401", RAW_A);
    env.t.reply(SKINS, 401, r#"{"error":"secret detail"}"#);
    let skin = SkinPng::validate(png(64, 64)).unwrap();
    let e = env
        .service()
        .upload_skin("a", SkinVariant::Classic, skin)
        .await
        .unwrap_err();
    assert_eq!(e, ProfileServiceError::Unauthorized);
    assert_eq!(e.code(), "unauthorized");
    assert!(!format!("{e:?} {e}").contains("secret detail"));
    assert_eq!(
        env.t
            .calls()
            .iter()
            .filter(|(_, u, _, _)| u == SKINS)
            .count(),
        2,
        "at most one retry after 401"
    );
    assert_eq!(
        env.t
            .calls()
            .iter()
            .filter(|(_, u, _, _)| u == ENDPOINTS[1])
            .count(),
        1,
        "at most one refresh"
    );
}

#[tokio::test]
async fn a_non_empty_corrupt_or_foreign_200_on_change_fails_without_falling_back_to_read() {
    // 200 with corrupt body -> BadResponse, no fallback GET /minecraft/profile
    let env = Env::new();
    env.t.reply(SKIN_ACTIVE, 200, "<html>corrupt</html>");
    env.t.reply(PROFILE, 200, profile_json(RAW_A, "", ""));
    let e = env.service().reset_skin("a").await.unwrap_err();
    assert!(matches!(e, ProfileServiceError::BadResponse(_)));
    assert_eq!(env.calls(), vec![(Method::Delete, SKIN_ACTIVE.to_owned())]);

    // 200 with another account's UUID -> BadResponse, no fallback GET /minecraft/profile
    let env = Env::new();
    env.t.reply(
        SKIN_ACTIVE,
        200,
        profile_json("0000000000004000800000000000b002", "", ""),
    );
    env.t.reply(PROFILE, 200, profile_json(RAW_A, "", ""));
    let e = env.service().reset_skin("a").await.unwrap_err();
    assert_eq!(
        e,
        ProfileServiceError::BadResponse("another profile answered")
    );
    assert_eq!(env.calls(), vec![(Method::Delete, SKIN_ACTIVE.to_owned())]);

    // 200 with whitespace-only body -> treated as bare success and reads current profile once
    let env = Env::new();
    env.t.reply(SKIN_ACTIVE, 200, "  \r\n ");
    env.t.reply(PROFILE, 200, profile_json(RAW_A, "", ""));
    let p = env.service().reset_skin("a").await.unwrap();
    assert_eq!(p.skin_variant, None);
    assert_eq!(
        env.calls(),
        vec![
            (Method::Delete, SKIN_ACTIVE.to_owned()),
            (Method::Get, PROFILE.to_owned())
        ]
    );
}

#[test]
fn auth_errors_map_no_java_profile_and_unknown_account_accurately() {
    assert_eq!(
        auth_error(&AuthError::NoJavaProfile {
            owns_minecraft: false
        }),
        ProfileServiceError::NoProfile
    );
    assert_eq!(
        auth_error(&AuthError::NoJavaProfile {
            owns_minecraft: true
        }),
        ProfileServiceError::NoProfile
    );
    assert_eq!(
        auth_error(&AuthError::UnknownAccount),
        ProfileServiceError::UnknownAccount
    );
}

#[tokio::test]
async fn no_connection_is_offline_and_a_malformed_answer_is_reported_as_such() {
    let env = Env::new();
    env.t.offline(PROFILE);
    assert_eq!(
        env.service().profile("a").await.unwrap_err(),
        ProfileServiceError::Offline
    );

    let env = Env::new();
    env.t.reply(PROFILE, 200, "<html>captive portal</html>");
    assert!(matches!(
        env.service().profile("a").await.unwrap_err(),
        ProfileServiceError::BadResponse(_)
    ));
}

#[tokio::test]
async fn another_accounts_profile_is_never_accepted() {
    let env = Env::new();
    env.t.reply(
        PROFILE,
        200,
        profile_json("0000000000004000800000000000b002", &classic(), ""),
    );
    assert!(matches!(
        env.service().profile("a").await.unwrap_err(),
        ProfileServiceError::BadResponse(_)
    ));
}

#[tokio::test]
async fn the_exact_account_is_used_and_no_other_ready_account_is_substituted() {
    // unknown id
    let env = Env::new();
    assert_eq!(
        env.service().profile("nobody").await.unwrap_err(),
        ProfileServiceError::UnknownAccount
    );
    // an account that must sign in again is refused although another account is ready
    let env = Env::new();
    env.mark_reauth("a");
    assert_eq!(
        env.service().reset_skin("a").await.unwrap_err(),
        ProfileServiceError::NeedsSignIn
    );
    assert!(
        env.t.calls().is_empty(),
        "no request, and the other account's token was not used"
    );
    // the other account's token is used only for the other account
    let env = Env::new();
    env.t.reply(
        PROFILE,
        200,
        profile_json("0000000000004000800000000000b002", "", ""),
    );
    env.service().profile("b").await.unwrap();
    let auth = env.t.calls()[0]
        .2
        .iter()
        .find(|h| h.0 == "Authorization")
        .unwrap()
        .1
        .clone();
    assert_eq!(auth, "Bearer FAKE-TOKEN-OF-THE-OTHER-ACCOUNT");
}

#[tokio::test]
async fn a_locked_vault_gives_the_unlock_error_and_sends_nothing() {
    let env = Env::new();
    *env.vault.lock().unwrap() = None;
    assert_eq!(
        env.service().profile("a").await.unwrap_err(),
        ProfileServiceError::VaultLocked
    );
    assert!(env.t.calls().is_empty());
}

#[tokio::test]
async fn the_token_is_only_ever_sent_to_the_minecraft_services_host() {
    let env = Env::new();
    env.t.reply(PROFILE, 200, profile_json(RAW_A, "", ""));
    env.t.reply(SKIN_ACTIVE, 200, profile_json(RAW_A, "", ""));
    env.service().profile("a").await.unwrap();
    env.service().reset_skin("a").await.unwrap();
    for (_, url, headers, _) in env.t.calls() {
        assert!(
            url.starts_with("https://api.minecraftservices.com/minecraft/profile"),
            "{url}"
        );
        assert!(headers.iter().any(|h| h.0 == "Authorization"));
    }
}

#[tokio::test]
async fn no_error_can_print_the_token() {
    let all = [
        ProfileServiceError::UnknownAccount,
        ProfileServiceError::NeedsSignIn,
        ProfileServiceError::VaultLocked,
        ProfileServiceError::InvalidSkin(SkinProblem::WrongSize),
        ProfileServiceError::Unauthorized,
        ProfileServiceError::Forbidden,
        ProfileServiceError::CapeNotOwned,
        ProfileServiceError::RateLimited,
        ProfileServiceError::Offline,
        ProfileServiceError::ServiceUnavailable,
        ProfileServiceError::NoProfile,
        ProfileServiceError::BadResponse("x"),
        ProfileServiceError::Storage,
        ProfileServiceError::Busy,
    ];
    for e in all {
        let text = format!("{e:?} {e} {} {:?}", e.code(), e.coded());
        assert!(!text.contains("FAKE-") && !text.to_lowercase().contains("bearer"));
    }
    // and a failed request leaves no trace of it either
    let env = Env::new();
    env.t.reply(PROFILE, 401, TOKEN);
    let e = env.service().profile("a").await.unwrap_err();
    assert!(!format!("{e:?} {e}").contains(TOKEN));
    let _ = HttpRequest {
        method: Method::Get,
        url: Url::parse(PROFILE).unwrap(),
        headers: vec![("Authorization", format!("Bearer {TOKEN}"))],
        body: Vec::new(),
        max_response_bytes: 1,
    };
}

#[test]
fn a_request_never_prints_its_authorization_header() {
    let r = HttpRequest {
        method: Method::Put,
        url: Url::parse(CAPE_ACTIVE).unwrap(),
        headers: vec![("Authorization", format!("Bearer {TOKEN}"))],
        body: b"{}".to_vec(),
        max_response_bytes: 1,
    };
    assert!(!format!("{r:?}").contains(TOKEN));
}

#[test]
fn only_one_change_can_be_in_flight() {
    let env = Env::new();
    let first = env.accounts.begin_appearance_change();
    assert!(first.is_some());
    assert!(env.accounts.begin_appearance_change().is_none());
    drop(first);
    assert!(env.accounts.begin_appearance_change().is_some());
}

#[test]
fn the_wire_format_for_the_interface_has_no_urls_and_no_tokens() {
    let capes = format!(
        r#"{{"id":"{CAPE_1}","state":"ACTIVE","url":"http://textures.minecraft.net/texture/aa","alias":"Migrator"}}"#
    );
    let p = parse_profile(profile_json(RAW_A, &slim(), &capes).as_bytes()).unwrap();
    let json =
        serde_json::to_string(&crate::appearance_cmd::AppearanceDto::from_profile(&p)).unwrap();
    assert_eq!(
        json,
        format!(
            r#"{{"skinVariant":"slim","capes":[{{"id":"{CAPE_1}","alias":"Migrator","active":true}}]}}"#
        )
    );
    assert!(!json.contains("textures.minecraft.net") && !json.contains("http"));
}
