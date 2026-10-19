//! Точка входа приложения (ТЗ §4.4: жизненный цикл).
//!
//! Порядок инициализации:
//! 1) логирование → 2) настройки → 3) app state → 4) plugins (single
//! instance, opener, autostart) → 5) окно + tray → 6) supervisor +
//! coordinator → 7) commands. При закрытии окна приложение остаётся в трее
//! (если closeToTray включен).

mod app_state;
mod commands;
mod config;
mod discord;
mod logging;
mod presence;
mod runtime;
mod sources;
mod tray;
mod yandex;

use std::sync::Arc;

use tauri::Manager;
use tauri::tray::TrayIconBuilder;

use crate::app_state::AppSharedState;
use crate::config::persistence::{load_settings, save_settings, settings_path};
use crate::runtime::Supervisor;
use crate::yandex::auth;

/// Лёгкий dataset-free PRNG (xorshift от времени и pid) для jitter/device id.
pub fn rand_between(max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = u64::from(std::process::id());
    let mut x = now ^ (pid << 32) ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    x % max
}

/// Построить и запустить приложение.
pub fn run() {
    let settings = load_settings(&settings_path());
    // Логгинг инициализируем ПОСЛЕ чтения настроек: diagnosticsEnabled
    // задаёт подробность (debug) собственного кода.
    let _log_guard = logging::init(settings.diagnostics_enabled);

    let shared = Arc::new(AppSharedState::new(settings.clone()));

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // ТЗ §9.5: повторный запуск — показать окно существующего
            // экземпляра, не создавая второй процесс.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(move |app| {
            // Координатор — собственный std-поток (§4.2, §7).
            let (supervisor, coordinator_rx) =
                Supervisor::new(Arc::clone(&shared), app.handle().clone());
            app.manage(supervisor);
            app.manage(commands::auth::DeviceAuthManager::default());

            let coordinator_shared = Arc::clone(&shared);
            let app_handle = app.handle().clone();
            let coordinator_shutdown = shared.shutdown.clone();
            std::thread::Builder::new()
                .name("presence-coordinator".to_string())
                .spawn(move || {
                    presence::coordinator::run_coordinator(
                        coordinator_shared,
                        app_handle,
                        coordinator_rx,
                        coordinator_shutdown,
                    )
                })
                .expect("thread на coordinator важен");

            // Tray (ТЗ §9.5).
            let menu = tray::build_menu(app.handle(), settings.presence_enabled)?;
            TrayIconBuilder::with_id(tray::TRAY_ID)
                .icon(app.default_window_icon().expect("иконка задана").clone())
                .menu(&menu)
                .tooltip("Яндекс Музыка → Discord")
                .on_menu_event(|app, event| {
                    let supervisor = app.state::<Supervisor>();
                    match event.id().as_ref() {
                        tray::MENU_OPEN => {
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.unminimize();
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                        tray::MENU_TOGGLE => {
                            let current = supervisor.shared().settings();
                            let enabled = !current.presence_enabled;
                            let mut next = current.clone();
                            next.presence_enabled = enabled;
                            let _ = save_settings(&settings_path(), &next);
                            supervisor.shared().update_settings(next);
                            tray::update_tray(app, enabled);
                        }
                        tray::MENU_QUIT => {
                            supervisor.shutdown_all();
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            // Автозапуск источников (PLAN п.2): арбитр сам решает, что
            // запускать — Ynison при наличии токена, SMTC fallback при
            // отсутствии (sourceMode=auto) или по выбранному режиму.
            let supervisor = app.state::<Supervisor>();
            match auth::load_token() {
                Ok(maybe_token) => {
                    // Токен есть или нет — арбитр сам решает подмножество
                    // источников по режиму (SMTC fallback работает без токена).
                    supervisor.set_token_present(maybe_token.is_some(), app.handle());
                }
                Err(err) => {
                    // Хранилище недоступно: сообщаем, токен считаем не заданным.
                    supervisor.set_token_present(false, app.handle());
                    supervisor.shared().push_error(&err, app.handle());
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    // ТЗ §9.5: закрытие окна при closeToTray сворачивает в трей.
                    let app = window.app_handle();
                    let close_to_tray = app.state::<Supervisor>().shared().settings().close_to_tray;
                    if close_to_tray {
                        api.prevent_close();
                        let _ = window.hide();
                    } else {
                        // Явный close — как Quit: чистим задачи и Activity.
                        let supervisor = app.state::<Supervisor>();
                        supervisor.shutdown_all();
                    }
                }
                // Tauri 2 не имеет события «минимизация запросена»; стандартный
                // паттерн — Resized + is_minimized (ТЗ §9.3: «Минимизировать
                // в трей»). При флаге минимизация превращается в сворачивание
                // в трей: окно скрывается, процесс продолжает работать.
                tauri::WindowEvent::Resized(_) => {
                    let app = window.app_handle();
                    let minimize_to_tray = app
                        .state::<Supervisor>()
                        .shared()
                        .settings()
                        .minimize_to_tray;
                    if minimize_to_tray && window.is_minimized().unwrap_or(false) {
                        let _ = window.hide();
                    }
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::playback::get_app_state,
            commands::playback::list_media_sessions,
            commands::playback::reconnect_yandex,
            commands::playback::reconnect_discord,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::set_presence_enabled,
            commands::auth::validate_yandex_token,
            commands::auth::save_yandex_token,
            commands::auth::remove_yandex_token,
            commands::auth::get_auth_status,
            commands::auth::start_device_auth,
            commands::auth::cancel_device_auth,
            commands::diagnostics::get_diagnostics,
            commands::diagnostics::open_log_directory,
            commands::diagnostics::quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("ошибка запуска tauri application");
}
