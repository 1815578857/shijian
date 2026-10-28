mod credentials;
mod database;
mod icon;
mod model_client;
mod model_capabilities;
mod models;
mod parser;
mod parse_worker;
mod session;
mod validation;
mod wechat;
mod wechat_client;
mod wechat_secrets;

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

use chrono::{Local, NaiveDate, NaiveTime, SecondsFormat, Utc};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl,
    WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as AutostartManagerExt};use tauri_plugin_dialog::DialogExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

use database::Database;
use models::{
    AppSnapshot, CloseBehavior, InboxRecord, PanelBounds, RecordSource, RecordStatus, Settings,
    UnlockReminder, RecordPatch, SettingsPatch,
};

type CommandResult<T> = Result<T, String>;

fn native_test_dir() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    {
        std::env::var_os("SHIJIAN_NATIVE_TEST_DIR").map(PathBuf::from)
    }
    #[cfg(not(debug_assertions))]
    {
        None
    }
}

fn native_test_mode() -> bool {
    native_test_dir().is_some()
}

struct AppState {
    database: Mutex<Database>,
    data_dir: PathBuf,
    paused_item: CheckMenuItem<tauri::Wry>,
    panel_item: CheckMenuItem<tauri::Wry>,
    reminder_clock: Mutex<ReminderClock>,
    exiting: AtomicBool,
    wechat: Arc<wechat::WechatRuntime>,
}

impl AppState {
    fn db(&self) -> CommandResult<MutexGuard<'_, Database>> {
        self.database
            .lock()
            .map_err(|_| "本地数据库锁异常，请重新启动应用".into())
    }
}

fn notify_changed(app: &AppHandle) {
    if let (Some(state), Some(tray)) = (app.try_state::<AppState>(), app.tray_by_id("main-tray")) {
        let _ = tray.set_tooltip(Some(state.wechat.tray_label()));
    }
    if app.emit("app-state-changed", ()).is_err() {
        eprintln!("无法向窗口发送状态刷新通知");
    }
}

fn sync_panel_menu(app: &AppHandle, state: &AppState, visible: bool) {
    if state.panel_item.set_checked(visible).is_err() {
        report_error(app, "无法同步托盘面板菜单".into());
    }
}

fn sync_paused_menu(app: &AppHandle, state: &AppState, paused: bool) {
    if state.paused_item.set_checked(paused).is_err() {
        report_error(app, "无法同步托盘暂停菜单".into());
    }
}

fn apply_autostart(app: &AppHandle, enabled: bool) -> CommandResult<()> {
    // 隔离测试不写当前用户的注册表启动项。
    if native_test_mode() {
        return Ok(());
    }
    let manager = app.autolaunch();
    if enabled {
        manager
            .enable()
            .map_err(|error| format!("无法设置开机自启动：{error}"))
    } else {
        manager
            .disable()
            .map_err(|error| format!("无法关闭开机自启动：{error}"))
    }
}

fn report_error(app: &AppHandle, message: String) {
    eprintln!("{message}");
    if app.emit("app-error", &message).is_err() {
        eprintln!("无法向窗口发送错误通知");
    }
}

fn model_credential_status(result: Result<Option<String>, String>) -> (bool, Option<String>) {
    match result {
        Ok(key) => (key.is_some_and(|key| !key.is_empty()), None),
        Err(_) => (false, Some("无法读取模型凭据；本地记录仍可使用，请检查系统凭据服务或重新保存密钥".into())),
    }
}

#[tauri::command]
fn get_snapshot(app: AppHandle, state: State<'_, AppState>) -> CommandResult<AppSnapshot> {
    let (settings, records) = {
        let db = state.db()?;
        (db.settings()?, db.records()?)
    };
    let (has_model_key, model_credential_error) = model_credential_status(if native_test_mode() {
        Ok(None)
    } else {
        credentials::read(&settings.model_base_url)
    });
    let wechat = state.wechat.snapshot(&app).unwrap_or_else(|_| models::WechatSnapshot {
        status: "error".into(),
        message: "无法读取微信连接状态；本地待办仍可使用，请重试或重新连接".into(),
        ..models::WechatSnapshot::default()
    });
    Ok(AppSnapshot {
        settings,
        records,
        has_model_key,
        model_credential_error,
        data_dir: state.data_dir.to_string_lossy().into_owned(),
        runtime: "desktop",
        wechat_status: wechat.status.clone(),
        wechat,
    })
}

