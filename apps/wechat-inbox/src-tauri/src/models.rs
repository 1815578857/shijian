use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    Local,
    External,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CloseBehavior {
    Hide,
    Quit,
}

/// 0.3 的旧提醒频率，只用于读取旧设置并迁移到新的提醒开关。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReminderMode {
    #[default]
    Daily,
    Unlock,
}

/// 解锁屏幕时的提醒方式。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum UnlockReminder {
    /// 每次解锁都提醒。
    EveryUnlock,
    /// 每天只在第一次解锁时提醒。
    #[default]
    FirstOfDay,
}

/// 解析请求的思考强度；Default 不携带 reasoning_effort 参数，跟随服务默认。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    #[default]
    Default,
    None,
    Low,
    Medium,
    High,
    Max,
}

/// 桌面待办面板窗口的边界；None 表示使用默认位置。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PanelBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    pub model_base_url: String,
    pub model_name: String,
    pub destination: Destination,
    pub destination_label: String,
    pub paused: bool,
    pub close_behavior: CloseBehavior,
    pub retention_days: u16,
    #[serde(default)]
    pub launch_at_login: bool,
    #[serde(default = "default_true")]
    pub reminders_enabled: bool,
    /// 旧字段：只读不写，读取后由数据库层迁移到下面的开关。
    #[serde(default, skip_serializing)]
    pub reminder_mode: ReminderMode,
    #[serde(default = "default_true")]
    pub remind_on_unlock: bool,
    #[serde(default)]
    pub unlock_reminder: UnlockReminder,
    #[serde(default = "default_true")]
    pub remind_on_startup: bool,
    #[serde(default)]
    pub remind_repeat: bool,
    #[serde(default = "default_repeat_minutes")]
    pub remind_interval_minutes: u16,
    #[serde(default)]
    pub reasoning_effort: ReasoningEffort,
    #[serde(default)]
    pub parsing_enabled: bool,
    #[serde(default)]
    pub panel_enabled: bool,
    #[serde(default)]
    pub panel_pinned: bool,
    #[serde(default)]
    pub panel_bounds: Option<PanelBounds>,
}

fn default_true() -> bool {
    true
}

