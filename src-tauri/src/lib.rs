use tauri::{
    menu::{MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Emitter, Manager, PhysicalPosition, WindowEvent,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, atomic::{AtomicBool, AtomicU64, Ordering}};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use chrono::{Local, Datelike, Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DailyEntry {
    period: String,
    #[serde(rename = "totalCost")]
    total_cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DailyResponse {
    daily: Vec<DailyEntry>,
}

#[derive(Debug, Clone)]
struct AppData {
    daily_entries: Vec<DailyEntry>,
    last_updated: Option<Instant>,
    ccusage_available: bool,
}

static APP_CACHE: Mutex<AppData> = Mutex::new(AppData {
    daily_entries: Vec::new(),
    last_updated: None,
    ccusage_available: false,
});

static IS_REFRESHING: AtomicBool = AtomicBool::new(false);
// Unix-ms timestamp of the last time the popover was hidden due to losing focus.
// Used so that clicking the tray icon to dismiss the window doesn't immediately reopen it.
static LAST_HIDE_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Stats payload sent to the webview
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct DayStat {
    name: String,
    cost: f64,
    #[serde(rename = "isToday")]
    is_today: bool,
}

#[derive(Debug, Clone, Serialize)]
struct StatsPayload {
    available: bool,
    #[serde(rename = "hasData")]
    has_data: bool,
    #[serde(rename = "monthTotal")]
    month_total: f64,
    today: f64,
    #[serde(rename = "weekTotal")]
    week_total: f64,
    days: Vec<DayStat>,
}

fn compute_stats() -> StatsPayload {
    let cache = APP_CACHE.lock().unwrap();
    let available = cache.ccusage_available;
    let has_data = cache.last_updated.is_some();

    let today = Local::now().date_naive();
    let today_str = today.format("%Y-%m-%d").to_string();
    let month_prefix = today.format("%Y-%m").to_string();

    let cost_map: HashMap<String, f64> = cache.daily_entries.iter()
        .map(|e| (e.period.clone(), e.total_cost))
        .collect();

    let month_total: f64 = cache.daily_entries.iter()
        .filter(|e| e.period.starts_with(&month_prefix))
        .map(|e| e.total_cost)
        .sum();

    let today_cost = cost_map.get(&today_str).copied().unwrap_or(0.0);

    let days_from_monday = today.weekday().num_days_from_monday() as i64;
    let week_start = today - Duration::days(days_from_monday);

    let day_names = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    let mut days = Vec::with_capacity(7);
    let mut week_total = 0.0;
    for i in 0i64..7 {
        let d = week_start + Duration::days(i);
        let ds = d.format("%Y-%m-%d").to_string();
        let cost = cost_map.get(&ds).copied().unwrap_or(0.0);
        week_total += cost;
        days.push(DayStat {
            name: day_names[i as usize].to_string(),
            cost,
            is_today: ds == today_str,
        });
    }

    StatsPayload {
        available,
        has_data,
        month_total,
        today: today_cost,
        week_total,
        days,
    }
}

// ---------------------------------------------------------------------------
// Tauri commands invoked from the webview
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_stats() -> StatsPayload {
    compute_stats()
}

#[tauri::command]
async fn refresh_now(app: tauri::AppHandle) {
    refresh_data(&app).await;
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
async fn debug_info() -> String {
    get_debug_info().await
}

// ---------------------------------------------------------------------------
// ccusage integration
// ---------------------------------------------------------------------------

async fn fetch_daily_data() -> (Vec<DailyEntry>, bool) {
    let shell_commands = vec![
        ("sh", vec!["-c", "PATH=/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:$HOME/.npm/bin:$HOME/.nvm/versions/node/*/bin:$HOME/.volta/bin:$PATH npx ccusage@latest daily --json"]),
        ("sh", vec!["-c", "PATH=/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:$HOME/.npm/bin:$HOME/.nvm/versions/node/*/bin:$HOME/.volta/bin:$PATH ccusage daily --json"]),
        ("sh", vec!["-c", "npx ccusage@latest daily --json"]),
        ("sh", vec!["-c", "ccusage daily --json"]),
        ("npx", vec!["ccusage@latest", "daily", "--json"]),
        ("ccusage", vec!["daily", "--json"]),
    ];

    for (cmd, args) in shell_commands {
        let output = Command::new(cmd).args(&args).output().await;
        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                match serde_json::from_str::<DailyResponse>(&stdout) {
                    Ok(response) => return (response.daily, true),
                    Err(e) => {
                        eprintln!("Failed to parse ccusage daily response: {}", e);
                        continue;
                    }
                }
            }
            Ok(output) => {
                eprintln!("ccusage daily command failed: {}", output.status);
                continue;
            }
            Err(e) => {
                eprintln!("Failed to execute '{}': {}", cmd, e);
                continue;
            }
        }
    }

    eprintln!("All attempts to fetch daily data failed");
    (Vec::new(), false)
}

async fn get_debug_info() -> String {
    let mut debug_info = String::new();

    debug_info.push_str("Environment:\n");
    if let Ok(path) = std::env::var("PATH") {
        debug_info.push_str(&format!("Default PATH: {}\n", path));
    } else {
        debug_info.push_str("Default PATH: (not set)\n");
    }

    let extended_path = "PATH=/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:$HOME/.npm/bin:$HOME/.nvm/versions/node/*/bin:$HOME/.volta/bin:$PATH";
    debug_info.push_str(&format!("Extended PATH used: {}\n\n", extended_path));

    debug_info.push_str("Command availability (with extended PATH):\n");

    let commands_to_test = vec![
        (format!("{} which npx", extended_path), "npx location"),
        (format!("{} which node", extended_path), "node location"),
        (format!("{} which ccusage", extended_path), "ccusage location"),
        (format!("{} npx --version", extended_path), "npx version"),
        (format!("{} node --version", extended_path), "node version"),
        (format!("{} ccusage --version 2>&1 || echo 'not found'", extended_path), "ccusage version"),
    ];

    for (cmd, desc) in commands_to_test {
        let output = Command::new("sh").args(&["-c", &cmd]).output().await;
        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                debug_info.push_str(&format!("{}: {}\n", desc, stdout.trim()));
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if stderr.trim().is_empty() {
                    debug_info.push_str(&format!("{}: not found\n", desc));
                } else {
                    debug_info.push_str(&format!("{}: {}\n", desc, stderr.trim()));
                }
            }
            Err(e) => {
                debug_info.push_str(&format!("{}: error - {}\n", desc, e));
            }
        }
    }

    debug_info
}