#[tauri::command]
async fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    patch: SettingsPatch,
) -> CommandResult<Settings> {
    let (previous, settings) = {
    let db = state.db()?;
    let previous = db.settings()?;
    let settings = validation::settings(patch.merge(previous.clone()))?;
    let model_changed = previous.model_base_url != settings.model_base_url
        || previous.model_name != settings.model_name
        || previous.reasoning_effort != settings.reasoning_effort;
    if model_changed && !model_capabilities::reasoning_options(&settings.model_base_url, &settings.model_name).contains(&settings.reasoning_effort) {
        return Err("此服务或模型不支持所选思考强度，请选择服务默认".into());
    }
    if previous.launch_at_login != settings.launch_at_login {
        apply_autostart(&app, settings.launch_at_login)?;
    }
    if previous.paused != settings.paused || previous.parsing_enabled != settings.parsing_enabled || model_changed {
        state.wechat.invalidate_locked();
        db.release_parse_claims()?;
    }
    db.save_settings(&settings)?;
    (previous, settings)
    };
    let paused_changed = previous.paused != settings.paused;
    let parsing_changed = previous.parsing_enabled != settings.parsing_enabled
        || previous.model_base_url != settings.model_base_url || previous.model_name != settings.model_name
        || previous.reasoning_effort != settings.reasoning_effort;
    let panel_changed = previous.panel_enabled != settings.panel_enabled;
    let pin_changed = previous.panel_pinned != settings.panel_pinned;
    if paused_changed || parsing_changed {
        state.wechat.reconcile(app.clone()).await?;
    }
    if panel_changed {
        if settings.panel_enabled {
            spawn_panel(&app)?;
        } else if let Some(window) = app.get_webview_window("panel") {
            let _ = window.hide();
            persist_panel_bounds(&app, &state);
        }
        sync_panel_menu(&app, &state, settings.panel_enabled);
    }
    if pin_changed {
        if let Some(window) = app.get_webview_window("panel") {
            let _ = window.set_always_on_top(settings.panel_pinned);
        }
    }
    sync_paused_menu(&app, &state, settings.paused);
    notify_changed(&app);
    Ok(settings)
}

#[tauri::command]
fn save_model_key(app: AppHandle, api_key: String, base_url: String) -> CommandResult<bool> {
    let key = validation::api_key(&api_key)?;
    credentials::save(&base_url, &key)?;
    notify_changed(&app);
    Ok(true)
}

#[tauri::command]
fn delete_model_key(app: AppHandle, state: State<'_, AppState>) -> CommandResult<()> {
    let settings = state.db()?.settings()?;
    credentials::delete(&settings.model_base_url)?;
    notify_changed(&app);
    Ok(())
}

#[tauri::command]
fn create_record(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
    original_text: String,
    due_date: Option<String>,
    due_time: Option<String>,
    priority: String,
) -> CommandResult<InboxRecord> {
    let due_date = validation::due_date(due_date)?;
    let due_time = validation::due_time(due_time, due_date.as_deref())?;
    let record = InboxRecord {
        id: uuid::Uuid::new_v4().to_string(),
        title: validation::text(&title, "标题", validation::MAX_TITLE)?,
        original_text: validation::text(&original_text, "原始文本", validation::MAX_ORIGINAL_TEXT)?,
        status: RecordStatus::Saved,
        source: RecordSource::Local,
        created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        due_date,
        priority: validation::priority(&priority)?,
        error: None,
        is_demo: false,
        status_before_archive: None,
            archived_at: None,
        due_time,
    };
    state.db()?.insert_record(&record)?;
    notify_changed(&app);
    Ok(record)
}

#[tauri::command]
fn update_record(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    patch: RecordPatch,
) -> CommandResult<InboxRecord> {
    validation::record_id(&id)?;
    let record = state.db()?.patch_record(&id, patch)?;
    notify_changed(&app);
    Ok(record)
}

#[tauri::command]
fn archive_record(app: AppHandle, state: State<'_, AppState>, id: String) -> CommandResult<()> {
    validation::record_id(&id)?;
    state.db()?.set_archived(&id, true)?;
    notify_changed(&app);
    Ok(())
}

