use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use chrono::{SecondsFormat, Utc};
use qrcode::{render::svg, QrCode};
use reqwest::Url;
use serde_json::Value;
use tauri::{AppHandle, Manager};
use tokio::{
    sync::{Mutex as AsyncMutex, Notify},
    task::JoinHandle,
};

use crate::{
    database::Database,
    models::WechatSnapshot,
    notify_changed,
    wechat_client::{self, ApiError, Session, WechatClient},
    wechat_secrets, AppState,
};

struct View {
    snapshot: WechatSnapshot,
    session: Option<Session>,
    verify_code: Option<String>,
}

pub struct WechatRuntime {
    view: Mutex<View>,
    lifecycle: AsyncMutex<()>,
    task: Mutex<Option<JoinHandle<()>>>,
    generation: AtomicU64,
    parse_allowed: std::sync::atomic::AtomicBool,
    parse_cancel: tokio::sync::watch::Sender<u64>,
    pub(crate) parse_running: AsyncMutex<()>,
    wake: Notify,
    client: WechatClient,
}

fn database<T>(
    app: &AppHandle,
    operation: impl FnOnce(&mut Database) -> Result<T, String>,
) -> Result<T, String> {
    let state = app.state::<AppState>();
    let mut db = state.db()?;
    operation(&mut db)
}

fn is_paused(app: &AppHandle) -> Result<bool, String> {
    database(app, |db| Ok(db.settings()?.paused))
}

