use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, Manager, Rect, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const CARD_WIDTH: f64 = 396.0;
const MIN_CARD_HEIGHT: f64 = 180.0;
const MAX_CARD_HEIGHT: f64 = 560.0;
const DEFAULT_CARD_HEIGHT: f64 = 380.0;
#[cfg(not(target_os = "macos"))]
const ICON_GAP: f64 = 6.0;
const SCREEN_MARGIN: f64 = 8.0;
const BLUR_GRACE_MS: i64 = 300;
const SHOW_GRACE_MS: i64 = 250;

// Asymmetric macOS shadow canvas: 8px top keeps card snug under menubar; 24px/26px sides & bottom
// let the CSS Gaussian shadow decay to 0 before reaching window bounds (zero clipped edge).
#[cfg(target_os = "macos")]
const PAD_TOP: f64 = 8.0;
#[cfg(target_os = "macos")]
const PAD_X: f64 = 24.0;
#[cfg(target_os = "macos")]
const PAD_BOTTOM: f64 = 26.0;
#[cfg(target_os = "macos")]
const CORNER_RADIUS: f64 = 14.0;

static WINDOW_PINNED: AtomicBool = AtomicBool::new(false);
static LAST_SHOWN_MS: AtomicI64 = AtomicI64::new(0);
static LAST_BLUR_HIDE_MS: AtomicI64 = AtomicI64::new(0);
static CARD_HEIGHT_BITS: AtomicU64 = AtomicU64::new(DEFAULT_CARD_HEIGHT.to_bits());

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn current_card_height() -> f64 {
    f64::from_bits(CARD_HEIGHT_BITS.load(Ordering::Relaxed))
}

pub fn set_pinned(pinned: bool) -> bool {
    WINDOW_PINNED.store(pinned, Ordering::Relaxed);
    pinned
}

pub fn ensure_window(app: &AppHandle) -> Option<WebviewWindow> {
    if let Some(win) = app.get_webview_window("main") {
        return Some(win);
    }
    let ch = current_card_height();
    #[cfg(target_os = "macos")]
    let (w, h) = (CARD_WIDTH + PAD_X * 2.0, ch + PAD_TOP + PAD_BOTTOM);
    #[cfg(not(target_os = "macos"))]
    let (w, h) = (CARD_WIDTH, ch);

    let win = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Antigravity Switch")
        .inner_size(w, h)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(!cfg!(target_os = "macos"))
        .always_on_top(true)
        .skip_taskbar(true)
        .visible_on_all_workspaces(true)
        .visible(false)
        .focused(false)
        .build()
        .ok()?;
    init_window(app, &win);
    Some(win)
}

pub fn init_window(app: &AppHandle, win: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    macos::init_window(app, win);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, win);
    }
}

pub fn sync_tray_hit_area(app: &AppHandle, tray_id: &str) {
    #[cfg(target_os = "macos")]
    macos::sync_tray_hit_area(app, tray_id);

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, tray_id);
    }
}

pub fn set_card_height(app: &AppHandle, height: f64) {
    if !height.is_finite() {
        return;
    }
    let clamped = height.clamp(MIN_CARD_HEIGHT, MAX_CARD_HEIGHT);
    let prev = f64::from_bits(CARD_HEIGHT_BITS.swap(clamped.to_bits(), Ordering::Relaxed));
    if (prev - clamped).abs() < 0.25 {
        return;
    }

    #[cfg(target_os = "macos")]
    macos::resize_popover(app, clamped);

    #[cfg(not(target_os = "macos"))]
    other::resize_popover(app, clamped.ceil());
}

pub fn hide_popover(win: &WebviewWindow) {
    let _ = win.hide();
}

pub fn on_window_blur(win: &WebviewWindow) {
    if WINDOW_PINNED.load(Ordering::Relaxed) || !win.is_visible().unwrap_or(false) {
        return;
    }
    let now = now_ms();
    if now - LAST_SHOWN_MS.load(Ordering::Relaxed) > SHOW_GRACE_MS {
        LAST_BLUR_HIDE_MS.store(now, Ordering::Relaxed);
        let _ = win.hide();
    }
}