fn default_repeat_minutes() -> u16 {
    90
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model_base_url: String::new(),
            model_name: String::new(),
            destination: Destination::Local,
            destination_label: "本地收件箱".into(),
            paused: false,
            close_behavior: CloseBehavior::Hide,
            retention_days: 30,
            launch_at_login: false,
            reminders_enabled: true,
            reminder_mode: ReminderMode::Daily,
            remind_on_unlock: true,
            unlock_reminder: UnlockReminder::FirstOfDay,
            remind_on_startup: true,
            remind_repeat: false,
            remind_interval_minutes: 90,
            reasoning_effort: ReasoningEffort::Default,
            parsing_enabled: false,
            panel_enabled: false,
            panel_pinned: false,
            panel_bounds: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    Saved,
    NeedsReview,
    Failed,
    Completed,
    Archived,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecordSource {
    Local,
    Wechat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Normal,
    High,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InboxRecord {
    pub id: String,
    pub title: String,
    pub original_text: String,
    pub status: RecordStatus,
    pub source: RecordSource,
    pub created_at: String,
    #[serde(default)]
    pub archived_at: Option<String>,
    pub due_date: Option<String>,
    /// 截止当天的具体时间（HH:MM），到点单独提醒；只有设置了日期才可能有时间。
    #[serde(default)]
    pub due_time: Option<String>,
    pub priority: Priority,
    pub error: Option<String>,
    pub is_demo: bool,
    /// 归档前的状态；恢复时还原，平时为空且不写入 JSON。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_before_archive: Option<RecordStatus>,
}

/// Missing preserves the current value; explicit null clears it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum NullablePatch<T> {
    #[default]
    Missing,
    Value(Option<T>),
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for NullablePatch<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(Self::Value)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordPatch {
    pub title: Option<String>,
    #[serde(default)]
    pub due_date: NullablePatch<String>,
    #[serde(default)]
    pub due_time: NullablePatch<String>,
    pub priority: Option<Priority>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SettingsPatch {
    pub model_base_url: Option<String>,
    pub model_name: Option<String>,
    pub destination: Option<Destination>,
    pub destination_label: Option<String>,
    pub paused: Option<bool>,
    pub close_behavior: Option<CloseBehavior>,
    pub retention_days: Option<u16>,
    pub launch_at_login: Option<bool>,
    pub reminders_enabled: Option<bool>,
    pub remind_on_unlock: Option<bool>,
    pub unlock_reminder: Option<UnlockReminder>,
    pub remind_on_startup: Option<bool>,
    pub remind_repeat: Option<bool>,
    pub remind_interval_minutes: Option<u16>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub parsing_enabled: Option<bool>,
    pub panel_enabled: Option<bool>,
    pub panel_pinned: Option<bool>,
}
impl SettingsPatch {
    pub fn merge(self, mut settings: Settings) -> Settings {
        if let Some(value) = self.model_base_url { settings.model_base_url = value; }
        if let Some(value) = self.model_name { settings.model_name = value; }
        if let Some(value) = self.destination { settings.destination = value; }
        if let Some(value) = self.destination_label { settings.destination_label = value; }
        if let Some(value) = self.paused { settings.paused = value; }
        if let Some(value) = self.close_behavior { settings.close_behavior = value; }
        if let Some(value) = self.retention_days { settings.retention_days = value; }
        if let Some(value) = self.launch_at_login { settings.launch_at_login = value; }
        if let Some(value) = self.reminders_enabled { settings.reminders_enabled = value; }
        if let Some(value) = self.remind_on_unlock { settings.remind_on_unlock = value; }
        if let Some(value) = self.unlock_reminder { settings.unlock_reminder = value; }
        if let Some(value) = self.remind_on_startup { settings.remind_on_startup = value; }
        if let Some(value) = self.remind_repeat { settings.remind_repeat = value; }
        if let Some(value) = self.remind_interval_minutes { settings.remind_interval_minutes = value; }
        if let Some(value) = self.reasoning_effort { settings.reasoning_effort = value; }
        if let Some(value) = self.parsing_enabled { settings.parsing_enabled = value; }
        if let Some(value) = self.panel_enabled { settings.panel_enabled = value; }
        if let Some(value) = self.panel_pinned { settings.panel_pinned = value; }
        settings
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub settings: Settings,
    pub records: Vec<InboxRecord>,
    pub has_model_key: bool,
    pub model_credential_error: Option<String>,
    pub data_dir: String,
    pub runtime: &'static str,
    pub wechat_status: String,
    pub wechat: WechatSnapshot,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WechatSnapshot {
    pub status: String,
    pub message: String,
    pub qr_code_svg: Option<String>,
    pub qr_expires_at: Option<String>,
    pub account_id: Option<String>,
    pub pending_replies: usize,
    pub failed_replies: usize,
    pub last_received_at: Option<String>,
}

impl Default for WechatSnapshot {
    fn default() -> Self {
        Self {
            status: "not_connected".into(),
            message: "尚未连接微信；本地手动记录仍可使用".into(),
            qr_code_svg: None,
            qr_expires_at: None,
            account_id: None,
            pending_replies: 0,
            failed_replies: 0,
            last_received_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_patch_preserves_unrelated_fields_and_rejects_internal_fields() {
        let current = Settings { panel_pinned: true, paused: true, ..Settings::default() };
        let patch: SettingsPatch = serde_json::from_str(r#"{"retentionDays":90}"#).unwrap();
        let updated = patch.merge(current);
        assert_eq!(updated.retention_days, 90);
        assert!(updated.panel_pinned && updated.paused);
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"panelBounds":null}"#).is_err());
        assert!(serde_json::from_str::<SettingsPatch>(r#"{"reminderMode":"unlock"}"#).is_err());
    }

    #[test]
    fn settings_saved_by_older_versions_still_load() {
        // 0.1 保存的设置没有新字段；缺少的字段必须落到默认值而不是报损坏。
        let legacy = r#"{"modelBaseUrl":"","modelName":"","destination":"local","destinationLabel":"本地收件箱","paused":true,"closeBehavior":"quit","retentionDays":7}"#;
        let settings: Settings = serde_json::from_str(legacy).unwrap();
        assert!(settings.paused);
        assert_eq!(settings.retention_days, 7);
        assert!(!settings.launch_at_login);
        assert!(settings.reminders_enabled);
        assert!(settings.remind_on_unlock && settings.remind_on_startup && !settings.remind_repeat);
        assert_eq!(settings.unlock_reminder, UnlockReminder::FirstOfDay);
        let value = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(value["remindersEnabled"], true);
        assert_eq!(value["launchAtLogin"], false);
        assert!(value.get("reminderMode").is_none(), "旧提醒频率只读不写");
        assert_eq!(value["unlockReminder"], "first_of_day");
        assert_eq!(value["remindIntervalMinutes"], 90);
        assert_eq!(value["reasoningEffort"], "default");
        assert!(!value["parsingEnabled"].as_bool().unwrap());
        assert!(!value["panelEnabled"].as_bool().unwrap());
        assert!(value.get("panelBounds").is_none() || value["panelBounds"].is_null());
    }

    #[test]
    fn status_before_archive_roundtrips_and_defaults_for_old_records() {
        let json = r#"{"id":"test","title":"t","originalText":"o","status":"archived","source":"local","createdAt":"2026-10-24T00:00:00.000Z","dueDate":null,"priority":"normal","error":null,"isDemo":false,"statusBeforeArchive":"completed"}"#;
        let record: InboxRecord = serde_json::from_str(json).unwrap();
        assert_eq!(record.status_before_archive, Some(RecordStatus::Completed));
        assert!(serde_json::to_string(&record)
            .unwrap()
            .contains(r#""statusBeforeArchive":"completed""#));
        // 旧版本归档的记录没有这个字段：恢复时退回待处理（旧行为）。
        let legacy = r#"{"id":"test","title":"t","originalText":"o","status":"archived","source":"local","createdAt":"2026-10-24T00:00:00.000Z","dueDate":null,"priority":"normal","error":null,"isDemo":false}"#;
        let legacy_record: InboxRecord = serde_json::from_str(legacy).unwrap();
        assert_eq!(legacy_record.status_before_archive, None);
    }

    #[test]
    fn snapshot_matches_frontend_contract() {
        let snapshot = AppSnapshot {
            settings: Settings::default(),
            records: vec![InboxRecord {
                id: "test".into(),
                title: "本地待办".into(),
                original_text: "手动输入".into(),
                status: RecordStatus::NeedsReview,
                source: RecordSource::Local,
                created_at: "2026-10-23T00:00:00Z".into(),
                due_date: None,
                priority: Priority::Normal,
                error: None,
                is_demo: false,
                status_before_archive: None,
            archived_at: None,
                due_time: None,
            }],
            has_model_key: false,
            model_credential_error: None,
            data_dir: "test-directory".into(),
            runtime: "desktop",
            wechat_status: "not_connected".into(),
            wechat: WechatSnapshot::default(),
        };
        let value = serde_json::to_value(snapshot).unwrap();
        assert_eq!(value["settings"]["modelBaseUrl"], "");
        assert_eq!(value["settings"]["destinationLabel"], "本地收件箱");
        assert_eq!(value["settings"]["closeBehavior"], "hide");
        assert_eq!(value["wechatStatus"], "not_connected");
        assert_eq!(value["hasModelKey"], false);
        assert_eq!(value["records"][0]["status"], "needs_review");
        assert_eq!(value["records"][0]["originalText"], "手动输入");
        assert_eq!(value["records"][0]["isDemo"], false);
        assert!(value["records"][0]["dueDate"].is_null());
        assert!(
            value["records"][0].get("statusBeforeArchive").is_none(),
            "归档前状态为空时不应出现在导出 JSON 中"
        );
        assert!(value["settings"].get("apiKey").is_none());
        assert!(value.get("has_model_key").is_none());
    }
}