#[tauri::command]
fn restore_record(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<InboxRecord> {
    validation::record_id(&id)?;
    let record = state.db()?.set_archived(&id, false)?;
    notify_changed(&app);
    Ok(record)
}

#[tauri::command]
fn complete_record(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    completed: bool,
) -> CommandResult<InboxRecord> {
    validation::record_id(&id)?;
    let record = state.db()?.set_completed(&id, completed)?;
    notify_changed(&app);
    Ok(record)
}

#[tauri::command]
fn delete_record(app: AppHandle, state: State<'_, AppState>, id: String) -> CommandResult<()> {
    validation::record_id(&id)?;
    state.db()?.delete_record(&id)?;
    notify_changed(&app);
    Ok(())
}

#[tauri::command]
async fn set_paused(
    app: AppHandle,
    state: State<'_, AppState>,
    paused: bool,
) -> CommandResult<Settings> {
    let settings = {
        let db = state.db()?;
        let mut settings = db.settings()?;
        settings.paused = paused;
        state.wechat.invalidate_locked();
        db.release_parse_claims()?;
        db.save_settings(&settings)?;
        settings
    };
    state.wechat.reconcile(app.clone()).await?;
    sync_paused_menu(&app, &state, paused);
    notify_changed(&app);
    Ok(settings)
}

#[tauri::command]
async fn start_wechat_login(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<models::WechatSnapshot> {
    if native_test_mode() {
        return Err("原生测试模式不会连接真实微信账号".into());
    }
    state.wechat.start_login(app).await
}

#[tauri::command]
fn submit_wechat_code(state: State<'_, AppState>, code: String) -> CommandResult<()> {
    state.wechat.submit_code(code)
}

#[tauri::command]
async fn disconnect_wechat(app: AppHandle, state: State<'_, AppState>) -> CommandResult<()> {
    if native_test_mode() {
        return Err("原生测试模式不会操作真实微信凭据".into());
    }
    state.wechat.disconnect(app).await
}

#[tauri::command]
fn retry_wechat_replies(app: AppHandle, state: State<'_, AppState>) -> CommandResult<()> {
    state.wechat.retry_replies(&app)
}

#[tauri::command]
fn open_data_dir(app: AppHandle, state: State<'_, AppState>) -> CommandResult<()> {
    // Deliberately no path argument: the frontend cannot open arbitrary files or URLs.
    app.opener()
        .open_path(state.data_dir.to_string_lossy().into_owned(), None::<&str>)
        .map_err(|_| "无法打开应用数据目录，请使用设置中显示的路径手动打开".into())
}

/// 显示或隐藏桌面面板，并把结果写回设置，主窗口开关、托盘菜单与面板始终一致。
fn set_panel_shown(app: &AppHandle, state: &AppState, visible: bool) -> CommandResult<()> {
    if visible {
        spawn_panel(app)?;
    } else if let Some(window) = app.get_webview_window("panel") {
        persist_panel_bounds(app, state);
        window.hide().map_err(|_| "无法隐藏面板".to_owned())?;
    }
    {
        let db = state.db()?;
        let mut settings = db.settings()?;
        if settings.panel_enabled != visible {
            settings.panel_enabled = visible;
            db.save_settings(&settings)?;
        }
    }
    sync_panel_menu(app, state, visible);
    notify_changed(app);
    Ok(())
}

/// 异步命令：Windows 上在同步命令里创建窗口会卡死主线程。
#[tauri::command]
async fn set_panel_visible(app: AppHandle, state: State<'_, AppState>, visible: bool) -> CommandResult<()> {
    set_panel_shown(&app, &state, visible)
}

#[tauri::command]
async fn toggle_panel(app: AppHandle, state: State<'_, AppState>) -> CommandResult<bool> {
    let visible = !app
        .get_webview_window("panel")
        .is_some_and(|window| window.is_visible().unwrap_or(false));
    set_panel_shown(&app, &state, visible)?;
    Ok(visible)
}

/// 置顶：始终浮在其他窗口上；取消置顶后恢复失焦沉到桌面层。
#[tauri::command]
async fn set_panel_pinned(app: AppHandle, state: State<'_, AppState>, pinned: bool) -> CommandResult<()> {
    {
        let db = state.db()?;
        let mut settings = db.settings()?;
        settings.panel_pinned = pinned;
        db.save_settings(&settings)?;
    }
    if let Some(window) = app.get_webview_window("panel") {
        window
            .set_always_on_top(pinned)
            .map_err(|_| "无法切换面板置顶".to_owned())?;
    }
    notify_changed(&app);
    Ok(())
}

/// 面板高度随内容自适应；宽度固定，右下角保持原地（位置随尺寸差平移）。
#[tauri::command]
fn resize_panel(app: AppHandle, width: f64, height: f64) -> CommandResult<()> {
    let Some(window) = app.get_webview_window("panel") else {
        return Ok(());
    };
    let (Ok(position), Ok(old_size)) = (window.outer_position(), window.outer_size()) else {
        return Ok(());
    };
    window
        .set_size(tauri::LogicalSize::new(width, height))
        .map_err(|error| format!("无法调整面板大小：{error}"))?;
    let Ok(new_size) = window.outer_size() else {
        return Ok(());
    };
    let x = position.x + old_size.width as i32 - new_size.width as i32;
    let y = position.y + old_size.height as i32 - new_size.height as i32;
    let _ = window.set_position(PhysicalPosition::new(x, y));
    Ok(())
}

#[tauri::command]
fn reset_panel_position(app: AppHandle, state: State<'_, AppState>) -> CommandResult<()> {
    let cleared = state.db().and_then(|db| {
        let mut settings = db.settings()?;
        settings.panel_bounds = None;
        db.save_settings(&settings)
    });
    cleared?;
    if let Some(window) = app.get_webview_window("panel") {
        place_panel_bottom_right(&window, main_window_work_area(&app));
    }
    Ok(())
}

#[tauri::command]
fn open_main_window(app: AppHandle) -> CommandResult<()> {
    show_main(&app)
}

#[tauri::command]
fn quit_app(app: AppHandle, state: State<'_, AppState>) {
    state.exiting.store(true, Ordering::SeqCst);
    state.wechat.stop_now(&app);
    app.exit(0);
}

#[tauri::command]
fn export_records(state: State<'_, AppState>) -> CommandResult<String> {
    // Serialize only the typed record fields, never settings, credential entries or raw database rows.
    serde_json::to_string_pretty(&state.db()?.records()?).map_err(|_| "无法导出收件记录".into())
}

#[tauri::command]
async fn export_records_file(app: AppHandle, state: State<'_, AppState>) -> CommandResult<String> {
    let json = export_records(state)?;
    tauri::async_runtime::spawn_blocking(move || -> CommandResult<String> {
        let mut dialog = app
            .dialog()
            .file()
            .set_title("导出拾笺收件记录")
            .add_filter("JSON 收件记录", &["json"])
            .set_file_name(format!(
                "shijian-records-{}.json",
                Utc::now().format("%Y%m%d-%H%M%S")
            ));
        if let Some(window) = app.get_webview_window("main") {
            dialog = dialog.set_parent(&window);
        }
        let Some(selected) = dialog.blocking_save_file() else {
            return Ok(String::new());
        };
        let path = selected.into_path().map_err(|_| "请选择本地文件保存位置")?;
        // No destination path comes from IPC. Writing is possible only after a native
        // save dialog returns the user's explicit selection (including overwrite approval).
        std::fs::write(&path, json.as_bytes())
            .map_err(|_| "无法保存导出文件，请检查目录权限和可用磁盘空间")?;
        Ok(path.to_string_lossy().into_owned())
    })
    .await
    .map_err(|_| "导出任务异常结束")?
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelTestResult {
    message: String,
    models: Vec<String>,
}

#[tauri::command]
async fn test_model_connection(state: State<'_, AppState>) -> CommandResult<ModelTestResult> {
    let settings = state.db()?.settings()?;
    validation::model_url(&settings.model_base_url)?.ok_or("请先填写模型服务地址")?;
    let key = credentials::read(&settings.model_base_url)?;
    let (message, models) = model_client::list_models(&settings, key).await?;
    Ok(ModelTestResult { message, models })
}

#[tauri::command]
fn model_reasoning_options(base_url: String, model_name: String) -> Vec<models::ReasoningEffort> {
    model_capabilities::reasoning_options(&base_url, &model_name)
}

#[tauri::command]
async fn discover_models(
    state: State<'_, AppState>,
    base_url: String,
    api_key: Option<String>,
    use_saved_key: bool,
) -> CommandResult<ModelTestResult> {
    let base_url = validation::normalized_model_base_url(&base_url)?;
    if base_url.is_empty() {
        return Err("请先填写模型服务地址".into());
    }
    let saved = state.db()?.settings()?;
    let key = match api_key {
        Some(key) => Some(validation::api_key(&key)?),
        None if model_capabilities::use_saved_key(&base_url, &saved.model_base_url, use_saved_key)? => credentials::read(&base_url)?,
        None => None,
    };
    let draft = Settings { model_base_url: base_url, model_name: String::new(), ..Settings::default() };
    let (message, models) = model_client::list_models(&draft, key).await?;
    Ok(ModelTestResult { message, models })
}

fn show_main(app: &AppHandle) -> CommandResult<()> {
    let window = app.get_webview_window("main").ok_or("找不到应用主窗口")?;
    window.show().map_err(|_| "无法显示应用主窗口")?;
    window.unminimize().map_err(|_| "无法还原应用主窗口")?;
    window.set_focus().map_err(|_| "无法聚焦应用主窗口")?;
    Ok(())
}

fn due_date_of(record: &InboxRecord) -> Option<NaiveDate> {
    record
        .due_date
        .as_deref()
        .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
}

fn is_open(record: &InboxRecord) -> bool {
    matches!(record.status, RecordStatus::Saved | RecordStatus::NeedsReview | RecordStatus::Failed)
}

fn short_title(record: &InboxRecord) -> String {
    format!("「{}」", record.title.chars().take(24).collect::<String>())
}

fn reminder_body(due: &[&InboxRecord], today: NaiveDate) -> String {
    let expiring = due.iter().filter(|r| due_date_of(r) == Some(today)).count();
    let overdue = due.len() - expiring;
    let mut body = match (expiring, overdue) {
        (n, 0) => format!("{n} 件待办今天到期"),
        (0, n) => format!("{n} 件待办已过截止日期"),
        (a, b) => format!("{a} 件今天到期，{b} 件已过期"),
    };
    let titles: Vec<String> = due
        .iter()
        .take(3)
        .map(|r| match (&r.due_time, due_date_of(r) == Some(today)) {
            (Some(time), true) => format!("{}{time}", short_title(r)),
            _ => short_title(r),
        })
        .collect();
    body.push('：');
    body.push_str(&titles.join("、"));
    if due.len() > 3 {
        body.push_str(&format!(" 等 {} 件", due.len()));
    }
    body
}

/// 触发这次提醒检查的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReminderTrigger {
    Startup,
    Unlock,
    Tick,
}

/// 到期汇总提醒的节奏状态；只在本次运行内有效，重启后由“启动时提醒”接上。
#[derive(Debug, Default)]
struct ReminderClock {
    /// 最近一次汇总提醒的时间戳（秒），定时重复从这里起算。
    last_summary: Option<i64>,
    /// 最近一次因解锁而提醒的日期，“每天首次解锁”据此去重。
    unlock_day: Option<NaiveDate>,
}

/// 这次触发是否应发出到期汇总提醒（有没有到期待办另算）。
fn summary_is_due(
    trigger: ReminderTrigger,
    settings: &Settings,
    clock: &ReminderClock,
    today: NaiveDate,
    now: i64,
) -> bool {
    match trigger {
        ReminderTrigger::Startup => settings.remind_on_startup,
        ReminderTrigger::Unlock => {
            settings.remind_on_unlock
                && (settings.unlock_reminder == UnlockReminder::EveryUnlock
                    || clock.unlock_day != Some(today))
        }
        // 时钟回拨时 saturating_sub 为 0，不会连续弹窗。
        ReminderTrigger::Tick => {
            settings.remind_repeat
                && clock.last_summary.is_none_or(|last| {
                    now.saturating_sub(last) >= i64::from(settings.remind_interval_minutes) * 60
                })
        }
    }
}

/// 今天设置了具体时间、且时间已到的未完成待办，返回 (记录, 去重键)。
fn timed_due(records: &[InboxRecord], today: NaiveDate, time: NaiveTime) -> Vec<(&InboxRecord, String)> {
    records
        .iter()
        .filter(|record| is_open(record) && due_date_of(record) == Some(today))
        .filter_map(|record| {
            let value = record.due_time.as_deref()?;
            let at = NaiveTime::parse_from_str(value, "%H:%M").ok()?;
            (at <= time).then(|| (record, format!("{} {value}", today.format("%Y-%m-%d"))))
        })
        .collect()
}

fn show_notification(app: &AppHandle, title: &str, body: String) -> CommandResult<()> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|error| format!("无法显示桌面提醒：{error}"))
}

