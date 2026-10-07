pub mod accounts;
pub mod accounts_api;
pub mod appearance_cmd;
pub mod auth;
pub mod coded_error;
pub mod diagnostics;
pub mod download;
pub mod download_policy;
pub mod game_session;
pub mod glass;
pub mod instances;
pub mod java_install;
pub mod launch_args;
pub mod launch_cmd;
pub mod loader_install;
pub mod loader_options;
pub mod loader_prepare;
pub mod loaders;
pub mod mc_catalog;
pub mod mc_meta;
pub mod ms_auth;
pub mod natives;
pub mod net;
pub mod playtime;
pub mod portable;
pub mod prepare;
pub mod prepare_cmd;
pub mod profile_lock;
pub mod profile_service;
pub mod profiles;
pub mod runtimes;
pub mod safe_archive;
pub mod settings;
pub mod skin;
pub mod startup;
pub mod startup_text;
pub mod support_cmd;
pub mod system;
pub mod ui_settings;
pub mod vault;
pub mod vault_recovery;
pub mod versions;
pub mod webview;
pub mod window_fit;

use portable::root::LAYOUT_VERSION;
use portable::{PortableRoot, RootLock};
use serde::Serialize;
use startup_text::StartupMessage;
use tauri::Manager;

/// Held for the life of the process. The lock is released when the app exits.
pub struct AppState {
    pub(crate) root: PortableRoot,
    _lock: RootLock,
    pub(crate) webview: webview::RuntimeSource,
    pub(crate) webview_version: String,
}

#[derive(Serialize)]
struct PortableStatus {
    layout_version: u32,
    webview: webview::RuntimeSource,
    webview_version: String,
    vault_present: bool,
    account_count: usize,
}

/// Non-sensitive diagnostics. Deliberately returns no absolute paths.
#[tauri::command]
fn portable_status(state: tauri::State<'_, AppState>) -> PortableStatus {
    PortableStatus {
        layout_version: LAYOUT_VERSION,
        webview: state.webview,
        webview_version: state.webview_version.clone(),
        vault_present: vault::Vault::exists(&state.root.vault_file()),
        account_count: accounts::AccountsFile::load(&state.root.accounts_file())
            .map(|a| a.accounts.len())
            .unwrap_or(0),
    }
}

