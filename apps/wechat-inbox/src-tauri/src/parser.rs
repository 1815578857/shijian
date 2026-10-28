//! 模型解析结果 → 确定性业务操作。
//! 模型只输出结构化提议；这里负责校验每个字段并决定如何落到记录上，
//! 不存在模型可执行的任意命令：能改的只有当前这一条记录的标题、日期、时间、优先级与状态。

use chrono::{Local, NaiveDate};

use crate::{
    models::{InboxRecord, Priority, RecordStatus},
    validation,
};

const MAX_REVIEW_REASON: usize = 200;
const PREFERRED_TITLE: usize = 60;

/// 模型返回的结构化提议（宽松反序列化，缺字段视为 None）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedIntent {
    pub title: Option<String>,
    pub due_date: Option<String>,
    pub due_time: Option<String>,
    pub priority: Option<Priority>,
    pub needs_review: bool,
    pub review_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// 配置或服务问题，重试也不会成功。
    Permanent(String),
    /// 网络、超时、限流等，稍后重试有意义。
    Transient(String),
}

impl ParseError {
    pub fn message(&self) -> &str {
        match self {
            ParseError::Permanent(message) | ParseError::Transient(message) => message,
        }
    }

    pub fn is_permanent(&self) -> bool {
        matches!(self, ParseError::Permanent(_))
    }
}

/// 应用解析结果；返回用于微信回执的结果摘要。
pub fn apply(record: &mut InboxRecord, intent: ParsedIntent, today: NaiveDate) -> Result<String, String> {
    if matches!(record.status, RecordStatus::Archived | RecordStatus::Completed) {
        return Err("已完成或归档的记录不接受解析结果".into());
    }

    let mut review_reason: Option<String> = intent
        .review_reason
        .map(|reason| reason.trim().chars().take(MAX_REVIEW_REASON).collect::<String>())
        .filter(|reason| !reason.is_empty());

    // 标题：模型给的可读短标题优先，非法时保留原文标题。
    if let Some(title) = intent.title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        if let Ok(valid) = validation::text(title, "标题", validation::MAX_TITLE) {
            record.title = if valid.chars().count() > PREFERRED_TITLE {
                valid.chars().take(PREFERRED_TITLE).collect()
            } else {
                valid
            };
        }
    }

    // 截止日期：必须是有效日期且不早于今天；过去或非法的日期不落库并要求补充。
    if let Some(due) = intent.due_date.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        match validation::due_date(Some(due.to_owned())) {
            Ok(Some(valid)) => {
                if let Ok(date) = NaiveDate::parse_from_str(&valid, "%Y-%m-%d") {
                    if date < today {
                        review_reason = Some(format!("解析出的截止日期 {valid} 已经过去，请补充正确日期"));
                    } else {
                        record.due_date = Some(valid);
                    }
                }
            }
            _ => review_reason = Some("模型给出的截止日期格式无效，请手动设置".into()),
        }
    }

    // Keep other valid fields, but never report malformed/orphaned time as success.
    // A date/review reason already established above is more important.
    if let Some(time) = intent.due_time.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        match validation::due_time(Some(time.to_owned()), record.due_date.as_deref()) {
            Ok(valid) => record.due_time = valid,
            Err(reason) if review_reason.is_none() => review_reason = Some(reason),
            Err(_) => {},
        }
    }

    if let Some(priority) = intent.priority {
        record.priority = priority;
    }

    if intent.needs_review || review_reason.is_some() {
        record.status = RecordStatus::NeedsReview;
        record.error = Some(
            review_reason
                .unwrap_or_else(|| "模型无法确认这条待办的关键信息，请补充后再确认".into()),
        );
    } else {
        record.status = RecordStatus::Saved;
        record.error = None;
    }

    Ok(if record.status == RecordStatus::NeedsReview {
        format!("已保存，待补充：{}", record.error.clone().unwrap_or_default())
    } else {
        let mut summary = format!("已解析保存：{}", record.title);
        if let Some(due) = &record.due_date {
            summary.push_str(&format!("，截止 {due}"));
            if let Some(time) = &record.due_time {
                summary.push_str(&format!(" {time}"));
            }
        }
        if record.priority == Priority::High {
            summary.push_str("，已标为优先处理");
        }
        summary
    })
}