fn send_reminders(
    app: &AppHandle,
    state: &AppState,
    records: &[InboxRecord],
    settings: &Settings,
    trigger: ReminderTrigger,
) -> CommandResult<()> {
    let local = Local::now();
    let today = local.date_naive();
    let now = local.timestamp();

    // 到点提醒：每条待办的每个“日期 时间”只提醒一次，不受触发方式影响。
    let mut timed = Vec::new();
    for (record, key) in timed_due(records, today, local.time()) {
        if state.db()?.last_reminder(&record.id, &key)?.is_none() {
            timed.push((record, key));
        }
    }
    if !timed.is_empty() {
        let body = timed
            .iter()
            .map(|(record, _)| format!("{} {}", record.due_time.as_deref().unwrap_or_default(), short_title(record)))
            .collect::<Vec<_>>()
            .join("、");
        show_notification(app, "拾笺 · 到点提醒", body)?;
        // 通知成功后才写日志；写入失败时下个周期会重复提醒，宁可重复也不漏。
        for (record, key) in &timed {
            state.db()?.mark_reminded(&record.id, key, today, now)?;
        }
    }

    let due: Vec<&InboxRecord> = records
        .iter()
        .filter(|record| is_open(record) && due_date_of(record).is_some_and(|date| date <= today))
        .collect();
    let mut clock = state.reminder_clock.lock().map_err(|_| "提醒状态锁异常".to_owned())?;
    if due.is_empty() || !summary_is_due(trigger, settings, &clock, today, now) {
        return Ok(());
    }
    show_notification(app, "拾笺 · 待办提醒", reminder_body(&due, today))?;
    clock.last_summary = Some(now);
    if trigger == ReminderTrigger::Unlock {
        clock.unlock_day = Some(today);
    }
    drop(clock);
    let _ = state
        .db()?
        .prune_reminder_log(&(today - chrono::Duration::days(60)).format("%Y-%m-%d").to_string());
    Ok(())
}

