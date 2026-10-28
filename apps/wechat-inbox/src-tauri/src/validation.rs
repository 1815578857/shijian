use chrono::NaiveDate;
use reqwest::Url;

use crate::models::{Priority, Settings};

pub const MAX_TITLE: usize = 500;
pub const MAX_ORIGINAL_TEXT: usize = 50_000;
const MAX_KEY: usize = 8192;

pub fn text(value: &str, label: &str, max: usize) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label}不能为空"));
    }
    if value.chars().count() > max {
        return Err(format!("{label}不能超过 {max} 个字符"));
    }
    if value.contains('\0') {
        return Err(format!("{label}不能包含空字符"));
    }
    Ok(value.to_owned())
}

pub fn model_url(value: &str) -> Result<Option<Url>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.len() > 2048 {
        return Err("模型服务地址过长".into());
    }
    let url = Url::parse(value).map_err(|_| "模型服务地址不是有效的绝对 URL")?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err("模型服务地址不能包含用户名或密码".into());
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err("模型服务地址不能包含查询参数或片段，请使用服务基础地址".into());
    }
    let host = url.host_str().ok_or("模型服务地址缺少主机名")?;
    let local = host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err("模型服务地址必须使用 HTTPS；仅本机 localhost/回环地址允许 HTTP".into());
    }
    Ok(Some(url))
}

pub fn normalized_model_base_url(value: &str) -> Result<String, String> {
    let Some(url) = model_url(value)? else {
        return Ok(String::new());
    };
    // URL serialization normalizes host casing, default ports and dot segments.
    // Both credential selection and persisted settings must use this exact representation.
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub fn settings(mut settings: Settings) -> Result<Settings, String> {
    if !(1..=365).contains(&settings.retention_days) {
        return Err("保留天数必须在 1 到 365 天之间".into());
    }
    settings.model_base_url = normalized_model_base_url(&settings.model_base_url)?;
    settings.model_name = settings.model_name.trim().to_owned();
    if settings.model_name.chars().count() > 200
        || settings.model_name.chars().any(char::is_control)
    {
        return Err("模型名称不能超过 200 个字符或包含控制字符".into());
    }
    settings.destination_label = text(&settings.destination_label, "目标名称", 100)?;
    if !(15..=1440).contains(&settings.remind_interval_minutes) {
        return Err("重复提醒间隔必须在 0.25 到 24 小时之间".into());
    }
    Ok(settings)
}

pub fn api_key(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_KEY || value.chars().any(char::is_control) {
        return Err("API Key 不能为空、超过 8192 字节或包含控制字符".into());
    }
    Ok(value.to_owned())
}

pub fn due_date(value: Option<String>) -> Result<Option<String>, String> {
    let Some(value) = value else { return Ok(None) };
    let bytes = value.as_bytes();
    let shape = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    if !shape || NaiveDate::parse_from_str(&value, "%Y-%m-%d").is_err() || value.starts_with("0000")
    {
        return Err("截止日期必须是有效的 YYYY-MM-DD 日期".into());
    }
    Ok(Some(value))
}

/// 截止时间：HH:MM，必须同时设置了截止日期。
pub fn due_time(value: Option<String>, due_date: Option<&str>) -> Result<Option<String>, String> {
    let Some(value) = value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if due_date.is_none() {
        return Err("设置时间前请先选择截止日期".into());
    }
    if value.len() != 5 || chrono::NaiveTime::parse_from_str(&value, "%H:%M").is_err() {
        return Err("截止时间必须是有效的 HH:MM 时间".into());
    }
    Ok(Some(value))
}

pub fn priority(value: &str) -> Result<Priority, String> {
    match value {
        "normal" => Ok(Priority::Normal),
        "high" => Ok(Priority::High),
        _ => Err("优先级仅支持 normal 或 high".into()),
    }
}

pub fn record_id(value: &str) -> Result<(), String> {
    uuid::Uuid::parse_str(value).map_err(|_| "记录 ID 无效".to_owned())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_transport_and_credentials() {
        for url in [
            "",
            "https://api.example.com/v1",
            "http://localhost:8080/v1",
            "http://127.0.0.1:11434/v1",
            "http://[::1]:8000/v1",
        ] {
            assert!(model_url(url).is_ok(), "{url}");
        }
        for url in [
            "http://example.com",
            "https://user:pass@example.com",
            "https://user@example.com",
            "ftp://localhost",
            "https://example.com?key=secret",
            "https://example.com/#fragment",
            "not a url",
            "http://localhost.evil.test",
        ] {
            assert!(model_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn validates_calendar_dates() {
        assert!(due_date(None).is_ok());
        assert_eq!(due_time(Some("09:30".into()), Some("2026-10-25")).unwrap().as_deref(), Some("09:30"));
        assert_eq!(due_time(Some(" ".into()), None).unwrap(), None);
        for bad in ["9:30", "24:00", "12:60", "noon"] {
            assert!(due_time(Some(bad.into()), Some("2026-10-25")).is_err(), "{bad}");
        }
        assert!(due_time(Some("09:30".into()), None).is_err(), "没有日期不能只设时间");
        assert!(due_date(Some("2024-02-29".into())).is_ok());
        for date in [
            "2025-02-29",
            "2026-04-31",
            "2026-1-01",
            "0000-01-01",
            "2026-10-23T12:00:00Z",
            "",
        ] {
            assert!(due_date(Some(date.into())).is_err(), "{date}");
        }
    }

    #[test]
    fn validates_settings_and_keys() {
        assert_eq!(settings(Settings::default()).unwrap().retention_days, 30);
        for retention_days in [0, 366] {
            assert!(settings(Settings {
                retention_days,
                ..Settings::default()
            })
            .is_err());
        }
        assert_eq!(api_key("  test-key  ").unwrap(), "test-key");
        assert!(api_key(" ").is_err());
        assert!(api_key("bad\nkey").is_err());
        assert!(api_key(&"x".repeat(8193)).is_err());
        assert!(text("  ", "标题", MAX_TITLE).is_err());
        assert!(text(&"文".repeat(500), "标题", MAX_TITLE).is_ok());
        assert!(text(&"文".repeat(501), "标题", MAX_TITLE).is_err());
        assert!(priority("urgent").is_err());
    }
}
