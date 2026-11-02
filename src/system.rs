//! Starting with the system, and the tray icon with its menu.

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

pub const MINIMIZED: &str = "--minimized";

fn autostart() -> Result<auto_launch::AutoLaunch, String> {
    let program = std::env::current_exe().map_err(|error| error.to_string())?;
    auto_launch::AutoLaunchBuilder::new()
        .set_app_name(crate::brand::NAME)
        .set_app_path(&program.to_string_lossy())
        .set_args(&[MINIMIZED])
        .set_use_launch_agent(true)
        .build()
        .map_err(|error| error.to_string())
}

pub fn set_autostart(on: bool) -> Result<(), String> {
    let launch = autostart()?;
    let result = if on { launch.enable() } else { launch.disable() };
    result.map_err(|error| error.to_string())
}

/// The launcher's icon (`assets/launcher-icon.png`, from `scripts/launcher_icon.py`; the JoF Launcher's
/// from `scripts/jof_art.py`):
/// a gold play mark in a gold ring on SJK's navy, `size` pixels square, RGBA.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    let png = crate::brand::ICON;
    match image::load_from_memory_with_format(png, image::ImageFormat::Png) {
        Ok(icon) => image::imageops::resize(&icon.to_rgba8(), size, size, image::imageops::FilterType::Lanczos3).into_raw(),
        Err(_) => vec![0; (size * size * 4) as usize],
    }
}

pub struct Tray {
    icon: TrayIcon,
}

/// What a tray click or menu item asks the launcher to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Chat,
    Play(crate::games::Game),
    Check,
    Quit,
}

impl Tray {
    /// `on_action` runs on whichever thread delivers tray events; it must not block.
    pub fn new(on_action: impl Fn(TrayAction) + Send + Sync + 'static) -> Result<Self, String> {
        use crate::games::Game;
        let menu = Menu::new();
        let open = MenuItem::new("Open launcher", true, None);
        let chat = MenuItem::new("SJK chat", true, None);
        let play_sjk = MenuItem::new("Play Sol JK", true, None);
        let play_jof = MenuItem::new("Play JoF EternalJK", true, None);
        let check = MenuItem::new("Check for updates", true, None);
        let quit = MenuItem::new("Quit", true, None);
        menu.append_items(&[
            &open,
            &chat,
            &PredefinedMenuItem::separator(),
            &play_sjk,
            &play_jof,
            &check,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .map_err(|error| error.to_string())?;
        let icon = Icon::from_rgba(icon_rgba(32), 32, 32).map_err(|error| error.to_string())?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(crate::brand::NAME)
            .with_icon(icon)
            .with_menu_on_left_click(false)
            .build()
            .map_err(|error| error.to_string())?;
        let actions: Vec<(MenuId, TrayAction)> = vec![
            (open.id().clone(), TrayAction::Show),
            (chat.id().clone(), TrayAction::Chat),
            (play_sjk.id().clone(), TrayAction::Play(Game::Sjk)),
            (play_jof.id().clone(), TrayAction::Play(Game::Jof)),
            (check.id().clone(), TrayAction::Check),
            (quit.id().clone(), TrayAction::Quit),
        ];
        let on_action = std::sync::Arc::new(on_action);
        let menu_action = std::sync::Arc::clone(&on_action);
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some((_, action)) = actions.iter().find(|(id, _)| *id == event.id) {
                menu_action(*action);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                on_action(TrayAction::Show);
            }
        }));
        Ok(Self { icon: tray })
    }

    pub fn set_tooltip(&self, text: &str) {
        let _ = self.icon.set_tooltip(Some(text));
    }
}
/// Open a file with the program the system gives its kind.
pub fn open(path: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// Show the hidden window. A hidden eframe window gets no frames on Windows, so
/// it is shown straight through the system rather than by a viewport command.
#[cfg(windows)]
pub fn show_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SW_RESTORE, SW_SHOW, SetForegroundWindow, ShowWindow};
    if hwnd != 0 {
        unsafe {
            ShowWindow(hwnd as _, SW_SHOW);
            ShowWindow(hwnd as _, SW_RESTORE);
            SetForegroundWindow(hwnd as _);
        }
    }
}

#[cfg(not(windows))]
pub fn show_window(_hwnd: isize) {}