fn run_maintenance(app: &AppHandle, state: &AppState, trigger: ReminderTrigger) -> CommandResult<()> {
    let (settings, records) = {
        let db = state.db()?;
        (db.settings()?, db.records()?)
    };
    let cutoff = (Utc::now() - chrono::Duration::days(i64::from(settings.retention_days)))
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let cleared = state.db()?.auto_clear_archived(&cutoff)?;
    if cleared > 0 {
        notify_changed(app);
    }
    if settings.reminders_enabled && !native_test_mode() {
        send_reminders(app, state, &records, &settings, trigger)?;
    }
    Ok(())
}

#[cfg(windows)]
fn work_area_of(hwnd: *mut std::ffi::c_void) -> Option<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return None;
    }
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return None;
    }
    let RECT { left, top, right, bottom } = info.rcWork;
    Some((left, top, right, bottom))
}

#[cfg(not(windows))]
fn work_area_of(_: *mut std::ffi::c_void) -> Option<(i32, i32, i32, i32)> {
    None
}

/// 主窗口所在显示器的工作区（排除任务栏），面板默认停靠在这里的右下角。
fn main_window_work_area(app: &AppHandle) -> Option<(i32, i32, i32, i32)> {
    let main = app.get_webview_window("main")?;
    let hwnd = main.hwnd().ok()?;
    work_area_of(hwnd.0)
}

#[cfg(windows)]
fn round_window_corners(hwnd: *mut std::ffi::c_void) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
        DWM_WINDOW_CORNER_PREFERENCE,
    };
    let preference = DWMWCP_ROUND;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &preference as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
    }
}