/// 把模型返回的原始 JSON 文本解析为提议；容忍代码块围栏与前后杂文字。
pub fn parse_intent_json(raw: &str) -> Result<ParsedIntent, ParseError> {
    let trimmed = raw.trim();
    let start = trimmed.find('{').ok_or_else(|| {
        ParseError::Transient("模型没有返回 JSON，请重试或更换模型".into())
    })?;
    let end = trimmed.rfind('}').ok_or_else(|| {
        ParseError::Transient("模型返回的 JSON 不完整，请重试或更换模型".into())
    })?;
    if end < start {
        return Err(ParseError::Transient("模型返回的 JSON 格式无效".into()));
    }
    #[derive(serde::Deserialize, Default)]
    #[serde(rename_all = "camelCase", default)]
    struct RawIntent {
        title: Option<String>,
        due_date: Option<String>,
        due_time: Option<String>,
        priority: Option<String>,
        needs_review: bool,
        review_reason: Option<String>,
    }
    let raw_intent: RawIntent = serde_json::from_str(&trimmed[start..=end]).map_err(|_| {
        ParseError::Transient("模型返回的 JSON 字段不符合约定，请重试或更换模型".into())
    })?;
    let nonempty = |value: &Option<String>| value.as_deref().is_some_and(|s| !s.trim().is_empty());
    let explicit_priority = matches!(raw_intent.priority.as_deref().map(str::trim), Some("high" | "高" | "normal" | "普通"));
    if !(nonempty(&raw_intent.title) || nonempty(&raw_intent.due_date)
        || nonempty(&raw_intent.due_time) || explicit_priority
        || raw_intent.needs_review || nonempty(&raw_intent.review_reason)) {
        return Err(ParseError::Transient("模型返回的 JSON 没有有效的待办字段，请重试或更换模型".into()));
    }
    let priority = match raw_intent.priority.as_deref().map(str::trim) {
        Some("high") | Some("高") => Some(Priority::High),
        // 缺省按普通处理；无法识别的值不猜，保持记录原值。
        Some("normal") | Some("普通") | Some("") | None => Some(Priority::Normal),
        _ => None,
    };
    Ok(ParsedIntent {
        title: raw_intent.title,
        due_date: raw_intent.due_date,
        due_time: raw_intent.due_time,
        priority,
        needs_review: raw_intent.needs_review,
        review_reason: raw_intent.review_reason,
    })
}

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::RecordSource;

    fn sample_record() -> InboxRecord {
        InboxRecord {
            id: "test".into(),
            title: "原始长标题文本".into(),
            original_text: "明天下午三点提醒我开会".into(),
            status: RecordStatus::Saved,
            source: RecordSource::Wechat,
            created_at: "2026-10-24T00:00:00.000Z".into(),
            due_date: None,
            priority: Priority::Normal,
            error: None,
            is_demo: false,
            status_before_archive: None,
            archived_at: None,
            due_time: None,
        }
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn clean_intent_is_applied_verbatim() {
        let mut record = sample_record();
        let summary = apply(
            &mut record,
            ParsedIntent {
                title: Some("参加产品评审会".into()),
                due_date: Some("2026-10-25".into()),
                due_time: Some("15:00".into()),
                priority: Some(Priority::High),
                needs_review: false,
                review_reason: None,
            },
            date(2026, 10, 24),
        )
        .unwrap();
        assert_eq!(record.title, "参加产品评审会");
        assert_eq!(record.due_date.as_deref(), Some("2026-10-25"));
        assert_eq!(record.priority, Priority::High);
        assert_eq!(record.status, RecordStatus::Saved);
        assert!(record.error.is_none());
        assert!(summary.contains("已解析保存"));
        assert_eq!(record.due_time.as_deref(), Some("15:00"));
        assert!(summary.contains("截止 2026-10-25 15:00"));
    }

    #[test]
    fn times_without_dates_or_in_bad_format_require_review() {
        let mut record = sample_record();
        apply(&mut record, ParsedIntent { due_time: Some("15:00".into()), ..Default::default() }, date(2026, 10, 24)).unwrap();
        assert!(record.due_time.is_none());
        let mut bad = sample_record();
        apply(&mut bad, ParsedIntent { due_date: Some("2026-10-25".into()), due_time: Some("下午三点".into()), ..Default::default() }, date(2026, 10, 24)).unwrap();
        assert_eq!(bad.due_date.as_deref(), Some("2026-10-25"));
        assert!(bad.due_time.is_none());
        assert_eq!(record.status, RecordStatus::NeedsReview);
        assert_eq!(record.error.as_deref(), Some("设置时间前请先选择截止日期"));
        assert_eq!(bad.status, RecordStatus::NeedsReview);
        assert_eq!(bad.error.as_deref(), Some("截止时间必须是有效的 HH:MM 时间"));
        assert_eq!(parse_intent_json("{\"dueDate\":\"2026-10-25\",\"dueTime\":\"09:30\"}").unwrap().due_time.as_deref(), Some("09:30"));
    }

    #[test]
    fn past_or_invalid_dates_become_review_requests() {
        let mut record = sample_record();
        apply(
            &mut record,
            ParsedIntent {
                due_date: Some("2020-01-01".into()),
                ..Default::default()
            },
            date(2026, 10, 24),
        )
        .unwrap();
        assert_eq!(record.status, RecordStatus::NeedsReview);
        assert!(record.due_date.is_none());
        assert!(record.error.as_deref().unwrap().contains("已经过去"));

        let mut invalid = sample_record();
        apply(
            &mut invalid,
            ParsedIntent {
                due_date: Some("2026-02-30".into()),
                ..Default::default()
            },
            date(2026, 10, 24),
        )
        .unwrap();
        assert_eq!(invalid.status, RecordStatus::NeedsReview);
    }

    #[test]
    fn review_flag_and_reason_are_kept() {
        let mut record = sample_record();
        apply(
            &mut record,
            ParsedIntent {
                needs_review: true,
                review_reason: Some("没有说明时间".into()),
                ..Default::default()
            },
            date(2026, 10, 24),
        )
        .unwrap();
        assert_eq!(record.status, RecordStatus::NeedsReview);
        assert_eq!(record.error.as_deref(), Some("没有说明时间"));
    }

    #[test]
    fn invalid_titles_fall_back_to_original() {
        let mut record = sample_record();
        apply(
            &mut record,
            ParsedIntent {
                title: Some("   ".into()),
                ..Default::default()
            },
            date(2026, 10, 24),
        )
        .unwrap();
        assert_eq!(record.title, "原始长标题文本");
    }

    #[test]
    fn json_extraction_tolerates_fences_and_prose() {
        let intent = parse_intent_json("好的，结果如下：\n```json\n{\"title\":\"买牛奶\",\"dueDate\":\"2026-10-25\",\"priority\":\"normal\",\"needsReview\":false,\"reviewReason\":null}\n```").unwrap();
        assert_eq!(intent.title.as_deref(), Some("买牛奶"));
        assert_eq!(intent.due_date.as_deref(), Some("2026-10-25"));
        assert!(!intent.needs_review);
        assert!(parse_intent_json("这不是 JSON").is_err());
    }

    #[test]
    fn empty_or_unrecognized_intents_are_transient_errors() {
        for raw in [
            "{}", r#"{"error":"service unavailable"}"#, r#"{"unknown":"x"}"#,
            r#"{"title":null,"dueDate":null,"dueTime":null,"priority":null,"reviewReason":null}"#,
            r#"{"title":"  ","dueDate":"\t","dueTime":" ","priority":" ","reviewReason":"\n"}"#,
            r#"{"needsReview":false}"#, r#"{"priority":"urgent"}"#,
        ] {
            assert!(matches!(parse_intent_json(raw), Err(ParseError::Transient(_))), "{raw}");
        }
    }

    #[test]
    fn meaningful_optional_fields_and_unknown_extensions_remain_compatible() {
        for raw in [
            r#"{"title":"x","extra":true}"#, r#"{"dueDate":"2026-10-25"}"#,
            r#"{"dueTime":"15:00"}"#, r#"{"needsReview":true}"#,
            r#"{"reviewReason":"需补充日期"}"#, r#"{"priority":"normal"}"#,
            r#"{"priority":"high"}"#, r#"{"priority":"普通"}"#, r#"{"priority":"高"}"#,
            "```json\n{\"title\":\"围栏\"}\n```",
        ] {
            assert!(parse_intent_json(raw).is_ok(), "{raw}");
        }
        assert_eq!(parse_intent_json(r#"{"title":"x"}"#).unwrap().priority, Some(Priority::Normal));
    }

    #[test]
    fn invalid_time_keeps_valid_fields_and_date_errors_take_precedence() {
        for invalid_time in ["24:00", "12:60", "9:30", "下午三点"] {
            let mut record = sample_record();
            let reply = apply(&mut record, ParsedIntent {
                title: Some("合法标题".into()), due_date: Some("2026-10-25".into()),
                due_time: Some(invalid_time.into()), priority: Some(Priority::High), ..Default::default()
            }, date(2026, 10, 24)).unwrap();
            assert_eq!(record.title, "合法标题");
            assert_eq!(record.priority, Priority::High);
            assert_eq!(record.due_date.as_deref(), Some("2026-10-25"));
            assert!(record.due_time.is_none());
            assert_eq!(record.status, RecordStatus::NeedsReview);
            assert!(reply.contains("HH:MM"));
            assert!(!reply.contains("已解析保存"));
        }
        for (due, expected) in [("2020-01-01", "已经过去"), ("2026-02-30", "日期格式无效")] {
            let mut record = sample_record();
            let reply = apply(&mut record, ParsedIntent { due_date: Some(due.into()), due_time: Some("bad".into()), ..Default::default() }, date(2026, 10, 24)).unwrap();
            assert!(reply.contains(expected));
        }
    }

    #[test]
    fn unknown_priority_is_not_guessed() {
        let intent = parse_intent_json("{\"title\":\"x\",\"priority\":\"urgent\"}").unwrap();
        assert_eq!(intent.priority, None);
        let normal = parse_intent_json("{\"title\":\"x\"}").unwrap();
        assert_eq!(normal.priority, Some(Priority::Normal));
    }
}
