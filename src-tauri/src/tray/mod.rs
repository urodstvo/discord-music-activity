//! System tray (ТЗ §9.5): меню, поведение клика, сворачивание в трей.

use tauri::menu::MenuBuilder;
use tauri::{AppHandle, Wry};

/// Идентификаторы пунктов меню tray.
pub const MENU_OPEN: &str = "open_window";
pub const MENU_TOGGLE: &str = "toggle_publish";
pub const MENU_QUIT: &str = "quit";

/// Идентификатор tray-иконки приложения.
pub const TRAY_ID: &str = "main";

/// Построить меню tray в соответствии с текущим состоянием публикации.
pub fn build_menu(
    app: &AppHandle,
    presence_enabled: bool,
) -> tauri::Result<tauri::menu::Menu<Wry>> {
    let toggle_label = if presence_enabled {
        "Публикация: выключить"
    } else {
        "Публикация: включить"
    };
    MenuBuilder::new(app)
        .text(MENU_OPEN, "Открыть окно")
        .separator()
        .text(MENU_TOGGLE, toggle_label)
        .separator()
        .text(MENU_QUIT, "Выход")
        .build()
}

/// Обновить пункт toggle по текущему Presence enabled.
pub fn update_tray(app: &AppHandle, presence_enabled: bool) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        if let Ok(menu) = build_menu(app, presence_enabled) {
            let _ = tray.set_menu(Some(menu));
        }
    }
}