#[cfg(not(windows))]
fn round_window_corners(_: *mut std::ffi::c_void) {}

/// 把面板放到工作区右下角；用当前实际外框尺寸计算，DPI 缩放下也精确。
fn place_panel_bottom_right(window: &tauri::WebviewWindow, area: Option<(i32, i32, i32, i32)>) {
    let Some((left, top, right, bottom)) = area else {
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let x = right - size.width as i32 - 16;
    let y = bottom - size.height as i32 - 16;
    let _ = window.set_position(PhysicalPosition::new(x.max(left), y.max(top)));
}

/// 恢复记忆位置时夹回当前工作区，显示器变更或分辨率变化后不会丢在屏幕外。
fn clamp_panel_position(window: &tauri::WebviewWindow, area: Option<(i32, i32, i32, i32)>, x: i32, y: i32) {
    let Some((left, top, right, bottom)) = area else {
        let _ = window.set_position(PhysicalPosition::new(x, y));
        return;
    };
    let Ok(size) = window.outer_size() else {
        return;
    };
    let max_x = (right - size.width as i32).max(left);
    let max_y = (bottom - size.height as i32).max(top);
    let _ = window.set_position(PhysicalPosition::new(x.clamp(left, max_x), y.clamp(top, max_y)));
}

fn spawn_panel(app: &AppHandle) -> CommandResult<()> {
    if app.get_webview_window("panel").is_some() {
        if let Some(window) = app.get_webview_window("panel") {
            let _ = window.show();
        }
        return Ok(());
    }
    let (bounds, pinned) = {
        let state = app.state::<AppState>();
        let db = state.db()?;
        let settings = db.settings()?;
        (settings.panel_bounds, settings.panel_pinned)
    };
    let window = WebviewWindowBuilder::new(app, "panel", WebviewUrl::App("index.html?view=panel".into()))
        .title("拾笺面板")
        .decorations(false)
        .skip_taskbar(true)
        // 高度由前端按内容自适应（resize_panel），不允许手动拉伸避免互相打架。
        .resizable(false)
        .visible(false)
        .focused(false)
        .always_on_top(pinned)
        .inner_size(320.0, 340.0)
        .build()
        .map_err(|error| format!("无法创建桌面待办面板：{error}"))?;
    #[cfg(windows)]
    if let Ok(handle) = window.hwnd() {
        round_window_corners(handle.0);
    }
    let area = main_window_work_area(app);
    match bounds {
        Some(bounds) => {
            let _ = window.set_size(PhysicalSize::new(bounds.width, bounds.height));
            clamp_panel_position(&window, area, bounds.x, bounds.y);
        }
        None => place_panel_bottom_right(&window, area),
    }
    let _ = window.show();
    Ok(())
}

fn persist_panel_bounds(app: &AppHandle, state: &AppState) {
    let Some(window) = app.get_webview_window("panel") else {
        return;
    };
    let (Ok(position), Ok(size)) = (window.outer_position(), window.outer_size()) else {
        return;
    };
    let bounds = PanelBounds {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };
    let result = state.db().and_then(|db| {
        let mut settings = db.settings()?;
        if settings.panel_bounds == Some(bounds) {
            return Ok(());
        }
        settings.panel_bounds = Some(bounds);
        db.save_settings(&settings)
    });
    if let Err(error) = result {
        eprintln!("无法保存面板位置：{error}");
    }
}

#[cfg(windows)]
fn sink_panel_to_desktop(window: &tauri::Window) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };
    // 桌面小组件式行为：失焦后沉到桌面层，其他窗口可以盖住它；点击会重新激活。
    if let Ok(handle) = window.hwnd() {
        unsafe {
            SetWindowPos(handle.0, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

#[cfg(not(windows))]
fn sink_panel_to_desktop(_: &tauri::Window) {}

fn start_maintenance(app: AppHandle) {
    let unlock = session::unlock_notify();
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut first = true;
        loop {
            // interval 的第一次 tick 立即完成，用作“启动时提醒”。
            let trigger = tokio::select! {
                _ = ticker.tick() => if std::mem::take(&mut first) { ReminderTrigger::Startup } else { ReminderTrigger::Tick },
                _ = unlock.notified() => ReminderTrigger::Unlock,
            };
            let Some(state) = app.try_state::<AppState>() else {
                continue;
            };
            if state.exiting.load(Ordering::SeqCst) {
                return;
            }
            if let Err(error) = run_maintenance(&app, &state, trigger) {
                eprintln!("例行维护失败：{error}");
            }
        }
    });
}

fn tray_icon() -> Image<'static> {
    let (rgba, size) = icon::rgba();
    Image::new_owned(rgba, size, size)
}