pub fn toggle_popover(app: &AppHandle, tray_id: &str, tray_rect: Option<Rect>) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            hide_popover(&win);
            return;
        }
    }
    // Clicking the tray icon blurs and hides an open popover on mouseDown; ignore the ensuing mouseUp.
    if now_ms() - LAST_BLUR_HIDE_MS.load(Ordering::Relaxed) < BLUR_GRACE_MS {
        return;
    }
    show_popover(app, tray_id, tray_rect, true);
}

pub fn show_popover(app: &AppHandle, tray_id: &str, tray_rect: Option<Rect>, from_click: bool) {
    #[cfg(target_os = "macos")]
    {
        let _ = tray_id;
        macos::show_popover(app, tray_rect, from_click);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = (tray_id, from_click);
        other::show_popover(app, tray_rect);
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::cell::RefCell;

    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, NSObjectProtocol};
    use objc2::{msg_send, sel, MainThreadMarker};
    use objc2_app_kit::{
        NSAutoresizingMaskOptions, NSEvent, NSGlassEffectView, NSGlassEffectViewStyle, NSScreen,
        NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState,
        NSVisualEffectView, NSWindow, NSWindowCollectionBehavior, NSWindowOrderingMode,
    };
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    thread_local! {
        static GLASS_VIEW: RefCell<Option<Retained<NSView>>> = const { RefCell::new(None) };
    }

    fn ns_window(win: &WebviewWindow) -> Option<&NSWindow> {
        let ptr = win.ns_window().ok()?;
        // SAFETY: Pointer comes from tao and remains valid for the window's lifetime on main thread.
        Some(unsafe { &*ptr.cast::<NSWindow>() })
    }

    fn make_glass(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSView> {
        if AnyClass::get(c"NSGlassEffectView").is_some() {
            let glass = NSGlassEffectView::initWithFrame(mtm.alloc(), frame);
            glass.setStyle(NSGlassEffectViewStyle::Regular);
            glass.setCornerRadius(CORNER_RADIUS);
            return Retained::into_super(glass);
        }
        let effect = NSVisualEffectView::initWithFrame(mtm.alloc(), frame);
        effect.setMaterial(NSVisualEffectMaterial::Popover);
        effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        effect.setState(NSVisualEffectState::Active);
        if effect.respondsToSelector(sel!(setCornerRadius:)) {
            unsafe {
                let _: () = msg_send![&*effect, setCornerRadius: CORNER_RADIUS];
            }
        } else {
            effect.setWantsLayer(true);
            unsafe {
                let layer: *mut AnyObject = msg_send![&*effect, layer];
                if !layer.is_null() {
                    let _: () = msg_send![layer, setCornerRadius: CORNER_RADIUS];
                    let _: () = msg_send![layer, setMasksToBounds: true];
                }
            }
        }
        Retained::into_super(effect)
    }

    fn glass_rect_for(content: &NSView, card_h: f64) -> NSRect {
        let content_h = content.bounds().size.height;
        let y = if content.isFlipped() {
            PAD_TOP
        } else {
            (content_h - PAD_TOP - card_h).max(0.0)
        };
        NSRect::new(NSPoint::new(PAD_X, y), NSSize::new(CARD_WIDTH, card_h))
    }

    fn sync_glass_frame(window: &NSWindow, card_h: f64) {
        let Some(content) = window.contentView() else {
            return;
        };
        let rect = glass_rect_for(&content, card_h);
        GLASS_VIEW.with(|slot| {
            if let Some(view) = slot.borrow().as_ref() {
                view.setFrame(rect);
            }
        });
    }

    fn configure_window_on_main(mtm: MainThreadMarker, win: &WebviewWindow) {
        let Some(window) = ns_window(win) else {
            return;
        };

        // FullScreenAuxiliary allows the popover to appear over fullscreen Spaces (CanJoinAllSpaces alone cannot).
        window.setCollectionBehavior(
            window.collectionBehavior() | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );

        let card_h = current_card_height();
        let win_w = CARD_WIDTH + PAD_X * 2.0;
        let win_h = card_h + PAD_TOP + PAD_BOTTOM;
        let cur = window.frame();
        window.setFrame_display(
            NSRect::new(cur.origin, NSSize::new(win_w, win_h)),
            false,
        );

        if let Some(content) = window.contentView() {
            let glass = make_glass(mtm, glass_rect_for(&content, card_h));
            content.addSubview_positioned_relativeTo(
                &glass,
                NSWindowOrderingMode::Below,
                None,
            );
            GLASS_VIEW.with(|slot| {
                *slot.borrow_mut() = Some(glass);
            });
        }
    }

    pub fn init_window(_app: &AppHandle, win: &WebviewWindow) {
        let _ = win.set_visible_on_all_workspaces(true);
        // Disable system shadow on macOS to avoid dark outline around transparent Liquid Glass cards.
        let _ = win.set_shadow(false);

        if let Some(mtm) = MainThreadMarker::new() {
            configure_window_on_main(mtm, win);
        } else {
            let target = win.clone();
            let _ = win.run_on_main_thread(move || {
                if let Some(mtm) = MainThreadMarker::new() {
                    configure_window_on_main(mtm, &target);
                }
            });
        }
    }

    pub fn resize_popover(app: &AppHandle, card_h: f64) {
        let app_clone = app.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(win) = app_clone.get_webview_window("main") else {
                return;
            };
            let Some(window) = ns_window(&win) else {
                return;
            };
            let cur = window.frame();
            let new_h = card_h.ceil() + PAD_TOP + PAD_BOTTOM;
            // Anchor to the top edge so resizing keeps the card locked beneath the menubar.
            let top_y = cur.origin.y + cur.size.height;
            let mut new_y = top_y - new_h;
            if let Some(screen) = window.screen() {
                let min_win_y = screen.visibleFrame().origin.y + SCREEN_MARGIN - PAD_BOTTOM;
                if new_y < min_win_y {
                    new_y = min_win_y;
                }
            }
            window.setFrame_display(
                NSRect::new(
                    NSPoint::new(cur.origin.x, new_y),
                    NSSize::new(CARD_WIDTH + PAD_X * 2.0, new_h),
                ),
                true,
            );
            sync_glass_frame(window, card_h);
        });
    }

    fn apply_tray_hit_area_async(app: AppHandle, tray_id: String, delay_ms: u64) {
        tauri::async_runtime::spawn(async move {
            if delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            let Some(tray) = app.tray_by_id(&tray_id) else {
                return;
            };
            // Run with_inner_tray_icon off the main thread so its internal main-thread channel wait never deadlocks.
            let _ = tokio::task::spawn_blocking(move || {
                let _ = tray.with_inner_tray_icon(|inner| {
                    let Some(mtm) = MainThreadMarker::new() else {
                        return;
                    };
                    if let Some(status_item) = inner.ns_status_item() {
                        if let Some(button) = status_item.button(mtm) {
                            // tray-icon's TaoTrayTarget subview is created without autoresizing and updated before
                            // AppKit lays out setTitle() text, leaving the text region unclickable.
                            button.setAutoresizesSubviews(true);
                            button.layoutSubtreeIfNeeded();
                            let bounds = button.bounds();
                            let mask = NSAutoresizingMaskOptions::ViewWidthSizable
                                | NSAutoresizingMaskOptions::ViewHeightSizable;
                            for subview in button.subviews() {
                                if subview.class().name() == c"TaoTrayTarget" {
                                    subview.setAutoresizingMask(mask);
                                    subview.setFrame(bounds);
                                }
                            }
                        }
                    }
                });
            })
            .await;
        });
    }

    pub fn sync_tray_hit_area(app: &AppHandle, tray_id: &str) {
        apply_tray_hit_area_async(app.clone(), tray_id.to_string(), 0);
        apply_tray_hit_area_async(app.clone(), tray_id.to_string(), 120);
    }

    fn rect_contains_point(rect: NSRect, pt: NSPoint) -> bool {
        pt.x >= rect.origin.x
            && pt.x <= rect.origin.x + rect.size.width
            && pt.y >= rect.origin.y
            && pt.y <= rect.origin.y + rect.size.height
    }

    fn tray_rect_to_appkit(rect: Rect, scale: f64, primary_height: f64) -> NSRect {
        let pos = rect.position.to_logical::<f64>(scale);
        let size = rect.size.to_logical::<f64>(scale);
        NSRect::new(
            NSPoint::new(pos.x, primary_height - pos.y - size.height),
            NSSize::new(size.width, size.height),
        )
    }

    pub fn show_popover(app: &AppHandle, tray_rect: Option<Rect>, from_click: bool) {
        let app_clone = app.clone();
        let _ = app.run_on_main_thread(move || {
            let Some(mtm) = MainThreadMarker::new() else {
                return;
            };
            let Some(win) = ensure_window(&app_clone) else {
                return;
            };
            let Some(window) = ns_window(&win) else {
                return;
            };

            let card_h = current_card_height();
            let mouse = NSEvent::mouseLocation();
            let screens = NSScreen::screens(mtm);
            let primary_height = screens
                .firstObject()
                .map_or(900.0, |s| s.frame().size.height);
            let chosen_screen = screens
                .iter()
                .find(|s| rect_contains_point(s.frame(), mouse))
                .or_else(|| NSScreen::mainScreen(mtm))
                .or_else(|| screens.firstObject());

            let (screen_frame, visible, scale) = match chosen_screen {
                Some(s) => (s.frame(), s.visibleFrame(), s.backingScaleFactor()),
                None => {
                    let f = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1440.0, 900.0));
                    (f, f, 2.0)
                }
            };

            let visible_top = visible.origin.y + visible.size.height;
            let screen_top = screen_frame.origin.y + screen_frame.size.height;

            // Only trust tray_rect when it sits in the active screen's top menubar band;
            // at startup NSStatusBarWindow can report (0,0), and mirrored menubars can point to another display.
            let btn_frame_on_screen = tray_rect.and_then(|r| {
                let bf = tray_rect_to_appkit(r, scale.max(1.0), primary_height);
                let center = NSPoint::new(
                    bf.origin.x + bf.size.width / 2.0,
                    bf.origin.y + bf.size.height / 2.0,
                );
                let in_menubar_band =
                    bf.size.width > 0.0 && bf.origin.y >= screen_top - 64.0 && bf.origin.y <= screen_top;
                (in_menubar_band && rect_contains_point(screen_frame, center)).then_some(bf)
            });

            let fallback_menubar_bottom = if screen_top - visible_top >= 18.0 {
                visible_top
            } else {
                screen_top - 32.0
            };

            let (anchor_x, anchor_card_top) = if let Some(bf) = btn_frame_on_screen {
                (bf.origin.x + bf.size.width / 2.0, bf.origin.y - PAD_TOP)
            } else if from_click {
                (mouse.x, fallback_menubar_bottom - PAD_TOP)
            } else {
                (
                    visible.origin.x + visible.size.width - CARD_WIDTH / 2.0 - 16.0,
                    fallback_menubar_bottom - PAD_TOP,
                )
            };

            let min_x = visible.origin.x + SCREEN_MARGIN;
            let max_x = (visible.origin.x + visible.size.width - SCREEN_MARGIN - CARD_WIDTH).max(min_x);
            let card_x = (anchor_x - CARD_WIDTH / 2.0).clamp(min_x, max_x);

            // Keep the outer window frame strictly below the menubar so AppKit never shifts it after showing.
            let max_card_top = fallback_menubar_bottom.min(visible_top) - PAD_TOP;
            let card_top = anchor_card_top.min(max_card_top);
            let card_y = (card_top - card_h).max(visible.origin.y + SCREEN_MARGIN);

            let win_h = card_h.ceil() + PAD_TOP + PAD_BOTTOM;
            let win_rect = NSRect::new(
                NSPoint::new(card_x - PAD_X, card_y - (win_h - PAD_TOP - card_h)),
                NSSize::new(CARD_WIDTH + PAD_X * 2.0, win_h),
            );
            window.setFrame_display(win_rect, true);
            sync_glass_frame(window, card_h);

            LAST_SHOWN_MS.store(now_ms(), Ordering::Relaxed);
            window.orderFrontRegardless();
            let _ = win.show();
            let _ = win.unminimize();
            let _ = win.set_focus();
            let _ = win.emit("popover-shown", ());
        });
    }
}

