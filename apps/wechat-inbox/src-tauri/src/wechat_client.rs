use std::time::Duration;

use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::StreamExt;
use reqwest::{Client, Method, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const LOGIN_BASE: &str = "https://ilinkai.weixin.qq.com";
const MAX_BYTES: usize = 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub bot_id: String,
    pub user_id: String,
    pub base_url: String,
}

impl Session {
    pub fn validate(&self) -> Result<(), String> {
        trusted_base(&self.base_url)?;
        for value in [&self.token, &self.bot_id, &self.user_id] {
            if value.is_empty() || value.len() > 8192 || value.chars().any(char::is_control) {
                return Err("微信登录凭据不完整，请重新扫码".into());
            }
        }
        if !self.token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err("微信登录凭据格式无效，请重新扫码".into());
        }
        Ok(())
    }
}

pub fn trusted_base(value: &str) -> Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "微信服务地址格式无效")?;
    let host = url.host_str().unwrap_or_default();
    if value.len() > 2048
        || url.scheme() != "https"
        || !(host == "weixin.qq.com" || host.ends_with(".weixin.qq.com"))
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("微信服务地址不在允许的 HTTPS 域名范围内".into());
    }
    Ok(url)
}

pub fn redirected_base(host: &str) -> Result<Url, String> {
    if host.is_empty() || host.contains(['/', '\\', '@', '?', '#', ':']) {
        return Err("微信二维码跳转地址无效".into());
    }
    trusted_base(&format!("https://{host}"))
}

#[derive(Debug)]
pub enum ApiError {
    SessionExpired,
    Timeout,
    Failure(String),
}

impl ApiError {
    pub fn message(&self) -> String {
        match self {
            Self::SessionExpired => "微信会话已失效，请重新扫码连接".into(),
            Self::Timeout => "微信请求超时，请检查网络后重试".into(),
            Self::Failure(message) => message.clone(),
        }
    }
}

pub fn check_response(value: &Value) -> Result<(), ApiError> {
    if ["ret", "errcode"]
        .iter()
        .any(|field| value.get(field).and_then(Value::as_i64) == Some(-14))
    {
        return Err(ApiError::SessionExpired);
    }
    for field in ["ret", "errcode"] {
        if let Some(code) = value.get(field) {
            let code = code
                .as_i64()
                .ok_or_else(|| ApiError::Failure("微信返回的业务状态格式无效".into()))?;
            if code == -14 {
                return Err(ApiError::SessionExpired);
            }
            if code != 0 {
                return Err(ApiError::Failure(format!(
                    "微信服务返回业务错误 {code}；未显示响应正文"
                )));
            }
        }
    }
    Ok(())
}