pub fn run() {
    let builder = tauri::Builder::default();
    let builder = if native_test_mode() {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Err(error) = show_main(app) {
                report_error(app, error);
            }
        }))
    };
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let data_dir = native_test_dir().unwrap_or(app.path().app_data_dir()?);
            std::fs::create_dir_all(&data_dir)?;
            let database =
                Database::open(&data_dir.join("inbox.sqlite3")).map_err(std::io::Error::other)?;
            let settings = database.settings().map_err(std::io::Error::other)?;
            let open = MenuItem::with_id(app, "open", "打开拾笺", true, None::<&str>)?;
            let paused = CheckMenuItem::with_id(
                app,
                "pause",
                "暂停接收微信消息",
                true,
                settings.paused,
                None::<&str>,
            )?;
            let panel = CheckMenuItem::with_id(
                app,
                "panel",
                "桌面待办面板",
                true,
                settings.panel_enabled,
                None::<&str>,
            )?;
            let quit = MenuItem::with_id(app, "quit", "退出拾笺", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &panel, &paused, &quit])?;
            let wechat = wechat::WechatRuntime::new().map_err(std::io::Error::other)?;
            app.manage(AppState {
                database: Mutex::new(database),
                data_dir,
                paused_item: paused,
                panel_item: panel,
                reminder_clock: Mutex::new(ReminderClock::default()),
                exiting: AtomicBool::new(false),
                wechat: wechat.clone(),
            });
            if !native_test_mode() {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    wechat.restore(app_handle).await;
                });
                parse_worker::start(app.handle().clone());
                start_maintenance(app.handle().clone());
                session::spawn_unlock_watcher();
            }
            if settings.panel_enabled {
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = spawn_panel(&app_handle) {
                        eprintln!("{error}");
                    }
                });
            }
            TrayIconBuilder::with_id("main-tray")
                .icon(tray_icon())
                .tooltip("拾笺 · 微信状态请在主窗口查看")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    let state = app.state::<AppState>();
                    match event.id.as_ref() {
                        "open" => {
                            if let Err(error) = show_main(app) {
                                report_error(app, error);
                            }
                        }
                        "pause" => {
                            let result = (|| -> CommandResult<Settings> {
                                let db = state.db()?;
                                let mut settings = db.settings()?;
                                settings.paused = !settings.paused;
                                state.wechat.invalidate_locked();
                                db.release_parse_claims()?;
                                db.save_settings(&settings)?;
                                Ok(settings)
                            })();
                            match result {
                                Ok(settings) => {
                                    sync_paused_menu(app, &state, settings.paused);
                                    let runtime = state.wechat.clone();
                                    let app = app.clone();
                                    tauri::async_runtime::spawn(async move {
                                        if let Err(error) = runtime.reconcile(app.clone()).await {
                                            report_error(&app, error);
                                        }
                                        notify_changed(&app);
                                    });
                                }
                                Err(error) => {
                                    // Native check items toggle before the callback; undo a failed write visibly.
                                    match state.db().and_then(|db| db.settings()) {
                                        Ok(settings) => {
                                            sync_paused_menu(app, &state, settings.paused)
                                        }
                                        Err(read_error) => report_error(app, read_error),
                                    }
                                    report_error(app, error);
                                }
                            }
                        }
                        "panel" => {
                            // 原生勾选项在回调前已切换，以实际窗口状态为准取反。
                            let visible = !app
                                .get_webview_window("panel")
                                .is_some_and(|window| window.is_visible().unwrap_or(false));
                            // 不在菜单回调（主线程）里直接建窗口，避免阻塞事件循环。
                            let app = app.clone();
                            tauri::async_runtime::spawn(async move {
                                let state = app.state::<AppState>();
                                if let Err(error) = set_panel_shown(&app, &state, visible) {
                                    report_error(&app, error);
                                }
                            });
                        }
                        "quit" => {
                            state.exiting.store(true, Ordering::SeqCst);
                            state.wechat.stop_now(app);
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        }
                    ) {
                        if let Err(error) = show_main(tray.app_handle()) {
                            report_error(tray.app_handle(), error);
                        }
                    }
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "panel" {
                let app = window.app_handle();
                match event {
                    WindowEvent::CloseRequested { api, .. } => {
                        // 面板只隐藏不退出；退出必须通过托盘或主窗口。
                        api.prevent_close();
                        let state = app.state::<AppState>();
                        if let Err(error) = set_panel_shown(app, &state, false) {
                            report_error(app, error);
                        }
                    }
                    WindowEvent::Focused(false) => {
                        let state = app.state::<AppState>();
                        let pinned = state.db().and_then(|db| db.settings()).is_ok_and(|s| s.panel_pinned);
                        if !pinned {
                            sink_panel_to_desktop(window);
                        }
                        persist_panel_bounds(app, &state);
                    }
                    _ => {}
                }
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let state = app.state::<AppState>();
                if state.exiting.load(Ordering::SeqCst) {
                    return;
                }
                let result = state.db().and_then(|db| db.settings());
                match result {
                    Ok(settings) if settings.close_behavior == CloseBehavior::Quit => {
                        state.exiting.store(true, Ordering::SeqCst);
                        state.wechat.stop_now(app);
                        app.exit(0);
                    }
                    Ok(_) => {
                        api.prevent_close();
                        if window.hide().is_err() {
                            report_error(app, "无法将窗口隐藏到托盘".into());
                        }
                    }
                    Err(error) => {
                        api.prevent_close();
                        report_error(app, error);
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            save_settings,
            save_model_key,
            delete_model_key,
            create_record,
            update_record,
            archive_record,
            restore_record,
            complete_record,
            delete_record,
            toggle_panel,
            set_panel_visible,
            set_panel_pinned,
            open_main_window,
            reset_panel_position,
            resize_panel,
            set_paused,
            open_data_dir,
            quit_app,
            export_records,
            export_records_file,
            test_model_connection,
            discover_models,
            model_reasoning_options,
            start_wechat_login,
            submit_wechat_code,
            disconnect_wechat,
            retry_wechat_replies,
        ])
        .run(tauri::generate_context!())
        .expect("拾笺原生应用启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{RecordSource, UnlockReminder};

    #[test]
    fn credential_diagnostics_distinguish_missing_and_error_without_exposing_secrets() {
        assert_eq!(model_credential_status(Ok(None)), (false, None));
        assert_eq!(model_credential_status(Ok(Some("key".into()))), (true, None));
        let (available, error) = model_credential_status(Err("private-key-sensitive-details".into()));
        assert!(!available);
        let error = error.unwrap();
        assert!(error.contains("本地记录仍可使用"));
        assert!(!error.contains("private-key"));
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn startup_and_unlock_switches_control_summary_reminders() {
        let today = date(2026, 10, 24);
        let defaults = Settings::default();
        let fresh = ReminderClock::default();
        assert!(summary_is_due(ReminderTrigger::Startup, &defaults, &fresh, today, 0));
        assert!(summary_is_due(ReminderTrigger::Unlock, &defaults, &fresh, today, 0));
        assert!(!summary_is_due(ReminderTrigger::Tick, &defaults, &fresh, today, 0), "定时重复默认关闭");
        let off = Settings { remind_on_startup: false, remind_on_unlock: false, ..Settings::default() };
        assert!(!summary_is_due(ReminderTrigger::Startup, &off, &fresh, today, 0));
        assert!(!summary_is_due(ReminderTrigger::Unlock, &off, &fresh, today, 0));
    }

    #[test]
    fn first_unlock_of_day_reminds_once_while_every_unlock_always_reminds() {
        let today = date(2026, 10, 24);
        let reminded_today = ReminderClock { last_summary: Some(10), unlock_day: Some(today) };
        let first = Settings::default();
        assert!(!summary_is_due(ReminderTrigger::Unlock, &first, &reminded_today, today, 20));
        assert!(summary_is_due(ReminderTrigger::Unlock, &first, &reminded_today, date(2026, 10, 25), 20));
        let every = Settings { unlock_reminder: UnlockReminder::EveryUnlock, ..Settings::default() };
        assert!(summary_is_due(ReminderTrigger::Unlock, &every, &reminded_today, today, 20));
    }

    #[test]
    fn repeat_interval_counts_from_last_summary_and_ignores_clock_skew() {
        let today = date(2026, 10, 24);
        let repeat = Settings { remind_repeat: true, remind_interval_minutes: 90, ..Settings::default() };
        let now = 1_800_000_000i64;
        assert!(summary_is_due(ReminderTrigger::Tick, &repeat, &ReminderClock::default(), today, now));
        let recent = ReminderClock { last_summary: Some(now - 90 * 60 + 1), unlock_day: None };
        assert!(!summary_is_due(ReminderTrigger::Tick, &repeat, &recent, today, now));
        let elapsed = ReminderClock { last_summary: Some(now - 90 * 60), unlock_day: None };
        assert!(summary_is_due(ReminderTrigger::Tick, &repeat, &elapsed, today, now));
        let future = ReminderClock { last_summary: Some(now + 100), unlock_day: None };
        assert!(!summary_is_due(ReminderTrigger::Tick, &repeat, &future, today, now));
    }

    #[test]
    fn timed_reminders_fire_only_for_open_records_due_today_after_their_time() {
        let record = |id: &str, due: &str, time: &str, status: RecordStatus| InboxRecord {
            id: id.into(),
            title: id.into(),
            original_text: id.into(),
            status,
            source: RecordSource::Local,
            created_at: "2026-10-24T00:00:00.000Z".into(),
            due_date: Some(due.into()),
            priority: crate::models::Priority::Normal,
            error: None,
            is_demo: false,
            status_before_archive: None,
            archived_at: None,
            due_time: Some(time.into()),
        };
        let records = vec![
            record("past", "2026-10-24", "09:00", RecordStatus::Saved),
            record("later", "2026-10-24", "15:00", RecordStatus::Saved),
            record("done", "2026-10-24", "08:00", RecordStatus::Completed),
            record("yesterday", "2026-10-23", "08:00", RecordStatus::Saved),
        ];
        let due = timed_due(&records, date(2026, 10, 24), NaiveTime::from_hms_opt(10, 0, 0).unwrap());
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0.id, "past");
        assert_eq!(due[0].1, "2026-10-24 09:00");
    }
}
