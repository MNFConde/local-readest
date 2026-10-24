//! System-tray frontend (⑩ P4) — the third command-queue client, next to
//! the TUI and the admin-page buttons. This is the local control surface for
//! autostarted headless instances (which own no terminal): per-service
//! start/stop/restart, the boot-autostart toggle (install / unregister, the
//! 2×2 "process × registration" semantics), opening the admin page, and
//! quit = the global stop (both services die with the supervisor).
//!
//! Windows-only module: every tray-icon release's Linux backend is the GTK
//! stack (muda → glib/gdk/pango system deps), which a headless server build
//! must not drag in — the documented Linux control surfaces are systemctl,
//! the attach TUI and the admin page.
//!
//! The supervisor stays a plain binary: the tray thread runs its own minimal
//! Win32 message pump (tray-icon needs a pumping thread; a 2-second WM_TIMER
//! refreshes the tooltip).
//!
//! Menu item handles are muda objects (Rc — thread-bound), so the global
//! MenuEvent handler never touches them: it performs actions through the
//! thread-safe CTX and pushes desired checkmark state over a plain mpsc
//! channel that the pumping thread applies on its next tick.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};

use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::cmd::CommandHub;
use crate::state::{Dash, Phase, Slot};

struct TrayCtx {
    hub: CommandHub,
    kill_tx: tokio::sync::watch::Sender<bool>,
    exe: PathBuf,
    web_port: u16,
}

static CTX: OnceLock<TrayCtx> = OnceLock::new();
/// Desired checkmark of the autostart toggle, driven by the handler.
static CHECK_TX: Mutex<Option<mpsc::Sender<bool>>> = Mutex::new(None);
/// Our belief about the registration, owned by the handler.
static AUTOSTART_ON: AtomicBool = AtomicBool::new(false);

/// The bundle ships the reader's own 32x32 icon; embed it so the tray works
/// from any working directory with no extra files.
const ICON_BYTES: &[u8] = include_bytes!("../../../apps/readest-app/src-tauri/icons/32x32.png");

fn load_icon() -> Option<Icon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(ICON_BYTES));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba {
        return None;
    }
    Icon::from_rgba(buf, info.width, info.height).ok()
}

fn item(id: &str, text: &str) -> MenuItem {
    MenuItem::with_id(MenuId::new(id), text, true, None)
}

fn build_menu() -> anyhow::Result<(Menu, CheckMenuItem)> {
    let menu = Menu::new();
    for slot_label in ["sync-server", "web"] {
        let header = MenuItem::with_id(
            MenuId::new(format!("{slot_label}:head")),
            slot_label,
            false,
            None,
        );
        menu.append(&header)?;
        menu.append(&item(&format!("{slot_label}:start"), "Start"))?;
        menu.append(&item(&format!("{slot_label}:stop"), "Stop"))?;
        menu.append(&item(&format!("{slot_label}:restart"), "Restart"))?;
        menu.append(&PredefinedMenuItem::separator())?;
    }
    let exe_dir = CTX
        .get()
        .and_then(|c| c.exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    let registered = crate::autostart::is_registered(&exe_dir);
    AUTOSTART_ON.store(registered, Ordering::SeqCst);
    let autostart = CheckMenuItem::with_id(
        MenuId::new("autostart"),
        "Autostart at login",
        true,
        registered,
        None,
    );
    menu.append(&autostart)?;
    menu.append(&item("open-admin", "Open admin page"))?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&item("quit", "Quit (stop everything)"))?;
    Ok((menu, autostart))
}

fn menu_handler(event: MenuEvent) {
    let Some(ctx) = CTX.get() else { return };
    let id = event.id.0.as_str();
    match id {
        "autostart" => {
            // The check item auto-toggles on click; we drive the truth from
            // our own belief, act, then push the outcome for the pump thread
            // to render (handles are thread-bound, see the module comment).
            let want_on = !AUTOSTART_ON.load(Ordering::SeqCst);
            let exe_dir = ctx
                .exe
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            let outcome = if want_on {
                crate::autostart::install(&ctx.exe)
                    .map(|created| eprintln!("supervisor: autostart registered ({created})"))
            } else {
                crate::autostart::unregister(&exe_dir).map(|existed| {
                    eprintln!(
                        "supervisor: autostart unregistered ({}); the running instance is untouched — {}",
                        if existed { "removed" } else { "nothing was registered" },
                        crate::autostart::temporary_stop_hint()
                    );
                })
            };
            let final_on = match &outcome {
                Ok(()) => want_on,
                Err(e) => {
                    eprintln!("supervisor: autostart change failed: {e}");
                    !want_on
                }
            };
            AUTOSTART_ON.store(final_on, Ordering::SeqCst);
            if let Some(tx) = CHECK_TX.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                let _ = tx.send(final_on);
            }
        }
        "open-admin" => open_admin(ctx.web_port),
        "quit" => {
            let _ = ctx.kill_tx.send(true);
        }
        other => {
            if let Some((slot, cmd)) = other.rsplit_once(':') {
                if let Err(e) = ctx.hub.send_from_labels(slot, cmd) {
                    eprintln!("supervisor: tray command rejected: {e}");
                }
            }
        }
    }
}