impl WechatRuntime {
    pub fn new() -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            view: Mutex::new(View {
                snapshot: WechatSnapshot::default(),
                session: None,
                verify_code: None,
            }),
            lifecycle: AsyncMutex::new(()),
            task: Mutex::new(None),
            generation: AtomicU64::new(0),
            parse_allowed: std::sync::atomic::AtomicBool::new(false),
            parse_cancel: tokio::sync::watch::channel(0).0,
            parse_running: AsyncMutex::new(()),
            wake: Notify::new(),
            client: WechatClient::new()?,
        }))
    }

    pub fn snapshot(&self, app: &AppHandle) -> Result<WechatSnapshot, String> {
        let mut snapshot = self
            .view
            .lock()
            .map_err(|_| "微信状态锁异常")?
            .snapshot
            .clone();
        if let Some(account) = snapshot.account_id.as_deref() {
            let stats = database(app, |db| db.wechat_stats(account))?;
            snapshot.pending_replies = stats.pending;
            snapshot.failed_replies = stats.failed;
            snapshot.last_received_at = stats.last_received;
        }
        Ok(snapshot)
    }

    fn update(&self, app: &AppHandle, status: &str, message: impl Into<String>) {
        if let Ok(mut view) = self.view.lock() {
            view.snapshot.status = status.into();
            view.snapshot.message = message.into();
            if !matches!(status, "waiting_scan" | "scanned" | "need_verifycode") {
                view.snapshot.qr_code_svg = None;
                view.snapshot.qr_expires_at = None;
            }
        }
        notify_changed(app);
    }

    pub fn tray_label(&self) -> String {
        let status = self
            .view
            .lock()
            .map(|view| view.snapshot.status.clone())
            .unwrap_or_default();
        let label = match status.as_str() {
            "connected" => "微信接收正常",
            "paused" => "已暂停接收微信消息",
            "waiting_scan" | "scanned" | "need_verifycode" | "connecting" => "正在连接微信",
            "reconnecting" => "微信正在重连",
            "error" => "微信连接异常",
            "expired" => "微信二维码已过期",
            _ => "微信尚未连接",
        };
        format!("拾笺 · {label}")
    }

    // Called under AppState's database mutex, also used by parsing commits.
    pub(crate) fn invalidate_locked(&self) {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.parse_cancel.send_replace(generation);
        self.parse_allowed.store(false, Ordering::SeqCst);
        self.wake.notify_waiters();
    }

    pub(crate) fn parse_context(&self) -> Result<Option<(String, u64)>, String> {
        let view = self.view.lock().map_err(|_| "微信状态锁异常")?;
        if !self.parse_allowed.load(Ordering::SeqCst) { return Ok(None); }
        Ok(view.session.as_ref().map(|session| (session.bot_id.clone(), self.generation.load(Ordering::SeqCst))))
    }

    pub(crate) fn parse_subscription(&self) -> tokio::sync::watch::Receiver<u64> {
        self.parse_cancel.subscribe()
    }

    pub fn stop_now(&self, app: &AppHandle) {
        if let Ok(db) = app.state::<AppState>().db() {
            self.invalidate_locked();
            let _ = db.release_parse_claims();
        }
        if let Ok(mut task) = self.task.lock() {
            if let Some(task) = task.take() {
                task.abort();
            }
        }
        self.wake.notify_waiters();
    }

    async fn stop_previous(&self, app: &AppHandle) -> Result<(), String> {
        database(app, |db| { self.invalidate_locked(); db.release_parse_claims() })?;
        self.abort_previous().await;
        Ok(())
    }

    async fn abort_previous(&self) {
        let previous = self.task.lock().ok().and_then(|mut task| task.take());
        if let Some(task) = previous {
            task.abort();
            let _ = task.await;
        }
        let _parsing_stopped = self.parse_running.lock().await;
        if let Ok(mut view) = self.view.lock() {
            view.verify_code = None;
        }
    }

    fn spawn(self: &Arc<Self>, app: AppHandle, session: Option<Session>) {
        let runtime = self.clone();
        let generation = self.generation.load(Ordering::SeqCst);
        let task = tokio::spawn(async move {
            if let Some(session) = session {
                runtime.receive(app, session, generation).await;
            } else {
                runtime.login(app, generation).await;
            }
        });
        if let Ok(mut slot) = self.task.lock() {
            *slot = Some(task);
        }
    }

    pub async fn restore(self: &Arc<Self>, app: AppHandle) {
        let _guard = self.lifecycle.lock().await;
        if self.generation.load(Ordering::SeqCst) != 0 || crate::native_test_mode() {
            return;
        }
        match wechat_secrets::read_session() {
            Ok(Some(session)) => {
                if let Ok(mut view) = self.view.lock() {
                    view.snapshot.account_id = Some(session.bot_id.clone());
                    view.session = Some(session.clone());
                }
                if is_paused(&app).unwrap_or(true) {
                    self.update(&app, "paused", "已暂停接收微信消息");
                } else {
                    self.update(&app, "reconnecting", "正在验证已保存的微信会话");
                    self.spawn(app, Some(session));
                }
            }
            Ok(None) => {
                if is_paused(&app).unwrap_or(false) {
                    self.update(&app, "paused", "已暂停，尚未连接微信");
                }
            }
            Err(error) => self.update(&app, "error", error),
        }
    }

    pub async fn start_login(self: &Arc<Self>, app: AppHandle) -> Result<WechatSnapshot, String> {
        let _guard = self.lifecycle.lock().await;
        if is_paused(&app)? {
            return Err("请先取消暂停，再开始微信扫码连接".into());
        }
        self.stop_previous(&app).await?;
        database(&app, |db| db.clear_wechat_contexts())?;
        {
            let mut view = self.view.lock().map_err(|_| "微信状态锁异常")?;
            view.session = None;
            view.snapshot = WechatSnapshot::default();
        }
        self.update(
            &app,
            "connecting",
            "正在向微信申请二维码；仅在你主动连接后请求",
        );
        self.spawn(app.clone(), None);
        self.snapshot(&app)
    }

    pub async fn reconcile(self: &Arc<Self>, app: AppHandle) -> Result<(), String> {
        let _guard = self.lifecycle.lock().await;
        self.stop_previous(&app).await?;
        let session = self
            .view
            .lock()
            .map_err(|_| "微信状态锁异常")?
            .session
            .clone();
        if is_paused(&app)? {
            self.update(&app, "paused", "已暂停接收微信消息");
        } else if let Some(session) = session {
            self.update(&app, "reconnecting", "正在恢复微信接收并验证会话");
            self.spawn(app, Some(session));
        } else {
            self.update(&app, "not_connected", "尚未连接微信，请主动扫码");
        }
        Ok(())
    }

    pub async fn disconnect(self: &Arc<Self>, app: AppHandle) -> Result<(), String> {
        let _guard = self.lifecycle.lock().await;
        self.stop_previous(&app).await?;
        {
            let mut view = self.view.lock().map_err(|_| "微信状态锁异常")?;
            view.session = None;
            view.snapshot = WechatSnapshot::default();
        }
        let cleared = database(&app, |db| db.clear_wechat_contexts());
        let removed = wechat_secrets::delete_session();
        if let Err(error) = cleared.and(removed) {
            self.update(&app, "error", error.clone());
            return Err(error);
        }
        self.update(
            &app,
            "not_connected",
            "已断开本应用的微信会话；已保存记录保留，微信端绑定需在微信管理",
        );
        Ok(())
    }

    pub fn submit_code(&self, code: String) -> Result<(), String> {
        if !(4..=12).contains(&code.len()) || !code.bytes().all(|b| b.is_ascii_digit()) {
            return Err("请输入手机微信显示的 4–12 位数字验证码".into());
        }
        let mut view = self.view.lock().map_err(|_| "微信状态锁异常")?;
        if view.snapshot.status != "need_verifycode" {
            return Err("当前没有等待输入验证码的微信登录".into());
        }
        view.verify_code = Some(code);
        drop(view);
        self.wake.notify_one();
        Ok(())
    }

    pub fn retry_replies(&self, app: &AppHandle) -> Result<(), String> {
        let view = self.view.lock().map_err(|_| "微信状态锁异常")?;
        let account = view
            .session
            .as_ref()
            .map(|s| s.bot_id.clone())
            .ok_or("请先连接微信，再重试回执")?;
        drop(view);
        database(app, |db| db.retry_replies(&account))?;
        self.wake.notify_one();
        notify_changed(app);
        Ok(())
    }

    fn active(&self, generation: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == generation
    }

    async fn login(self: Arc<Self>, app: AppHandle, generation: u64) {
        if let Err(error) = self.login_inner(&app, generation).await {
            if self.active(generation) {
                self.update(&app, "error", error);
            }
        }
    }

    async fn login_inner(self: &Arc<Self>, app: &AppHandle, generation: u64) -> Result<(), String> {
        let response = self.client.qr().await.map_err(|e| e.message())?;
        let qrcode = response
            .get("qrcode")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 8192)
            .ok_or("微信未返回有效的扫码会话")?;
        let content = response
            .get("qrcode_img_content")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= 3000)
            .ok_or("微信未返回有效的二维码内容")?;
        let qr = QrCode::new(content.as_bytes()).map_err(|_| "微信返回的二维码内容无法编码")?;
        let svg = qr
            .render::<svg::Color>()
            .min_dimensions(240, 240)
            .quiet_zone(true)
            .build();
        let deadline = Instant::now() + Duration::from_secs(300);
        {
            let mut view = self.view.lock().map_err(|_| "微信状态锁异常")?;
            view.snapshot.qr_code_svg = Some(svg);
            view.snapshot.qr_expires_at = Some(
                (Utc::now() + chrono::Duration::minutes(5))
                    .to_rfc3339_opts(SecondsFormat::Secs, true),
            );
        }
        self.update(
            app,
            "waiting_scan",
            "请用手机微信扫码，并在手机端确认；账号准入以微信服务端为准",
        );
        let mut base = Url::parse(wechat_client::LOGIN_BASE).expect("fixed URL");
        let mut redirects = 0;
        let mut failures = 0;
        let mut verify: Option<String> = None;
        while self.active(generation) && Instant::now() < deadline {
            let response = match self
                .client
                .qr_status(&base, qrcode, verify.as_deref())
                .await
            {
                Ok(response) => {
                    failures = 0;
                    response
                }
                Err(ApiError::Timeout) => continue,
                Err(error) => {
                    failures += 1;
                    if failures >= 3 {
                        return Err(error.message());
                    }
                    tokio::time::sleep(Duration::from_secs(failures * 2)).await;
                    continue;
                }
            };
            if !self.active(generation) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                break;
            }
            match qr_state(&response)? {
                QrState::Wait => {}
                QrState::Scanned => {
                    verify = None;
                    self.update(app, "scanned", "已扫码，请在手机微信确认连接");
                }
                QrState::NeedCode => {
                    self.update(
                        app,
                        "need_verifycode",
                        if verify.is_some() {
                            "验证码未通过，请重新输入手机微信显示的数字"
                        } else {
                            "请输入手机微信显示的数字验证码"
                        },
                    );
                    verify = None;
                    loop {
                        let notified = self.wake.notified();
                        let code = self
                            .view
                            .lock()
                            .map_err(|_| "微信状态锁异常")?
                            .verify_code
                            .take();
                        if let Some(code) = code {
                            verify = Some(code);
                            break;
                        }
                        if tokio::time::timeout(
                            deadline.saturating_duration_since(Instant::now()),
                            notified,
                        )
                        .await
                        .is_err()
                        {
                            break;
                        }
                    }
                    if verify.is_none() {
                        break;
                    }
                    continue;
                }
                QrState::Expired => {
                    self.update(app, "expired", "微信二维码已过期，请点击重新扫码");
                    return Ok(());
                }
                QrState::Blocked => return Err("验证码错误次数过多，请稍后重新扫码".into()),
                QrState::AlreadyBound => {
                    return Err(
                        "微信返回已绑定，但未签发可验证的新凭据；请在微信端检查绑定后重新扫码"
                            .into(),
                    )
                }
                QrState::Redirect => {
                    redirects += 1;
                    if redirects > 3 {
                        return Err("微信扫码跳转次数过多，已停止连接".into());
                    }
                    base = wechat_client::redirected_base(
                        response
                            .get("redirect_host")
                            .and_then(Value::as_str)
                            .ok_or("微信扫码跳转缺少地址")?,
                    )?;
                }
                QrState::Confirmed => {
                    let session = wechat_client::confirmed_session(&response)?;
                    wechat_secrets::save_session(&session)?;
                    {
                        let mut view = self.view.lock().map_err(|_| "微信状态锁异常")?;
                        view.snapshot.account_id = Some(session.bot_id.clone());
                        view.session = Some(session.clone());
                    }
                    self.update(
                        app,
                        "reconnecting",
                        "手机已确认，正在验证消息接收；尚未执行模型解析",
                    );
                    self.receive(app.clone(), session, generation).await;
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        if self.active(generation) {
            self.update(app, "expired", "微信扫码已超时，请重新生成二维码");
        }
        Ok(())
    }

    async fn receive(&self, app: AppHandle, session: Session, generation: u64) {
        let mut failures = 0u32;
        let mut timeout_ms = 35_000;
        let mut verified = false;
        while self.active(generation) {
            if is_paused(&app).unwrap_or(true) {
                return;
            }
            let result = async {
                if verified {
                    self.flush_replies(&app, &session, generation).await?;
                }
                let cursor = database(&app, |db| db.wechat_cursor(&session.bot_id))
                    .map_err(ApiError::Failure)?;
                let response = tokio::select! {
                    result=self.client.updates(&session, &cursor, timeout_ms)=>result?,
                    _=self.wake.notified()=> {
                        if verified { self.flush_replies(&app,&session,generation).await?; }
                        return Ok(());
                    }
                };
                timeout_ms = response
                    .get("longpolling_timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(35_000)
                    .clamp(5_000, 60_000);
                if !self.active(generation) || is_paused(&app).unwrap_or(true) {
                    return Ok(());
                }
                database(&app, |db| {
                    if !self.active(generation) || db.settings()?.paused { return Ok(()); }
                    db.ingest_wechat(&session, &response)?;
                    self.parse_allowed.store(true, Ordering::SeqCst);
                    Ok(())
                }).map_err(ApiError::Failure)?;
                if !self.active(generation) { return Ok(()); }
                verified = true;
                self.update(
                    &app,
                    "connected",
                    "微信文本接收正常；原文保存到本地，尚未解析日期或设置提醒",
                );
                self.flush_replies(&app, &session, generation).await?;
                Ok::<(), ApiError>(())
            }
            .await;
            match result {
                Ok(()) => failures = 0,
                Err(ApiError::SessionExpired) => {
                    let _ = database(&app, |db| { self.invalidate_locked(); db.release_parse_claims() });
                    if let Ok(mut view) = self.view.lock() {
                        view.session = None;
                    }
                    self.update(&app, "error", ApiError::SessionExpired.message());
                    return;
                }
                Err(ApiError::Timeout) => {}
                Err(error) => {
                    failures = failures.saturating_add(1);
                    self.update(
                        &app,
                        "reconnecting",
                        format!("{}；稍后自动重试", error.message()),
                    );
                    tokio::select! {
                        _=tokio::time::sleep(Duration::from_secs(2u64.saturating_pow(failures.min(5)).min(30)))=>{},
                        _=self.wake.notified()=>{},
                    }
                }
            }
            if self.active(generation) {
                tokio::select! { _=tokio::time::sleep(Duration::from_millis(500))=>{}, _=self.wake.notified()=>{} }
            }
        }
    }

    async fn flush_replies(
        &self,
        app: &AppHandle,
        session: &Session,
        generation: u64,
    ) -> Result<(), ApiError> {
        for _ in 0..20 {
            if !self.active(generation) || is_paused(app).unwrap_or(true) {
                return Ok(());
            }
            let Some(job) =
                database(app, |db| db.next_reply(&session.bot_id)).map_err(ApiError::Failure)?
            else {
                break;
            };
            if job.attempts >= 3 {
                database(app, |db| db.finish_reply(&job.client_id, false, true))
                    .map_err(ApiError::Failure)?;
                continue;
            }
            let context = match wechat_secrets::unprotect_context(&job.context) {
                Ok(context) => context,
                Err(_) => {
                    database(app, |db| db.finish_reply(&job.client_id, false, true))
                        .map_err(ApiError::Failure)?;
                    continue;
                }
            };
            let claimed = database(app, |db| {
                if !self.active(generation) || db.settings()?.paused { return Ok(false); }
                db.start_reply_attempt(&job.client_id)
            }).map_err(ApiError::Failure)?;
            if !claimed { continue; }
            // No database lock crosses the network await. Once submitted, a send
            // cannot be recalled by a subsequent local deletion.
            let result = self
                .client
                .send(
                    session,
                    &job.recipient,
                    &context,
                    &job.client_id,
                    &job.reply,
                )
                .await;
            database(app, |db| {
                db.finish_reply(&job.client_id, result.is_ok(), false)
            })
            .map_err(ApiError::Failure)?;
            notify_changed(app);
            if matches!(result, Err(ApiError::SessionExpired)) {
                return Err(ApiError::SessionExpired);
            }
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
enum QrState {
    Wait,
    Scanned,
    NeedCode,
    Confirmed,
    Expired,
    Blocked,
    Redirect,
    AlreadyBound,
}
fn qr_state(value: &Value) -> Result<QrState, String> {
    match value.get("status").and_then(Value::as_str) {
        Some("wait") => Ok(QrState::Wait),
        Some("scaned") => Ok(QrState::Scanned),
        Some("need_verifycode") => Ok(QrState::NeedCode),
        Some("confirmed") => Ok(QrState::Confirmed),
        Some("expired") => Ok(QrState::Expired),
        Some("verify_code_blocked") => Ok(QrState::Blocked),
        Some("scaned_but_redirect") => Ok(QrState::Redirect),
        Some("binded_redirect") => Ok(QrState::AlreadyBound),
        _ => Err("微信返回了未支持的扫码状态，未标记为已连接".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn maps_every_documented_qr_state_and_rejects_unknown() {
        for (status, expected) in [
            ("wait", QrState::Wait),
            ("scaned", QrState::Scanned),
            ("need_verifycode", QrState::NeedCode),
            ("confirmed", QrState::Confirmed),
            ("expired", QrState::Expired),
            ("verify_code_blocked", QrState::Blocked),
            ("scaned_but_redirect", QrState::Redirect),
            ("binded_redirect", QrState::AlreadyBound),
        ] {
            assert_eq!(qr_state(&json!({"status":status})).unwrap(), expected);
        }
        assert!(qr_state(&json!({"status":"unknown"})).is_err());
    }
    #[tokio::test]
    async fn replacement_and_pause_abort_previous_work() {
        let runtime = WechatRuntime::new().unwrap();
        let count = Arc::new(AtomicU64::new(0));
        let running = count.clone();
        let task = tokio::spawn(async move {
            loop {
                running.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        });
        *runtime.task.lock().unwrap() = Some(task);
        tokio::time::sleep(Duration::from_millis(10)).await;
        runtime.invalidate_locked();
        runtime.abort_previous().await;
        let stopped = count.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(stopped, count.load(Ordering::SeqCst));
        assert!(runtime.task.lock().unwrap().is_none());
        assert!(!runtime.active(0));
    }
    #[test]
    fn verify_codes_are_validated_at_boundary() {
        let runtime = WechatRuntime::new().unwrap();
        for value in ["123", "1234567890123", "abcd", "1234\n"] {
            assert!(runtime.submit_code(value.into()).is_err());
        }
        assert!(runtime.submit_code("1234".into()).is_err());
        runtime.view.lock().unwrap().snapshot.status = "need_verifycode".into();
        assert!(runtime.submit_code("1234".into()).is_ok());
        let public = serde_json::to_string(&runtime.view.lock().unwrap().snapshot).unwrap();
        assert!(!public.contains("1234"));
    }
}
