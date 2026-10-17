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
mod tray;
mod yandex;

use std::sync::Arc;

use tauri::tray::TrayIconBuilder;
use tauri::Manager;

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
    let _log_guard = logging::init();

    let settings = load_settings(&settings_path());
    let shared = Arc::new(AppSharedState::new(settings.clone()));

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // ТЗ §9.5: повторный запуск — показать окно существующего
            // экземпляра, не создавая второй процесс.
            if let Some(window) = app.get_webview_window("main") {
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
            let (supervisor, coordinator_rx) = Supervisor::new(Arc::clone(&shared));
            app.manage(supervisor);

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
                .icon(
                    app.default_window_icon()
                        .expect("иконка задана")
                        .clone(),
                )
                .menu(&menu)
                .tooltip("Yandex Music Presence")
                .on_menu_event(|app, event| {
                    let supervisor = app.state::<Supervisor>();
                    match event.id().as_ref() {
                        tray::MENU_OPEN => {
                            if let Some(window) = app.get_webview_window("main") {
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

            // Автозапуск подключения (ТЗ §4.4): если токен уже сохранён в
            // хранилище — подключаемся при старте, не дожидаясь ввода заново.
            match auth::load_token() {
                Ok(Some(_)) => {
                    shared.init_auth_required(true);
                    let supervisor = app.state::<Supervisor>();
                    supervisor.start_ynison(app.handle());
                }
                Ok(None) => {
                    shared.init_auth_required(false);
                    // Настройка может быть включена — просто не найдём токен
                    // (start_ynison сам залогирует «тока нет»).
                    if settings.start_on_launch {
                        let supervisor = app.state::<Supervisor>();
                        supervisor.start_ynison(app.handle());
                    }
                }
                Err(err) => {
                    // Хранилище недоступно: сообщаем, токен считаем не заданным.
                    shared.init_auth_required(false);
                    shared.push_error(&err, app.handle());
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
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
        })
        .invoke_handler(tauri::generate_handler![
            commands::playback::get_app_state,
            commands::playback::reconnect_yandex,
            commands::playback::reconnect_discord,
            commands::settings::get_settings,
            commands::settings::update_settings,
            commands::settings::set_presence_enabled,
            commands::auth::validate_yandex_token,
            commands::auth::save_yandex_token,
            commands::auth::remove_yandex_token,
            commands::auth::get_auth_status,
            commands::diagnostics::get_diagnostics,
            commands::diagnostics::open_log_directory,
            commands::diagnostics::quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("ошибка запуска tauri application");
}
