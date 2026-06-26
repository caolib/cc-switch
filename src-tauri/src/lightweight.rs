use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use once_cell::sync::Lazy;
use tauri::Manager;
use tokio::sync::oneshot;

static LIGHTWEIGHT_MODE: AtomicBool = AtomicBool::new(false);

/// 进入轻量模式延迟计时器的取消通道。
/// 隐藏窗口时启动计时器，指定秒数后自动销毁 WebView 释放内存。
/// 在此期间若重新显示窗口则取消计时器。
static HIDE_TIMER_CANCEL: Lazy<Mutex<Option<oneshot::Sender<()>>>> =
    Lazy::new(|| Mutex::new(None));

/// 取消挂起的轻量模式延迟计时器（例如窗口被重新显示时调用）
pub fn cancel_pending_lightweight() {
    if let Some(sender) = HIDE_TIMER_CANCEL.lock().unwrap().take() {
        let _ = sender.send(());
        log::info!("已取消轻量模式延迟计时器");
    }
}

/// 安排延迟进入轻量模式。
/// - `-1`：不进入轻量模式（仅隐藏窗口）
/// - `0`：立即进入
/// - `>0`：延迟指定秒数后进入
/// 若已有挂起的计时器，先取消旧计时器。
pub fn schedule_enter_lightweight(app: &tauri::AppHandle, delay_seconds: i64) {
    // 先取消旧的计时器
    cancel_pending_lightweight();

    if delay_seconds < 0 {
        // -1 表示不进入轻量模式，仅隐藏窗口
        log::info!("轻量模式已禁用（delay={}），仅隐藏窗口", delay_seconds);
        return;
    }

    if delay_seconds == 0 {
        // 立即进入
        if let Err(e) = enter_lightweight_mode(app) {
            log::error!("进入轻量模式失败: {e}");
        }
        return;
    }

    let (tx, rx) = oneshot::channel();
    *HIDE_TIMER_CANCEL.lock().unwrap() = Some(tx);

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::select! {
            _ = rx => {
                // 计时器被取消（窗口重新显示）
            }
            _ = tokio::time::sleep(Duration::from_secs(delay_seconds as u64)) => {
                // 计时到期，进入轻量模式
                log::info!("隐藏计时到期（{} 秒），进入轻量模式释放 WebView 内存", delay_seconds);
                let _ = enter_lightweight_mode(&app);
            }
        }
    });

    log::info!("已安排 {} 秒后进入轻量模式", delay_seconds);
}

pub fn enter_lightweight_mode(app: &tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.set_skip_taskbar(true);
        }
    }
    #[cfg(target_os = "macos")]
    {
        crate::tray::apply_tray_policy(app, false);
    }

    if let Some(window) = app.get_webview_window("main") {
        crate::save_window_state_before_exit(app);
        window
            .destroy()
            .map_err(|e| format!("销毁主窗口失败: {e}"))?;
    }
    // else: already in lightweight mode or window not found, just set the flag

    LIGHTWEIGHT_MODE.store(true, Ordering::Release);
    crate::tray::refresh_tray_menu(app);
    log::info!("进入轻量模式");
    Ok(())
}

pub fn exit_lightweight_mode(app: &tauri::AppHandle) -> Result<(), String> {
    // 退出轻量模式前确保取消挂起的延迟计时器
    cancel_pending_lightweight();

    use tauri::WebviewWindowBuilder;

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        #[cfg(target_os = "linux")]
        {
            crate::linux_fix::nudge_main_window(window.clone());
        }
        #[cfg(target_os = "windows")]
        {
            let _ = window.set_skip_taskbar(false);
        }
        #[cfg(target_os = "macos")]
        {
            crate::tray::apply_tray_policy(app, true);
        }
        LIGHTWEIGHT_MODE.store(false, Ordering::Release);
        crate::tray::refresh_tray_menu(app);
        log::info!("退出轻量模式");
        return Ok(());
    }

    let window_config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .ok_or("主窗口配置未找到")?;

    WebviewWindowBuilder::from_config(app, window_config)
        .map_err(|e| format!("加载主窗口配置失败: {e}"))?
        .build()
        .map_err(|e| format!("创建主窗口失败: {e}"))?;

    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        #[cfg(target_os = "linux")]
        {
            crate::linux_fix::nudge_main_window(window.clone());
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.set_skip_taskbar(false);
        }
    }
    #[cfg(target_os = "macos")]
    {
        crate::tray::apply_tray_policy(app, true);
    }

    LIGHTWEIGHT_MODE.store(false, Ordering::Release);
    crate::tray::refresh_tray_menu(app);
    log::info!("退出轻量模式");
    Ok(())
}

pub fn is_lightweight_mode() -> bool {
    LIGHTWEIGHT_MODE.load(Ordering::Acquire)
}