#[cfg(not(target_os = "macos"))]
mod other {
    use super::*;
    use tauri::{LogicalSize, PhysicalPosition, Position, Size};

    fn rect_to_physical(rect: Rect, scale: f64) -> (f64, f64, f64, f64) {
        let (rx, ry) = match rect.position {
            Position::Physical(p) => (p.x as f64, p.y as f64),
            Position::Logical(p) => (p.x * scale, p.y * scale),
        };
        let (rw, rh) = match rect.size {
            Size::Physical(s) => (s.width as f64, s.height as f64),
            Size::Logical(s) => (s.width * scale, s.height * scale),
        };
        (rx, ry, rw, rh)
    }

    pub fn resize_popover(app: &AppHandle, card_h: f64) {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.set_size(LogicalSize::new(CARD_WIDTH, card_h));
        }
    }

    pub fn show_popover(app: &AppHandle, tray_rect: Option<Rect>) {
        let Some(win) = ensure_window(app) else {
            return;
        };
        let card_h = current_card_height();
        let monitors = win.available_monitors().unwrap_or_default();
        let fallback_monitor = win
            .primary_monitor()
            .ok()
            .flatten()
            .or_else(|| monitors.first().cloned());
        let default_scale = fallback_monitor
            .as_ref()
            .map(|m| m.scale_factor())
            .unwrap_or_else(|| win.scale_factor().unwrap_or(1.0));

        let chosen_monitor = if let Some(rect) = tray_rect {
            let (rx, ry, rw, rh) = rect_to_physical(rect, default_scale);
            let cx = rx + rw / 2.0;
            let cy = ry + rh / 2.0;
            monitors
                .iter()
                .find(|m| {
                    let p = m.position();
                    let s = m.size();
                    cx >= p.x as f64
                        && cx <= p.x as f64 + s.width as f64
                        && cy >= p.y as f64
                        && cy <= p.y as f64 + s.height as f64
                })
                .cloned()
                .or(fallback_monitor)
        } else {
            fallback_monitor
        };

        let scale = chosen_monitor
            .as_ref()
            .map(|m| m.scale_factor())
            .unwrap_or(default_scale);
        let (mon_x, mon_y, mon_w, mon_h) = match chosen_monitor {
            Some(m) => {
                let p = m.position();
                let s = m.size();
                (p.x as f64, p.y as f64, s.width as f64, s.height as f64)
            }
            None => (0.0, 0.0, 1440.0 * scale, 900.0 * scale),
        };

        let win_w = CARD_WIDTH * scale;
        let win_h = card_h * scale;
        let margin = SCREEN_MARGIN * scale;

        let (x, y) = if let Some(rect) = tray_rect {
            let (rx, ry, rw, rh) = rect_to_physical(rect, scale);
            let x = (rx + rw / 2.0 - win_w / 2.0)
                .clamp(mon_x + margin, (mon_x + mon_w - win_w - margin).max(mon_x + margin));
            let y = if ry > mon_y + mon_h / 2.0 {
                (ry - win_h - margin).max(mon_y + margin)
            } else {
                ry + rh + ICON_GAP * scale
            };
            (x, y)
        } else {
            let x = (mon_x + mon_w - win_w - 16.0 * scale).max(mon_x + margin);
            let y = (mon_y + mon_h - win_h - 52.0 * scale).max(mon_y + margin);
            (x, y)
        };

        let _ = win.set_size(LogicalSize::new(CARD_WIDTH, card_h));
        let _ = win.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
        LAST_SHOWN_MS.store(now_ms(), Ordering::Relaxed);
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
        let _ = win.emit("popover-shown", ());
    }
}