async fn refresh_data(app_handle: &tauri::AppHandle) {
    IS_REFRESHING.store(true, Ordering::Relaxed);

    let (daily_entries, ccusage_available) = fetch_daily_data().await;

    let today_str = Local::now().date_naive().format("%Y-%m-%d").to_string();
    let today_cost = daily_entries.iter()
        .find(|e| e.period == today_str)
        .map(|e| e.total_cost)
        .unwrap_or(0.0);

    let title = if today_cost > 0.0 {
        format!("${:.2}", today_cost)
    } else {
        String::new()
    };

    {
        let mut cache = APP_CACHE.lock().unwrap();
        cache.daily_entries = daily_entries;
        cache.last_updated = Some(Instant::now());
        cache.ccusage_available = ccusage_available;
    }

    if let Some(tray) = app_handle.tray_by_id("main") {
        let _ = tray.set_title(Some(title));
    }

    // Notify the webview that fresh data is available.
    let _ = app_handle.emit("stats-updated", ());

    IS_REFRESHING.store(false, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// Tray icon tinting
// ---------------------------------------------------------------------------

/// Re-colour every non-transparent pixel in a PNG to (r, g, b), keeping alpha.
fn tint_png(bytes: &[u8], r: u8, g: u8, b: u8) -> Vec<u8> {
    let img = image::load_from_memory(bytes).expect("valid png");
    let mut rgba = img.to_rgba8();
    for pixel in rgba.pixels_mut() {
        if pixel[3] > 0 {
            pixel[0] = r;
            pixel[1] = g;
            pixel[2] = b;
        }
    }
    let mut buf = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .expect("png encode");
    buf
}

#[tauri::command]
async fn fetch_status_html() -> Result<String, String> {
    eprintln!("[fetch_status_html] fetching https://status.claude.com/ ...");
    let client = reqwest::Client::new();
    let response = client
        .get("https://status.claude.com/")
        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
        .send()
        .await
        .map_err(|e| {
            eprintln!("[fetch_status_html] request failed: {e:#?}");
            e.to_string()
        })?;
    eprintln!("[fetch_status_html] HTTP status: {}", response.status());
    let html = response
        .text()
        .await
        .map_err(|e| {
            eprintln!("[fetch_status_html] failed to read body: {e}");
            e.to_string()
        })?;
    eprintln!("[fetch_status_html] received {} bytes; contains marker: {}", html.len(), html.contains("var uptimeData = "));
    Ok(html)
}

#[tauri::command]
fn set_claude_status(app: tauri::AppHandle, indicator: String) {
    let Some(tray) = app.tray_by_id("main") else { return };
    let bars = include_bytes!("../icons/bars.png");

    match indicator.as_str() {
        "none" => {
            let icon = tauri::image::Image::from_bytes(bars).unwrap();
            let _ = tray.set_icon(Some(icon));
            #[cfg(target_os = "macos")]
            let _ = tray.set_icon_as_template(true);
        }
        "minor" => {
            let tinted = tint_png(bars, 255, 159, 10);
            let icon = tauri::image::Image::from_bytes(&tinted).unwrap();
            let _ = tray.set_icon(Some(icon));
            #[cfg(target_os = "macos")]
            let _ = tray.set_icon_as_template(false);
        }
        "major" | "critical" => {
            let tinted = tint_png(bars, 255, 59, 48);
            let icon = tauri::image::Image::from_bytes(&tinted).unwrap();
            let _ = tray.set_icon(Some(icon));
            #[cfg(target_os = "macos")]
            let _ = tray.set_icon_as_template(false);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Tray menu (right-click) — actions only; stats live in the popover window
// ---------------------------------------------------------------------------

fn build_action_menu(app: &tauri::AppHandle) -> Result<tauri::menu::Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    let refresh = MenuItemBuilder::with_id("refresh", "Refresh").build(app)?;
    let debug = MenuItemBuilder::with_id("debug", "Debug Info").build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit")
        .accelerator("Cmd+Q")
        .build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&refresh)
        .item(&debug)
        .separator()
        .item(&quit)
        .build()?;
    Ok(menu)
}

/// Show the popover positioned just below the tray icon, or hide it if already visible.
fn toggle_popover(app: &tauri::AppHandle, rect: tauri::Rect) {
    let Some(win) = app.get_webview_window("main") else { return };

    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        return;
    }

    // Avoid reopening immediately after a focus-loss hide triggered by this same click.
    if now_ms().saturating_sub(LAST_HIDE_MS.load(Ordering::Relaxed)) < 250 {
        return;
    }

    let scale = win.scale_factor().unwrap_or(1.0);
    let pos = rect.position.to_physical::<f64>(scale);
    let size = rect.size.to_physical::<f64>(scale);
    let win_size = win.outer_size().map(|s| (s.width as f64, s.height as f64)).unwrap_or((280.0, 430.0));

    let x = pos.x + size.width / 2.0 - win_size.0 / 2.0;
    let y = pos.y + size.height;

    let _ = win.set_position(PhysicalPosition::new(x, y));
    let _ = win.show();
    let _ = win.set_focus();
    // Make sure the webview has the latest numbers when it appears.
    let _ = app.emit("stats-updated", ());
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_stats, refresh_now, quit_app, debug_info, set_claude_status, fetch_status_html])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let app_handle = app.handle().clone();

            // Hide the popover when it loses focus (e.g. clicking elsewhere).
            if let Some(win) = app.get_webview_window("main") {
                let win_for_event = win.clone();
                win.on_window_event(move |event| {
                    if let WindowEvent::Focused(false) = event {
                        LAST_HIDE_MS.store(now_ms(), Ordering::Relaxed);
                        let _ = win_for_event.hide();
                    }
                });
            }

            // Periodic refresh every 2 minutes.
            let periodic_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(120));
                loop {
                    interval.tick().await;
                    if !IS_REFRESHING.load(Ordering::Relaxed) {
                        let should_refresh = {
                            let cache = APP_CACHE.lock().unwrap();
                            cache.last_updated.is_some()
                        };
                        if should_refresh {
                            refresh_data(&periodic_handle).await;
                        }
                    }
                }
            });

            // Build tray + initial data.
            let menu = build_action_menu(&app_handle)?;
            let tray = TrayIconBuilder::with_id("main")
                .icon(
                    tauri::image::Image::from_bytes(include_bytes!("../icons/bars.png"))
                        .unwrap()
                        .to_owned(),
                )
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quit" => app.exit(0),
                    "refresh" => {
                        let app_handle = app.app_handle().clone();
                        tauri::async_runtime::spawn(async move {
                            refresh_data(&app_handle).await;
                        });
                    }
                    "debug" => {
                        tauri::async_runtime::spawn(async move {
                            let info = get_debug_info().await;
                            println!("=== DEBUG INFO ===\n{}\n==================", info);
                            #[cfg(target_os = "macos")]
                            {
                                use std::process::Command as StdCommand;
                                let _ = StdCommand::new("osascript")
                                    .args(&[
                                        "-e",
                                        &format!(
                                            r#"display dialog "{}" buttons {{"OK"}} default button "OK" with title "CCUsage Debug Info""#,
                                            info.replace("\"", "\\\"").replace("\n", "\\n")
                                        ),
                                    ])
                                    .spawn();
                            }
                        });
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        rect,
                        ..
                    } = event
                    {
                        toggle_popover(tray.app_handle(), rect);
                    }
                })
                .build(&app_handle)?;

            app_handle.manage(Arc::new(tray));

            // Initial data refresh on startup.
            let startup_handle = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                refresh_data(&startup_handle).await;
            });

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