pub fn confirmed_session(value: &Value) -> Result<Session, String> {
    let field = |name| {
        value
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let session = Session {
        token: field("bot_token"),
        bot_id: field("ilink_bot_id"),
        user_id: field("ilink_user_id"),
        base_url: field("baseurl"),
    };
    session.validate()?;
    Ok(session)
}

#[derive(Clone)]
pub struct WechatClient {
    client: Client,
}

impl WechatClient {
    pub fn new() -> Result<Self, String> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(65))
            .build()
            .map_err(|_| "无法初始化微信连接")?;
        Ok(Self { client })
    }

    async fn request(
        &self,
        method: Method,
        mut url: Url,
        body: Option<Value>,
        token: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, ApiError> {
        url.set_fragment(None);
        let mut request = self
            .client
            .request(method.clone(), url)
            .header("iLink-App-Id", "bot")
            .header("iLink-App-ClientVersion", "256")
            .timeout(timeout);
        if method == Method::POST {
            let id = uuid::Uuid::new_v4();
            let random = u32::from_le_bytes(id.as_bytes()[..4].try_into().expect("UUID prefix"));
            request = request
                .header("AuthorizationType", "ilink_bot_token")
                .header("X-WECHAT-UIN", STANDARD.encode(random.to_string()));
            if let Some(token) = token {
                request = request.bearer_auth(token);
            }
            let bytes = serde_json::to_vec(&body.unwrap_or(Value::Null))
                .map_err(|_| ApiError::Failure("无法编码微信请求".into()))?;
            if bytes.len() > MAX_BYTES {
                return Err(ApiError::Failure("微信请求超过 1 MiB 安全上限".into()));
            }
            request = request
                .header("Content-Type", "application/json")
                .body(bytes);
        }
        let response = request.send().await.map_err(network_error)?;
        if token.is_some() && matches!(response.status().as_u16(), 401 | 403) {
            return Err(ApiError::SessionExpired);
        }
        if !response.status().is_success() {
            return Err(ApiError::Failure(format!(
                "微信服务返回 HTTP {}；未显示响应正文",
                response.status().as_u16()
            )));
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(network_error)?;
            if bytes.len().saturating_add(chunk.len()) > MAX_BYTES {
                return Err(ApiError::Failure("微信响应超过 1 MiB 安全上限".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| ApiError::Failure("微信返回了无法识别的 JSON 响应".into()))?;
        if !value.is_object() {
            return Err(ApiError::Failure("微信响应不是 JSON 对象".into()));
        }
        check_response(&value)?;
        Ok(value)
    }

    pub async fn qr(&self) -> Result<Value, ApiError> {
        let url = Url::parse(&format!("{LOGIN_BASE}/ilink/bot/get_bot_qrcode?bot_type=3"))
            .expect("fixed URL");
        self.request(
            Method::POST,
            url,
            Some(json!({"local_token_list": []})),
            None,
            Duration::from_secs(15),
        )
        .await
    }

    pub async fn qr_status(
        &self,
        base: &Url,
        qrcode: &str,
        code: Option<&str>,
    ) -> Result<Value, ApiError> {
        let mut url = base
            .join("ilink/bot/get_qrcode_status")
            .expect("validated base");
        url.query_pairs_mut().append_pair("qrcode", qrcode);
        if let Some(code) = code {
            url.query_pairs_mut().append_pair("verify_code", code);
        }
        self.request(Method::GET, url, None, None, Duration::from_secs(35))
            .await
    }

    pub async fn updates(
        &self,
        session: &Session,
        cursor: &str,
        timeout_ms: u64,
    ) -> Result<Value, ApiError> {
        let base = trusted_base(&session.base_url).map_err(ApiError::Failure)?;
        self.request(
            Method::POST,
            base.join("ilink/bot/getupdates").expect("fixed path"),
            Some(json!({"get_updates_buf": cursor,"base_info": base_info()})),
            Some(&session.token),
            Duration::from_millis(timeout_ms.clamp(5_000, 60_000) + 5_000),
        )
        .await
    }

    pub async fn send(
        &self,
        session: &Session,
        recipient: &str,
        context: &str,
        client_id: &str,
        text: &str,
    ) -> Result<(), ApiError> {
        if context.is_empty() {
            return Err(ApiError::Failure("消息缺少回复上下文，未发送回执".into()));
        }
        let base = trusted_base(&session.base_url).map_err(ApiError::Failure)?;
        let value = self
            .request(
                Method::POST,
                base.join("ilink/bot/sendmessage").expect("fixed path"),
                Some(json!({"msg": {
            "from_user_id": "", "to_user_id": recipient, "client_id": client_id,
            "message_type": 2, "message_state": 2, "context_token": context,
            "item_list": [{"type":1,"text_item":{"text":text}}]
        }, "base_info":base_info()})),
                Some(&session.token),
                Duration::from_secs(15),
            )
            .await?;
        if value.get("ret").and_then(Value::as_i64) != Some(0) {
            return Err(ApiError::Failure("微信未明确确认回执发送成功".into()));
        }
        Ok(())
    }
}

fn base_info() -> Value {
    json!({"channel_version":"0.1.0", "bot_agent":"Shijian/0.1.0"})
}

fn network_error(error: reqwest::Error) -> ApiError {
    if error.is_timeout() {
        ApiError::Timeout
    } else {
        ApiError::Failure("微信网络请求失败，请检查网络和证书".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_official_https_roots_are_accepted() {
        for value in [
            LOGIN_BASE,
            "https://region.weixin.qq.com/",
            "https://weixin.qq.com:443",
        ] {
            assert!(trusted_base(value).is_ok());
        }
        for value in [
            "http://ilinkai.weixin.qq.com",
            "https://weixin.qq.com.evil.test",
            "https://127.0.0.1",
            "https://user@ilinkai.weixin.qq.com",
            "https://ilinkai.weixin.qq.com:8443",
            "https://ilinkai.weixin.qq.com/path",
            "https://ilinkai.weixin.qq.com/?token=x",
            "https://ilinkai.weixin.qq.com/#x",
        ] {
            assert!(trusted_base(value).is_err(), "{value}");
        }
        for host in [
            "evil.test",
            "weixin.qq.com@evil.test",
            "ilinkai.weixin.qq.com/path",
            "ilinkai.weixin.qq.com:443",
            "ilinkai.weixin.qq.com\\evil",
        ] {
            assert!(redirected_base(host).is_err());
        }
    }

    #[test]
    fn confirmation_requires_complete_valid_credentials() {
        let valid = json!({"bot_token":"secret","ilink_bot_id":"bot","ilink_user_id":"owner","baseurl":LOGIN_BASE});
        assert!(confirmed_session(&valid).is_ok());
        for field in ["bot_token", "ilink_bot_id", "ilink_user_id", "baseurl"] {
            let mut value = valid.clone();
            value.as_object_mut().unwrap().remove(field);
            assert!(confirmed_session(&value).is_err());
        }
        assert!(matches!(
            check_response(&json!({"ret":0,"errcode":-14})),
            Err(ApiError::SessionExpired)
        ));
        assert!(matches!(
            check_response(&json!({"ret":1,"errcode":-14})),
            Err(ApiError::SessionExpired)
        ));
        assert!(check_response(&json!({"ret":1,"errmsg":"SECRET"}))
            .unwrap_err()
            .message()
            .contains("1"));
        assert!(!check_response(&json!({"ret":1,"errmsg":"SECRET"}))
            .unwrap_err()
            .message()
            .contains("SECRET"));
    }

    #[tokio::test]
    async fn post_headers_and_errors_never_echo_tokens() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/test", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut bytes = [0; 8192];
            let length = socket.read(&mut bytes).unwrap();
            let original = String::from_utf8_lossy(&bytes[..length]);
            let request = original.to_lowercase();
            assert!(request.contains("authorization: bearer sensitive-token"));
            assert!(request.contains("authorizationtype: ilink_bot_token"));
            let uin = original
                .lines()
                .find_map(|line| line.strip_prefix("x-wechat-uin: "))
                .unwrap();
            let raw = STANDARD.decode(uin).unwrap();
            assert!(raw.iter().all(u8::is_ascii_digit));
            socket.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 15\r\nConnection: close\r\n\r\nsensitive-token").unwrap();
        });
        let result = WechatClient::new()
            .unwrap()
            .request(
                Method::POST,
                url,
                Some(json!({"base_info":base_info()})),
                Some("sensitive-token"),
                Duration::from_secs(2),
            )
            .await;
        server.join().unwrap();
        let error = result.unwrap_err().message();
        assert!(error.contains("400"));
        assert!(!error.contains("sensitive-token"));
    }

    #[tokio::test]
    async fn redirects_are_not_followed_and_responses_are_bounded() {
        use std::io::{Read, Write};
        for oversized in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url =
                Url::parse(&format!("http://{}/test", listener.local_addr().unwrap())).unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                let mut bytes = [0; 8192];
                let _ = socket.read(&mut bytes);
                if oversized {
                    let body = format!("{{\"pad\":\"{}\"}}", "x".repeat(MAX_BYTES));
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(header.as_bytes());
                    let _ = socket.write_all(body.as_bytes());
                } else {
                    socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/untrusted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                }
            });
            let result = WechatClient::new()
                .unwrap()
                .request(Method::GET, url, None, None, Duration::from_secs(2))
                .await;
            server.join().unwrap();
            let error = result.unwrap_err().message();
            assert!(error.contains(if oversized { "1 MiB" } else { "302" }));
            assert!(!error.contains("untrusted"));
        }
    }

    #[tokio::test]
    async fn local_mock_receives_safe_application_headers() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/test", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut bytes = [0; 8192];
            let length = socket.read(&mut bytes).unwrap();
            let request = String::from_utf8_lossy(&bytes[..length]).to_lowercase();
            assert!(request.contains("ilink-app-id: bot"));
            assert!(request.contains("ilink-app-clientversion: 256"));
            assert!(!request.contains("authorization:"));
            assert!(!request.contains("authorizationtype:"));
            assert!(!request.contains("x-wechat-uin:"));
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\n{\"ret\":0}",
                )
                .unwrap();
        });
        let client = WechatClient::new().unwrap();
        // This private request helper is reachable only from fixed/validated production endpoints.
        let result = client
            .request(Method::GET, url, None, None, Duration::from_secs(2))
            .await;
        server.join().unwrap();
        assert!(result.is_ok());
    }
}