pub fn run() {
    let root = PortableRoot::discover().unwrap_or_else(|e| {
        startup::fail(
            StartupMessage::CannotLocateFolder,
            None,
            Some(&e.to_string()),
        )
    });
    startup::preview_requested(root.path());

    // Before any WebView exists: choose the bundled runtime if present.
    let webview_source = webview::select_runtime(&root);

    root.ensure_layout().unwrap_or_else(|e| {
        let id = match e {
            portable::root::RootError::NotWritable(_) => StartupMessage::FolderNotWritable,
            _ => StartupMessage::StartupFailed,
        };
        startup::fail(id, Some(root.path()), Some(&e.to_string()))
    });
    // Only one Clade may use the folder. If it is Clade on this PC (possibly with its window hidden
    // because it was closed during a game), bring that one back and quietly stop; otherwise the
    // folder is in use elsewhere and that is reported.
    let lock = match RootLock::acquire(&root) {
        Ok(l) => l,
        Err(portable::root::RootError::AlreadyRunning)
            if startup::wake_running_instance("Clade Launcher") =>
        {
            std::process::exit(0)
        }
        Err(e) => {
            let id = match e {
                portable::root::RootError::AlreadyRunning => StartupMessage::FolderInUse,
                _ => StartupMessage::StartupFailed,
            };
            startup::fail(id, Some(root.path()), Some(&e.to_string()))
        }
    };
    startup::init_logging(&root);

    let launcher_settings = settings::LauncherSettings::load_or_recover(&root.settings_file())
        .unwrap_or_else(|e| {
            let id = match e {
                settings::SettingsError::FolderTooNew { .. } => StartupMessage::FolderTooNew,
                _ => StartupMessage::StartupFailed,
            };
            startup::fail(id, Some(root.path()), Some(&e.to_string()))
        });
    // Scratch space a crashed preparation may have left (verified files and partial downloads stay).
    if let Err(e) = prepare_cmd::sweep_scratch(&root) {
        tracing::warn!(error = %e, "could not clean scratch space");
    }
    // Console-output files of games that ended while Clade was not watching (a game that is still
    // running keeps writing into its unlinked file until it ends).
    game_session::sweep_output(&root);
    // Remove loader-install staging trees left behind by a crash.
    if let Err(e) = loader_install::sweep_stale(&root) {
        tracing::warn!(error = %e, "could not clean stale loader staging");
    }
    // A profile whose instance.json cannot be read is hidden from the list but never touched; say so
    // in the log (and in the diagnostics) instead of letting it vanish without a trace.
    if let Ok(listing) = instances::InstanceStore::new(&root).list() {
        for (folder, reason) in &listing.problems {
            tracing::warn!(profile_folder = %folder, %reason, "a profile could not be read; its files were left untouched");
        }
    }
    // Bank any game session that was cut short by a crash or power loss last time.
    match playtime::PlaytimeStore::new(&root).recover() {
        Ok(sessions) if !sessions.is_empty() => {
            tracing::info!(
                count = sessions.len(),
                "recovered unfinished playtime sessions"
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(error = %e, "playtime recovery failed"),
    }
    let webview_version = webview::runtime_version().unwrap_or_else(|e| {
        tracing::error!(error = %e, "no usable WebView2 runtime");
        startup::fail(StartupMessage::WebViewMissing, Some(root.path()), None)
    });
    tracing::info!(layout = LAYOUT_VERSION, webview = ?webview_source, %webview_version, "launcher starting");

    // Networking lives in Rust only. Two clients: one for Mojang and the global allow-list, one
    // restricted to the loaders' own hosts. A failure to build one only disables discovery.
    let profiles_state = profiles::ProfilesState {
        root: root.clone(),
        global: net::HttpFetcher::new(download_policy::DownloadPolicy::new(
            download_policy::DEFAULT_ALLOWED_HOSTS,
        ))
        .map_err(|e| e.to_string()),
        loaders: net::HttpFetcher::new(loaders::official_policy()).map_err(|e| e.to_string()),
    };
    let accounts_state = accounts_api::AccountsState::new(root.accounts_file()).with_skins(
        skin::SkinCache::new(root.cache_dir().join("skins")),
        net::HttpFetcher::new(download_policy::DownloadPolicy::new(
            download_policy::DEFAULT_ALLOWED_HOSTS,
        ))
        .map_err(|e| e.to_string()),
    );
    let auth_state = auth::AuthState::new(
        root.vault_file(),
        net::HttpFetcher::for_auth(download_policy::DownloadPolicy::new(
            download_policy::DEFAULT_ALLOWED_HOSTS,
        ))
        .map_err(|e| e.to_string()),
    );
    // One lock registry for both: launching and preparing the same profile exclude each other.
    let profile_locks = std::sync::Arc::new(profile_lock::ProfileLocks::default());
    let prepare_state = prepare_cmd::PrepareState::new(
        root.clone(),
        net::HttpFetcher::new(download_policy::DownloadPolicy::new(
            download_policy::DEFAULT_ALLOWED_HOSTS,
        ))
        .map_err(|e| e.to_string()),
        net::HttpFetcher::new(loaders::official_policy()).map_err(|e| e.to_string()),
        net::HttpFetcher::for_downloads(download_policy::DownloadPolicy::new(
            download_policy::DEFAULT_ALLOWED_HOSTS,
        ))
        .map_err(|e| e.to_string()),
        net::HttpFetcher::for_downloads(loaders::official_policy()).map_err(|e| e.to_string()),
    )
    .with_locks(profile_locks.clone());
    let launch_state = launch_cmd::LaunchState::new(root.clone()).with_locks(profile_locks);
    let settings_state = ui_settings::SettingsState::new(root.settings_file(), launcher_settings);
    let webview_dir = root.webview_data_dir();
    let state = AppState {
        root,
        _lock: lock,
        webview: webview_source,
        webview_version,
    };

    let root_for_errors = state.root.path().to_path_buf();
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Starting Clade again brings it back (also when its window was closed while a game ran).
            if let Some(w) = app.get_webview_window("main") {
                app.state::<launch_cmd::LaunchState>()
                    .set_exit_when_idle(false);
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .on_window_event(|window, event| {
            // Closing the window while Minecraft runs must not end the game or lose its playtime:
            // hide the window and keep Clade alive (no tray, no service) until the game exits.
            // A window that is on screen again (someone started Clade once more) is not "closed".
            if let tauri::WindowEvent::Focused(true) = event {
                window
                    .app_handle()
                    .state::<launch_cmd::LaunchState>()
                    .set_exit_when_idle(false);
                // The Windows transparency or High Contrast preference may have changed meanwhile.
                glass::refresh(window.app_handle());
            }
            // Clade is a windowed launcher: if the window ever ends up maximized or fullscreen
            // (a shortcut, a snap gesture), put it back to a normal window.
            if let tauri::WindowEvent::Resized(_) = event {
                if window.is_fullscreen().unwrap_or(false) {
                    let _ = window.set_fullscreen(false);
                }
                if window.is_maximized().unwrap_or(false) {
                    let _ = window.unmaximize();
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let launch = window.app_handle().state::<launch_cmd::LaunchState>();
                if launch.has_live_game() {
                    api.prevent_close();
                    launch.set_exit_when_idle(true);
                    let _ = window.hide();
                }
            }
        })
        .plugin(tauri_plugin_opener::init())
        .manage(state)
        .manage(settings_state)
        .manage(glass::GlassState::new())
        .manage(profiles_state)
        .manage(accounts_state)
        .manage(auth_state)
        .manage(prepare_state)
        .manage(launch_state)
        .invoke_handler(tauri::generate_handler![
            portable_status,
            ui_settings::get_ui_settings,
            ui_settings::set_sound_settings,
            ui_settings::set_last_phrase,
            ui_settings::set_language,
            ui_settings::set_reduced_motion,
            ui_settings::set_transparency,
            glass::get_glass_status,
            ui_settings::set_memory,
            ui_settings::set_behavior,
            ui_settings::complete_onboarding,
            ui_settings::reset_settings,
            support_cmd::open_clade_folder,
            support_cmd::open_logs_folder,
            support_cmd::export_diagnostics,
            profiles::list_profiles,
            profiles::select_profile,
            profiles::get_version_catalog,
            profiles::get_loader_options,
            profiles::create_profile,
            accounts_api::list_accounts,
            accounts_api::select_account,
            accounts_api::remove_account,
            appearance_cmd::account_appearance,
            appearance_cmd::set_account_skin,
            appearance_cmd::reset_account_skin,
            appearance_cmd::set_account_cape,
            auth::vault_status,
            auth::create_vault,
            auth::unlock_vault,
            vault_recovery::reset_vault,
            auth::begin_sign_in,
            auth::cancel_sign_in,
            auth::refresh_account,
            prepare_cmd::prepare_profile,
            prepare_cmd::remove_profile,
            prepare_cmd::cancel_prepare,
            prepare_cmd::prepare_status,
            launch_cmd::launch_profile,
            launch_cmd::launch_status,
            launch_cmd::open_game_logs
        ])
        .setup(move |app| {
            // The window is created here, not from tauri.conf.json, because only the builder
            // accepts an absolute WebView2 profile directory (the config option is always
            // resolved under %LOCALAPPDATA%).
            let cfg = app
                .config()
                .app
                .windows
                .first()
                .cloned()
                .ok_or("tauri.conf.json defines no window")?;
            let mut builder = tauri::WebviewWindowBuilder::from_config(app.handle(), &cfg)?
                .data_directory(webview_dir)
                .maximizable(false)
                .fullscreen(false);
            // Size and place the window inside the usable area (taskbar excluded) of the monitor
            // in use; the compact default stays whenever it fits.
            if let Some(m) = active_monitor(app.handle()) {
                let wa = m.work_area();
                let scale = m.scale_factor();
                let p = window_fit::fit(window_fit::WorkArea {
                    x: wa.position.x,
                    y: wa.position.y,
                    width: wa.size.width,
                    height: wa.size.height,
                    scale,
                });
                builder = builder
                    .inner_size(p.width, p.height)
                    .min_inner_size(p.min_width, p.min_height)
                    .position(p.x / scale, p.y / scale);
            }
            builder.build()?;
            // Cosmetic only: a failure here is logged and Clade simply stays opaque.
            glass::refresh(app.handle());
            glass::watch_system_preference(app.handle().clone());
            Ok(())
        })
        .run(tauri::generate_context!());
    if let Err(e) = result {
        tracing::error!(error = %e, "the application failed to run");
        startup::fail(StartupMessage::WindowFailed, Some(&root_for_errors), None);
    }
}

/// The monitor in use: the one under the mouse pointer, else the primary one.
fn active_monitor(app: &tauri::AppHandle) -> Option<tauri::Monitor> {
    app.cursor_position()
        .ok()
        .and_then(|c| app.monitor_from_point(c.x, c.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())
}
