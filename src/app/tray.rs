//! Notification-area icon for background mode: Show / Open… / Quit.

use std::sync::mpsc::{self, Receiver};

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone, Copy)]
pub enum TrayAction {
    Show,
    Open,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    pub rx: Receiver<TrayAction>,
}

/// Must be called on the main (event-loop) thread.
pub fn create(ctx: &egui::Context) -> Option<Tray> {
    let img = image::load_from_memory(include_bytes!("../../assets/icon-256.png")).ok()?;
    let img = img.resize_exact(64, 64, image::imageops::FilterType::Lanczos3).to_rgba8();
    let icon = tray_icon::Icon::from_rgba(img.into_raw(), 64, 64).ok()?;

    let show = MenuItem::new("Show Flick", true, None);
    let open = MenuItem::new("Open video…", true, None);
    let quit = MenuItem::new("Quit Flick", true, None);
    let menu = Menu::new();
    menu.append(&show).ok()?;
    menu.append(&open).ok()?;
    menu.append(&PredefinedMenuItem::separator()).ok()?;
    menu.append(&quit).ok()?;

    let (tx, rx) = mpsc::channel();
    let (show_id, open_id, quit_id) = (show.id().clone(), open.id().clone(), quit.id().clone());
    {
        let (tx, ctx) = (tx.clone(), ctx.clone());
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let action = if e.id == show_id {
                TrayAction::Show
            } else if e.id == open_id {
                TrayAction::Open
            } else if e.id == quit_id {
                TrayAction::Quit
            } else {
                return;
            };
            let _ = tx.send(action);
            ctx.request_repaint();
        }));
    }
    {
        let ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                let _ = tx.send(TrayAction::Show);
                ctx.request_repaint();
            }
        }));
    }
    let icon = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Flick")
        .with_icon(icon)
        .build()
        .ok()?;
    Some(Tray { _icon: icon, rx })
}

