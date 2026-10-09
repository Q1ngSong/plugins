//! 托盘图标：点窗口的关闭按钮不退出，缩到托盘，任务栏上不留图标；点托盘图标恢复，右键菜单里有「打开」和「退出」。
//! 最小化照常留在任务栏。真正退出只走托盘菜单的「退出」（更新后重启不经过关闭按钮，不受影响）。
//! 哪些系统这样做看 platform::TRAY。
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Manager, Webview, WebviewWindow, WindowEvent};

/// 窗口里的页面。窗口藏起来时页面自己不知道（WebView2 照样报告看得见），每分钟一次的刷新会一直跑下去，
/// 所以藏窗口时把页面也设成看不见，叫回来时再设回来（页面随即刷新一次）
fn page(w: &WebviewWindow) -> &Webview {
    w.as_ref()
}

/// 把窗口叫回来：从最小化恢复、显示出来、放到最前面
pub fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = page(&w).show();
        let _ = w.set_focus();
    }
}

/// 建托盘图标，并让 window 点关闭时缩进托盘。托盘图标建不出来就返回错误，窗口照旧点关闭就退出
pub fn install(app: &App, window: &WebviewWindow) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let mut tray = TrayIconBuilder::with_id("main")
        .tooltip("插件中心")
        .menu(&Menu::with_items(app, &[&open, &quit])?)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    let w = window.clone();
    window.on_window_event(move |event| {
        // 点关闭不退出，藏起来留在托盘
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = w.hide();
            let _ = page(&w).hide();
        }
    });
    Ok(())
}
