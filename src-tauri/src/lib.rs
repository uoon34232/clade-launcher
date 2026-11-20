pub mod accounts;
pub mod accounts_api;
pub mod appearance_cmd;
pub mod auth;
pub mod coded_error;
pub mod diagnostics;
pub mod download;
pub mod download_policy;
pub mod game_embed;
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

    root.ensure_data_dir().unwrap_or_else(|e| {
        let id = match e {
            portable::root::RootError::NotWritable(_) => StartupMessage::FolderNotWritable,
            _ => StartupMessage::StartupFailed,
        };
        startup::fail(id, Some(root.path()), Some(&e.to_string()))
    });
    // Only one Clade may use the folder. Acquire the lock before touching shared layout files. If
    // another Clade on this PC owns this folder (possibly with its window hidden while a game
    // runs), wake that one and quietly stop; otherwise the folder is in use elsewhere.
    let lock = match RootLock::acquire(&root) {
        Ok(l) => l,
        Err(portable::root::RootError::AlreadyRunning)
            if startup::wake_running_instance(root.path()) =>
        {
            std::process::exit(0)
        }
        Err(e) => {
            let id = match e {
                portable::root::RootError::AlreadyRunning => StartupMessage::FolderInUse,
                portable::root::RootError::NotWritable(_) => StartupMessage::FolderNotWritable,
                _ => StartupMessage::StartupFailed,
            };
            startup::fail(id, Some(root.path()), Some(&e.to_string()))
        }
    };
    root.ensure_layout().unwrap_or_else(|e| {
        let id = match e {
            portable::root::RootError::NotWritable(_) => StartupMessage::FolderNotWritable,
            _ => StartupMessage::StartupFailed,
        };
        startup::fail(id, Some(root.path()), Some(&e.to_string()))
    });
    startup::init_logging(&root);

    let launcher_settings = settings::LauncherSettings::load_or_recover(&root.settings_file())
        .unwrap_or_else(|e| {
            let id = match e {
                settings::SettingsError::FolderTooNew { .. } => StartupMessage::FolderTooNew,
                _ => StartupMessage::StartupFailed,
            };
            startup::fail(id, Some(root.path()), Some(&e.to_string()))
        });

    // Before any WebView or thread pool exists: choose the bundled runtime if present and configure
    // Linux X11/NVIDIA WebKitGTK startup overrides while single-threaded.
    let webview_source =
        webview::select_runtime(&root, launcher_settings.behavior.embeds_on_launch());

    let workers = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2)
        .clamp(2, 4);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .max_blocking_threads(8)
        .enable_all()
        .build()
        .unwrap_or_else(|e| {
            startup::fail(
                StartupMessage::StartupFailed,
                Some(root.path()),
                Some(&e.to_string()),
            )
        });
    tauri::async_runtime::set(rt.handle().clone());
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
    // Reclaim disposable Chromium component-updater bloat (Widevine DRM, Speech, Subresource
    // Filter, metrics) before WebView2 opens its user-data directory.
    webview::sweep_profile_bloat(&root);
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

    // One lock registry shared across profile edits, preparation and launching.
    let profile_locks = std::sync::Arc::new(profile_lock::ProfileLocks::default());
    // Networking lives in Rust only. Two clients: one for Mojang and the global allow-list, one
    // restricted to the loaders' own hosts. A failure to build one only disables discovery.
    let profiles_state = profiles::ProfilesState::new(
        root.clone(),
        net::HttpFetcher::new(download_policy::DownloadPolicy::new(
            download_policy::DEFAULT_ALLOWED_HOSTS,
        ))
        .map_err(|e| e.to_string()),
        net::HttpFetcher::new(loaders::official_policy()).map_err(|e| e.to_string()),
    )
    .with_locks(profile_locks.clone());
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
    let wake_root = root_for_errors.clone();
    let result = tauri::Builder::default()
        .on_window_event(|window, event| match event {
            // Closing the window while Minecraft runs must not end the game or lose its playtime:
            // hide the window and keep Clade alive (no tray, no service) until the game exits.
            // A window that is on screen again (someone started Clade once more) is not "closed".
            tauri::WindowEvent::Focused(true) => {
                window
                    .app_handle()
                    .state::<launch_cmd::LaunchState>()
                    .set_exit_when_idle(false);
                webview::resume_window(window.app_handle());
                glass::resume(window.app_handle());
                game_embed::on_host_focused(window.app_handle());
            }
            tauri::WindowEvent::Focused(false) => {
                if window
                    .app_handle()
                    .state::<launch_cmd::LaunchState>()
                    .has_live_game()
                    && window.is_visible().unwrap_or(false)
                {
                    webview::throttle_unfocused_window(window.app_handle());
                }
            }
            // Clade is a windowed launcher: outside framed game mode, keep the menu window non-maximized
            // and non-fullscreen. While a game is framed inside Clade, allow resizing and maximizing.
            tauri::WindowEvent::Resized(size) if size.width > 0 && size.height > 0 => {
                if game_embed::is_active(window.app_handle()) {
                    game_embed::on_host_resized(window.app_handle());
                } else {
                    keep_windowed(window, size.width, size.height);
                }
            }
            tauri::WindowEvent::CloseRequested { api, .. } => {
                if game_embed::request_close_or_detach(window.app_handle()) {
                    api.prevent_close();
                    return;
                }
                let launch = window.app_handle().state::<launch_cmd::LaunchState>();
                if launch.close_requested() {
                    api.prevent_close();
                    let _ = window.hide();
                    glass::suspend(window.app_handle());
                    webview::suspend_hidden_window(window.app_handle());
                } else {
                    prepare_cmd::cancel_prepare(
                        window.app_handle().state::<prepare_cmd::PrepareState>(),
                    );
                }
            }
            _ => {}
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
        .manage(game_embed::EmbedState::new())
        .invoke_handler(tauri::generate_handler![
            portable_status,
            ui_settings::get_ui_settings,
            ui_settings::set_sound_settings,
            ui_settings::set_last_phrase,
            ui_settings::set_language,
            ui_settings::set_ui_mode,
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
            profiles::rename_profile,
            profiles::set_profile_memory,
            profiles::set_profile_version,
            profiles::duplicate_profile,
            profiles::set_profile_resolution,
            profiles::set_profile_jvm_args,
            profiles::open_profile_folder,
            profiles::open_instances_folder,
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
            launch_cmd::open_game_logs,
            game_embed::get_game_embed_status,
            game_embed::set_game_stage_bounds,
            game_embed::toggle_game_maximize
        ])
        .setup(move |app| {
            // The window is created here, not from tauri.conf.json, because only the builder
            // accepts an absolute WebView2 profile directory (the config option is always
            // resolved under %LOCALAPPDATA%).
            let placement = active_monitor(app.handle()).map(|m| {
                let wa = m.work_area();
                let scale = m.scale_factor();
                window_fit::fit(window_fit::WorkArea {
                    x: wa.position.x,
                    y: wa.position.y,
                    width: wa.size.width,
                    height: wa.size.height,
                    scale,
                })
            });
            let mut cfg = app
                .config()
                .app
                .windows
                .first()
                .cloned()
                .ok_or("tauri.conf.json defines no window")?;
            if placement.is_some() {
                cfg.center = false;
            }
            #[cfg(not(windows))]
            let show_after_build = {
                let show = cfg.visible;
                // Keep unrealized while tao attaches its Wayland CSD headerbar for decorations=false,
                // and request an RGBA visual so "Transparency effects" can toggle window alpha live.
                cfg.visible = false;
                cfg.transparent = true;
                cfg.additional_browser_args = None;
                show
            };
            let mut builder = tauri::WebviewWindowBuilder::from_config(app.handle(), &cfg)?
                .data_directory(webview_dir)
                .maximizable(false)
                .fullscreen(false);
            if let Some(p) = placement {
                builder = builder
                    .inner_size(p.width, p.height)
                    .min_inner_size(p.min_width, p.min_height);
            }
            let window = builder.build()?;
            if let Some(p) = placement {
                let _ = window.set_position(tauri::Position::Physical(
                    tauri::PhysicalPosition::new(p.x as i32, p.y as i32),
                ));
            }
            #[cfg(not(windows))]
            if show_after_build {
                let _ = window.show();
            }

            let app_handle = app.handle().clone();
            startup::spawn_wake_listener(&wake_root, move || {
                app_handle
                    .state::<launch_cmd::LaunchState>()
                    .set_exit_when_idle(false);
                webview::resume_window(&app_handle);
                if let Some(w) = app_handle.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
                glass::resume(&app_handle);
            });

            launch_cmd::reattach_live_sessions(app.handle());
            // Cosmetic only: a failure here is logged and Clade simply stays opaque.
            glass::watch_system_preference(app.handle().clone());
            glass::refresh(app.handle());
            Ok(())
        })
        .run(tauri::generate_context!());
    if let Err(e) = result {
        tracing::error!(error = %e, "the application failed to run");
        startup::fail(StartupMessage::WindowFailed, Some(&root_for_errors), None);
    }
}

fn keep_windowed(window: &tauri::Window, width: u32, height: u32) {
    #[cfg(windows)]
    {
        if let Ok(hwnd) = window.hwnd() {
            // SAFETY: `hwnd` is the live window handle owned by Tauri; `IsZoomed` is a lightweight
            // read-only check on the window style bits.
            if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed(hwnd.0.cast()) } != 0
            {
                let _ = window.unmaximize();
            }
        }
        if width >= 800 && height >= 500 && window.is_fullscreen().unwrap_or(false) {
            let _ = window.set_fullscreen(false);
        }
    }
    #[cfg(not(windows))]
    if width >= 800 && height >= 500 {
        if window.is_fullscreen().unwrap_or(false) {
            let _ = window.set_fullscreen(false);
        }
        if window.is_maximized().unwrap_or(false) {
            let _ = window.unmaximize();
        }
    }
}

/// The monitor in use: the one under the mouse pointer, else the primary one.
fn active_monitor(app: &tauri::AppHandle) -> Option<tauri::Monitor> {
    app.cursor_position()
        .ok()
        .and_then(|c| app.monitor_from_point(c.x, c.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten())
}
