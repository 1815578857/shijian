use std::time::Duration;

use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::json;

use crate::{
    models::{ReasoningEffort, Settings},
    parser::{ParseError, ParsedIntent},
    validation,
};

#[derive(Deserialize)]
struct ModelList {
    data: Vec<Model>,
}

#[derive(Deserialize)]
struct Model {
    id: String,
}

/// 读取 /models 列表：返回给用户的提示消息 + 可选用的模型 ID。
pub async fn list_models(
    settings: &Settings,
    key: Option<String>,
) -> Result<(String, Vec<String>), String> {
    let mut url = validation::model_url(&settings.model_base_url)?.ok_or("请先填写模型服务地址")?;
    url.set_path(&format!("{}/models", url.path().trim_end_matches('/')));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "无法初始化模型连接测试")?;
    let request = client.get(url);
    let request = if let Some(key) = &key {
        request.bearer_auth(key)
    } else {
        request
    };
    let response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            "模型连接测试超时（15 秒）"
        } else {
            "模型连接测试失败，请检查地址、证书和网络"
        }
        .to_owned()
    })?;
    let status = response.status();
    if !status.is_success() {
        // Never expose response bodies (or URLs in reqwest errors): providers may echo credentials.
        return Err(format!(
            "模型服务返回 HTTP {}；未读取响应内容",
            status.as_u16()
        ));
    }
    const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
    let bytes = read_bounded_response(response, MAX_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            ResponseReadError::TooLarge => "模型列表超过 1 MiB 安全上限",
            ResponseReadError::Read => "读取模型列表失败或请求超时",
        })?;
    let Ok(list) = serde_json::from_slice::<ModelList>(&bytes) else {
        return Ok((
            "服务连接成功，但 /models 未返回兼容的模型列表，无法核对模型名称".into(),
            Vec::new(),
        ));
    };
    let mut ids: Vec<String> = list.data.into_iter().map(|model| model.id).filter(|id| {
        !id.trim().is_empty() && id == id.trim() && id.chars().count() <= 200
            && !id.chars().any(char::is_control)
            && !key.as_ref().is_some_and(|secret| id.contains(secret))
    }).collect();
    ids.sort();
    ids.dedup();
    let count = ids.len();
    let message = if count == 0 {
        "未获取到可用模型，可手动填写模型 ID".into()
    } else {
        format!("已获取 {count} 个模型")
    };
    Ok((message, ids))
}

#[derive(Debug)]
enum ResponseReadError {
    TooLarge,
    Read,
}

