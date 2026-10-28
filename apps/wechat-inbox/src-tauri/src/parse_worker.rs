use std::sync::atomic::Ordering;

use chrono::{NaiveDate, Utc};
use tauri::{AppHandle, Manager};

use crate::{credentials, database::Database, model_client, notify_changed, parser, report_error, AppState};

pub(crate) fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(15));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let state = app.state::<AppState>();
            if state.exiting.load(Ordering::SeqCst) { return; }
            if let Err(error) = cycle(&app, &state).await {
                report_error(&app, format!("模型解析循环异常：{error}"));
            }
        }
    });
}

async fn cycle(app: &AppHandle, state: &AppState) -> Result<(), String> {
    // Lifecycle cancellation waits for this guard without holding the database gate.
    let _running = state.wechat.parse_running.lock().await;
    let mut cancelled = state.wechat.parse_subscription();
    let (settings, job, record, generation, finalized) = {
        let db = state.db()?;
        let settings = db.settings()?;
        if !settings.parsing_enabled || settings.paused || state.exiting.load(Ordering::SeqCst) { return Ok(()); }
        let Some((account, generation)) = state.wechat.parse_context()? else { return Ok(()); };
        let (job, finalized) = db.claim_parse_job_with_changes(&account, Utc::now().timestamp())?;
        let Some(job) = job else {
            drop(db);
            if finalized { notify_changed(app); }
            return Ok(());
        };
        let record = db.record(&job.record_id)?;
        (settings, job, record, generation, finalized)
    };
    if finalized { notify_changed(app); }
    let reference = NaiveDate::parse_from_str(&job.reference_date, "%Y-%m-%d");
    let request = async {
        let date = reference.map_err(|_| parser::ParseError::Permanent("消息日期基准无效，请手动确认".into()))?;
        let key = credentials::read(&settings.model_base_url)
            .map_err(|_| parser::ParseError::Permanent("无法读取模型凭据，请检查系统凭据服务或重新保存密钥".into()))?;
        model_client::parse_intent(&settings, key, &record.original_text, date).await
    };
    let Some(result) = cancellable(generation, &mut cancelled, request).await else { return Ok(()); };
    let result = match result {
        Ok(intent) => {
            let mut updated = record;
            match parser::apply(&mut updated, intent, parser::today()) {
                Ok(summary) => Ok((updated, summary)),
                Err(reason) => Err((reason, true)),
            }
        }
        Err(error) => Err((error.message().to_owned(), error.is_permanent())),
    };
    let changed = {
        let db = state.db()?;
        let settings = db.settings()?;
        if state.exiting.load(Ordering::SeqCst) || settings.paused || !settings.parsing_enabled
            || state.wechat.parse_context()? != Some((job.account.clone(), generation)) {
            return Ok(());
        }
        commit(&db, &job, result)?
    };
    if changed { notify_changed(app); }
    Ok(())
}

async fn cancellable<T>(generation: u64, cancelled: &mut tokio::sync::watch::Receiver<u64>, request: impl std::future::Future<Output = T>) -> Option<T> {
    tokio::select! {
        biased;
        _ = cancelled.wait_for(|value| *value != generation) => None,
        result = request => Some(result),
    }
}

fn commit(db: &Database, job: &crate::database::ParseJob, result: Result<(crate::models::InboxRecord, String), (String, bool)>) -> Result<bool, String> {
    db.commit_parse(job, result, Utc::now().timestamp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_drops_an_inflight_request_and_wins_when_both_are_ready() {
        let (sender, mut receiver) = tokio::sync::watch::channel(0);
        let (finish, pending) = tokio::sync::oneshot::channel::<()>();
        let request = tokio::spawn(async move { cancellable(0, &mut receiver, pending).await });
        tokio::task::yield_now().await;
        sender.send_replace(1);
        assert!(request.await.unwrap().is_none());
        assert!(finish.send(()).is_err(), "request future must be dropped");
        let mut receiver = sender.subscribe();
        assert!(cancellable(0, &mut receiver, async { "already ready" }).await.is_none());
    }

    #[tokio::test]
    async fn unchanged_generation_allows_a_successful_request() {
        let (_sender, mut receiver) = tokio::sync::watch::channel(7);
        assert_eq!(cancellable(7, &mut receiver, async { 42 }).await, Some(42));
    }
}