fn open_admin(web_port: u16) {
    let url = format!("http://127.0.0.1:{web_port}/admin");
    if let Ok(mut child) = std::process::Command::new("cmd")
        .args(["/C", "start", "", &url])
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn tooltip_text(dash: &Dash) -> String {
    let mut parts = Vec::with_capacity(2);
    for slot in [Slot::Sync, Slot::Web] {
        let st = dash.status(slot);
        let extra = match st.phase {
            Phase::Restarting => st
                .restart_at
                .map(|at| {
                    format!(
                        " in {}s",
                        at.saturating_duration_since(std::time::Instant::now())
                            .as_secs()
                    )
                })
                .unwrap_or_default(),
            _ => String::new(),
        };
        parts.push(format!("{}: {}{}", slot.name(), st.phase.label(), extra));
    }
    format!("Readest Server\n{}", parts.join("\n"))
}

/// Spawn the tray thread. Degrades to a log line on any failure — the
/// supervisor's supervision never depends on it.
pub fn spawn(
    dash: Arc<Dash>,
    hub: CommandHub,
    kill_tx: tokio::sync::watch::Sender<bool>,
    exe: PathBuf,
    web_port: u16,
) {
    if CTX
        .set(TrayCtx {
            hub,
            kill_tx,
            exe,
            web_port,
        })
        .is_err()
    {
        return; // double spawn: ignore
    }
    std::thread::spawn(move || {
        let Some(icon) = load_icon() else {
            eprintln!("supervisor: tray icon unavailable — tray disabled");
            return;
        };
        let (menu, autostart_item) = match build_menu() {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("supervisor: tray menu build failed ({e}) — tray disabled");
                return;
            }
        };
        MenuEvent::set_event_handler(Some(menu_handler));
        let tray = match TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Readest Server")
            .with_icon(icon)
            .build()
        {
            Ok(t) => t,
            Err(e) => {
                // No desktop / no DBus session (typical headless Linux): the
                // documented control path there is systemctl, by design.
                eprintln!("supervisor: tray unavailable ({e}) — continuing without it");
                return;
            }
        };
        // The pumping thread owns the check item; the handler pushes desired
        // state through CHECK_TX.
        let (check_tx, check_rx) = mpsc::channel::<bool>();
        if let Some(tx_slot) = CHECK_TX.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            *tx_slot = check_tx;
        }
        run_message_loop(tray, autostart_item, check_rx, dash);
    });
}

/// Windows: pump Win32 messages forever; a 2s WM_TIMER refreshes the tooltip
/// and applies pending checkmark updates. tray-icon needs its objects
/// created on (and pumped by) one thread.
#[cfg(windows)]
fn run_message_loop(
    tray: TrayIcon,
    autostart_item: CheckMenuItem,
    check_rx: mpsc::Receiver<bool>,
    dash: Arc<Dash>,
) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, SetTimer,
        TranslateMessage, MSG, WM_TIMER, WNDCLASSW,
    };
    unsafe {
        let hinstance =
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
        let class_name: Vec<u16> = "ReadestSupervisorTrayPump\0".encode_utf16().collect();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            lpszClassName: class_name.as_ptr(),
            hInstance: hinstance,
            ..std::mem::zeroed()
        };
        if RegisterClassW(&wc) == 0 {
            eprintln!("supervisor: tray RegisterClassW failed — tray disabled");
            return;
        }
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0, // never shown: message pump only
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            eprintln!("supervisor: tray window creation failed — tray disabled");
            return;
        }
        SetTimer(hwnd, 1, 2000, None);
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let r = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if r <= 0 {
                break; // WM_QUIT or error: main is exiting anyway
            }
            if msg.message == WM_TIMER {
                let _ = tray.set_tooltip(Some(&tooltip_text(&dash)));
                apply_pending_checks(&autostart_item, &check_rx);
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn apply_pending_checks(autostart_item: &CheckMenuItem, check_rx: &mpsc::Receiver<bool>) {
    while let Ok(want) = check_rx.try_recv() {
        autostart_item.set_checked(want);
    }
}