/// Call only after checking status: error bodies may echo credentials.
async fn read_bounded_response(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ResponseReadError> {
    if response.content_length().is_some_and(|length| length > limit as u64) {
        return Err(ResponseReadError::TooLarge);
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| ResponseReadError::Read)?;
        // Check before extending, including for chunked / close-delimited bodies.
        if chunk.len() > limit - bytes.len() {
            return Err(ResponseReadError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

const PARSE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PARSE_RESPONSE_BYTES: usize = 256 * 1024;

fn system_prompt(today: chrono::NaiveDate) -> String {
    format!(
        "你是待办解析器。把用户消息解析成一条结构化待办，只输出一个 JSON 对象，不要输出解释文字、代码块标记或任何其他内容。
字段约定：{{\"title\":\"简洁明确的待办标题，不超过30字\",\"dueDate\":\"YYYY-MM-DD 或 null\",\"dueTime\":\"HH:MM（24 小时制）或 null\",\"priority\":\"normal 或 high\",\"needsReview\":true 或 false,\"reviewReason\":null 或 一句话}}
规则：今天是 {today}；“明天、后天、下周三、月底、周五前”等相对时间按今天换算成具体日期；消息明确说了具体时刻（如“下午三点”“9点半”“晚上8点”）时把 dueTime 换算成 24 小时制，只说“上午、晚上”等模糊时段或没有时刻时 dueTime 为 null；有时刻但没说日期时默认今天；无法推断出日期时 dueDate 和 dueTime 都为 null；消息含糊、缺少关键信息或不是待办时 needsReview 为 true，并在 reviewReason 用不超过 50 字说明缺少什么；其余情况 needsReview 为 false、reviewReason 为 null。"
    )
}

/// 仅为已确认支持的服务携带思考参数；旧配置的不兼容档位回到服务默认。
fn parse_request_body(
    settings: &Settings,
    original_text: &str,
    today: chrono::NaiveDate,
) -> serde_json::Value {
    let mut body = json!({
        "model": settings.model_name,
        "temperature": 0,
        "stream": false,
        "messages": [
            {"role": "system", "content": system_prompt(today)},
            {"role": "user", "content": original_text}
        ]
    });
    let effort = crate::model_capabilities::effective_effort(
        &settings.model_base_url, &settings.model_name, settings.reasoning_effort,
    );
    if effort != ReasoningEffort::Default {
        body["reasoning_effort"] = json!(effort);
    }
    body
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatMessage {
    content: Option<String>,
}

/// 调用 OpenAI 兼容 /chat/completions 解析一条原始文本。
pub async fn parse_intent(
    settings: &Settings,
    key: Option<String>,
    original_text: &str,
    today: chrono::NaiveDate,
) -> Result<ParsedIntent, ParseError> {
    let mut url = validation::model_url(&settings.model_base_url)
        .map_err(ParseError::Permanent)?
        .ok_or_else(|| ParseError::Permanent("请先配置模型服务地址".into()))?;
    url.set_path(&format!("{}/chat/completions", url.path().trim_end_matches('/')));
    if settings.model_name.trim().is_empty() {
        return Err(ParseError::Permanent("请先配置模型名称".into()));
    }
    let client = reqwest::Client::builder()
        .timeout(PARSE_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ParseError::Permanent("无法初始化模型请求".into()))?;
    let body = parse_request_body(settings, original_text, today);
    let mut request = client.post(url).json(&body);
    if let Some(key) = key {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            ParseError::Transient("模型解析超时（30 秒）".into())
        } else {
            ParseError::Transient("无法连接模型服务，请检查地址和网络".into())
        }
    })?;
    let status = response.status();
    if !status.is_success() {
        // 与连接测试一致：不读取错误响应体，避免服务端回显凭据。
        let message = format!("模型服务返回 HTTP {}；未读取响应内容", status.as_u16());
        return Err(if status.as_u16() == 401 || status.as_u16() == 403 || status.as_u16() == 404 {
            ParseError::Permanent(message)
        } else {
            ParseError::Transient(message)
        });
    }
    let bytes = read_bounded_response(response, MAX_PARSE_RESPONSE_BYTES)
        .await
        .map_err(|error| ParseError::Transient(match error {
            ResponseReadError::TooLarge => "模型响应超过安全上限".into(),
            ResponseReadError::Read => "读取模型响应失败".into(),
        }))?;
    let parsed: ChatResponse = serde_json::from_slice(&bytes)
        .map_err(|_| ParseError::Transient("模型响应不是兼容的补全格式".into()))?;
    let content = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| ParseError::Transient("模型响应缺少结果".into()))?
        .message
        .content
        .unwrap_or_default();
    crate::parser::parse_intent_json(&content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Settings;
    use std::io::{Read, Write};

    fn settings(url: &str) -> Settings {
        Settings {
            model_base_url: url.into(),
            model_name: "test-model".into(),
            ..Settings::default()
        }
    }

    #[tokio::test]
    async fn parses_intent_from_local_mock() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut bytes = [0; 8192];
            let length = socket.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..length]).to_lowercase();
            assert!(request.contains("post /v1/chat/completions"));
            assert!(request.contains("authorization: bearer test-key"));
            assert!(request.contains("test-model"));
            assert!(request.contains("明天下午三点"));
            let body = "{\"choices\":[{\"message\":{\"content\":\"{\\\"title\\\":\\\"开会\\\",\\\"dueDate\\\":\\\"2026-10-25\\\",\\\"priority\\\":\\\"high\\\",\\\"needsReview\\\":false,\\\"reviewReason\\\":null}\"}}]}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            socket.write_all(response.as_bytes()).unwrap();
        });
        let intent = parse_intent(
            &settings(&format!("http://{addr}/v1")),
            Some("test-key".into()),
            "明天下午三点提醒我开会",
            chrono::NaiveDate::from_ymd_opt(2026, 10, 24).unwrap(),
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(intent.title.as_deref(), Some("开会"));
        assert_eq!(intent.priority, Some(crate::models::Priority::High));
    }

    // Keep the connection open until the caller returns. No EOF / final chunk or
    // remaining declared body is sent, so returning cannot depend on draining it.
    fn unfinished_response(
        response: Vec<u8>,
    ) -> (String, std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (release, wait) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            // Consume the entire request, not just the first TCP packet.
            let mut request = Vec::new();
            let mut buffer = [0u8; 8192];
            loop {
                let n = socket.read(&mut buffer).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length = headers.lines().find_map(|line| {
                        line.strip_prefix("content-length:")?.trim().parse::<usize>().ok()
                    }).unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            // Early rejection can close the connection during this write.
            let _ = socket.write_all(&response);
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
        });
        (format!("http://{addr}/v1"), release, server)
    }

    async fn response_error(response: Vec<u8>, discovery: bool) -> String {
        let (url, release, server) = unfinished_response(response);
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            if discovery {
                list_models(&settings(&url), Some("private-key".into())).await.unwrap_err()
            } else {
                let error = parse_intent(
                    &settings(&url), Some("private-key".into()), "买牛奶",
                    chrono::NaiveDate::from_ymd_opt(2026, 10, 24).unwrap(),
                ).await.unwrap_err();
                assert!(matches!(error, ParseError::Transient(_)));
                error.message().to_owned()
            }
        }).await;
        release.send(()).unwrap();
        server.join().unwrap();
        let message = result.expect("must reject before the server finishes the body");
        assert!(!message.contains("private-key"));
        message
    }

    #[tokio::test]
    async fn rejects_oversized_content_length_without_reading_body() {
        for discovery in [false, true] {
            let limit = if discovery { 1024 * 1024 } else { MAX_PARSE_RESPONSE_BYTES };
            // Only headers: even the first body byte is withheld.
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                limit + 1,
            );
            assert!(response_error(response.into_bytes(), discovery).await.contains("安全上限"));
        }
    }

    #[tokio::test]
    async fn rejects_unknown_length_and_chunked_bodies_before_eof() {
        for discovery in [false, true] {
            let limit = if discovery { 1024 * 1024 } else { MAX_PARSE_RESPONSE_BYTES };
            for chunked in [false, true] {
                let mut response = if chunked {
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec()
                } else {
                    b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec()
                };
                // Individually small chunks reach the boundary, then exceed it.
                for length in std::iter::repeat_n(4096, limit / 4096).chain([1]) {
                    if chunked {
                        write!(response, "{length:X}\r\n").unwrap();
                    }
                    response.extend(std::iter::repeat_n(b'x', length));
                    if chunked {
                        response.extend_from_slice(b"\r\n");
                    }
                }
                // Deliberately omit the final zero chunk / connection close.
                assert!(response_error(response, discovery).await.contains("安全上限"));
            }
        }
    }

    #[tokio::test]
    async fn accepts_exact_boundary_with_and_without_content_length() {
        for discovery in [false, true] {
            let limit = if discovery { 1024 * 1024 } else { MAX_PARSE_RESPONSE_BYTES };
            let body = if discovery {
                r#"{"data":[{"id":"test-model"}]}"#.to_owned()
            } else {
                json!({"choices": [{"message": {"content": json!({
                    "title": "买牛奶", "dueDate": null, "priority": "normal",
                    "needsReview": false, "reviewReason": null,
                }).to_string()}}]}).to_string()
            };
            let mut body = body.into_bytes();
            body.resize(limit, b' '); // Valid JSON whitespace, included in the limit.
            for framing in ["length", "chunked", "close"] {
                let mut response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n".to_vec();
                match framing {
                    "length" => write!(response, "Content-Length: {limit}\r\n\r\n").unwrap(),
                    "chunked" => response.extend_from_slice(b"Transfer-Encoding: chunked\r\n\r\n"),
                    _ => response.extend_from_slice(b"\r\n"),
                }
                if framing == "chunked" {
                    write!(response, "{limit:X}\r\n").unwrap();
                }
                response.extend_from_slice(&body);
                if framing == "chunked" {
                    response.extend_from_slice(b"\r\n0\r\n\r\n");
                }
                let (url, release, server) = unfinished_response(response);
                // Close-delimited success needs EOF; other framings must not.
                if framing == "close" {
                    release.send(()).unwrap();
                }
                let result = tokio::time::timeout(Duration::from_secs(3), async {
                    if discovery {
                        assert_eq!(list_models(&settings(&url), None).await.unwrap().1, vec!["test-model"]);
                    } else {
                        let intent = parse_intent(
                            &settings(&url), None, "买牛奶",
                            chrono::NaiveDate::from_ymd_opt(2026, 10, 24).unwrap(),
                        ).await.unwrap();
                        assert_eq!(intent.title.as_deref(), Some("买牛奶"));
                    }
                }).await;
                if framing != "close" {
                    release.send(()).unwrap();
                }
                server.join().unwrap();
                result.unwrap();
            }
        }
    }

    #[test]
    fn parse_body_carries_reasoning_effort_only_when_configured() {
        let settings = Settings {
            model_name: "test-model".into(),
            ..Settings::default()
        };
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 24).unwrap();
        assert!(parse_request_body(&settings, "买牛奶", today)
            .get("reasoning_effort")
            .is_none());
        let high = Settings {
            model_base_url: "https://api.deepseek.com".into(),
            model_name: "deepseek-flash".into(),
            reasoning_effort: ReasoningEffort::High,
            ..settings.clone()
        };
        assert_eq!(parse_request_body(&high, "买牛奶", today)["reasoning_effort"], "high");
        for effort in [ReasoningEffort::None, ReasoningEffort::Low, ReasoningEffort::Max] {
            let explicit = Settings { reasoning_effort: effort, ..high.clone() };
            assert_eq!(parse_request_body(&explicit, "买牛奶", today)["reasoning_effort"], json!(effort));
        }
        let legacy = Settings { reasoning_effort: ReasoningEffort::Medium, ..high.clone() };
        assert!(parse_request_body(&legacy, "买牛奶", today).get("reasoning_effort").is_none());
        let unknown = Settings { model_base_url: "https://proxy.example.com/v1".into(), ..high };
        assert!(parse_request_body(&unknown, "买牛奶", today).get("reasoning_effort").is_none());
    }

    #[tokio::test]
    async fn discovers_without_model_id_and_sanitizes_list() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let n = socket.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_lowercase();
            assert!(request.contains("get /v1/models"));
            assert!(request.contains("authorization: bearer draft-key"));
            let body = r#"{"data":[{"id":"b"},{"id":"a"},{"id":"a"},{"id":""},{"id":" draft "},{"id":"echo-draft-key"},{"id":"bad\nname"}]}"#;
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });
        let draft = Settings { model_base_url: format!("http://{addr}/v1"), ..Settings::default() };
        let (message, ids) = list_models(&draft, Some("draft-key".into())).await.unwrap();
        server.join().unwrap();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(message, "已获取 2 个模型");
    }

    #[tokio::test]
    async fn model_discovery_handles_empty_malformed_and_oversized_responses() {
        for (body, oversized) in [
            (r#"{"data":[]}"#.to_owned(), false),
            ("not-json".to_owned(), false),
            ("x".repeat(1024 * 1024 + 1), true),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                let mut buf = [0u8; 8192];
                let _ = socket.read(&mut buf);
                let _ = write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            });
            let result = list_models(&settings(&format!("http://{addr}/v1")), None).await;
            server.join().unwrap();
            if oversized {
                assert!(result.unwrap_err().contains("1 MiB"));
            } else {
                assert!(result.unwrap().1.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn auth_errors_are_permanent_and_do_not_echo_bodies() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            let _ = socket.read(&mut buf);
            socket
                .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 40\r\nConnection: close\r\n\r\n{\"error\":\"secret-token-should-not-leak\"}")
                .unwrap();
        });
        let error = parse_intent(
            &settings(&format!("http://{addr}/v1")),
            Some("bad-key".into()),
            "买牛奶",
            chrono::NaiveDate::from_ymd_opt(2026, 10, 24).unwrap(),
        )
        .await
        .unwrap_err();
        server.join().unwrap();
        assert!(error.is_permanent());
        assert!(!error.message().contains("secret-token-should-not-leak"));
        assert!(error.message().contains("401"));
    }
}
