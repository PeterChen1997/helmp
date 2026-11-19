//! 三种模型接口的阻塞式调用（ureq）：对话 30 秒，列模型 10 秒。
//!
//! 请求格式按官方文档：
//! - Anthropic Messages：`POST {base}/v1/messages`，头 `x-api-key`、`anthropic-version: 2023-06-01`，
//!   体 `{model, max_tokens, system, messages}`，回复取 `content[].text`。
//! - OpenAI 兼容 Chat Completions：`POST {base}/chat/completions`（base 含 `/v1`，如
//!   `https://api.deepseek.com/v1`），头 `Authorization: Bearer`，系统提示用 `system` 角色（DeepSeek
//!   等兼容实现不认 `developer`），回复取 `choices[0].message.content`。
//! - Ollama：`POST {base}/api/chat`，`stream: false`，回复取 `message.content`。
//!
//! 错误按界面语言显示。对话请求的 HTTP 错误还附上服务端原话。
//! 发给模型的提示词不经过界面语言。

use std::time::Duration;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use crate::config::{ModelConfig, ModelKind};
use crate::i18n::{tr, trf};

/// 单次请求的总超时。
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// 设置页列模型请求的总超时。
pub const LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// Anthropic 接口版本。只用到 model / max_tokens / system / messages 这几个稳定字段。
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// 模型调用失败的类别：界面据此决定引导用户「打开设置」还是「重试」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// 配置不完整：没填模型名、没填 API Key、钥匙串读不出来。
    Setup,
    /// 401 / 403：Key 无效或没有权限。
    Auth,
    /// 404：地址或模型名不对（Ollama 没 pull）。
    NotFound,
    /// 429：请求太频繁或额度用完。
    RateLimited,
    /// 等待超时。
    Timeout,
    /// 连不上：域名解析失败、端口没人监听、证书校验失败。
    Unreachable,
    /// 5xx：服务端出错。
    Server,
    /// 返回内容看不懂（多半是 Base URL 填错）。
    BadResponse,
    Other,
}

/// 模型调用失败：`Display` 是给用户看的那句话，`kind` 给界面分流。
#[derive(Debug)]
pub struct ModelError {
    pub kind: FailureKind,
    message: String,
}

impl ModelError {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ModelError {}

/// 错误链里的模型失败类别；不是模型调用造成的错误返回 None。
pub fn failure_kind(err: &anyhow::Error) -> Option<FailureKind> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<ModelError>())
        .map(|err| err.kind)
}

fn status_kind(status: u16) -> FailureKind {
    match status {
        401 | 403 => FailureKind::Auth,
        404 => FailureKind::NotFound,
        408 => FailureKind::Timeout,
        429 => FailureKind::RateLimited,
        500..=599 => FailureKind::Server,
        _ => FailureKind::Other,
    }
}

/// 一次调用的参数。
pub struct Request<'a> {
    pub system: &'a str,
    pub user: &'a str,
    /// 最多生成多少 token。OpenAI 兼容接口不传（新模型改叫 `max_completion_tokens`，
    /// 各家兼容实现不一致），只用于 Anthropic（必填）与 Ollama（`num_predict`）。
    pub max_tokens: u32,
}

/// 发出的 HTTP 请求（拆出来便于测试请求格式，不必真的发）。
#[derive(Debug, PartialEq)]
pub struct HttpRequest {
    pub url: String,
    pub headers: Vec<(&'static str, String)>,
    pub body: Value,
}

/// 各接口的默认 Base URL。
pub fn default_base_url(kind: ModelKind) -> &'static str {
    match kind {
        ModelKind::Anthropic => "https://api.anthropic.com",
        ModelKind::OpenAiCompatible => "https://api.openai.com/v1",
        ModelKind::Ollama => "http://localhost:11434",
    }
}

/// 该接口是否需要 API Key。
pub fn needs_key(kind: ModelKind) -> bool {
    !matches!(kind, ModelKind::Ollama)
}

fn base(model: &ModelConfig) -> String {
    let base = model.base_url.trim();
    let base = if base.is_empty() {
        default_base_url(model.kind)
    } else {
        base
    };
    base.trim_end_matches('/').to_string()
}

fn auth_headers(model: &ModelConfig, key: Option<&str>) -> Result<Vec<(&'static str, String)>> {
    if !needs_key(model.kind) {
        return Ok(Vec::new());
    }
    let Some(key) = key.map(str::trim).filter(|key| !key.is_empty()) else {
        return Err(ModelError::new(
            FailureKind::Setup,
            trf!(
                "「{}」还没有填写 API Key",
                "Enter an API key for “{}”",
                model.name
            ),
        )
        .into());
    };
    Ok(match model.kind {
        ModelKind::Anthropic => vec![
            ("x-api-key", key.to_string()),
            ("anthropic-version", ANTHROPIC_VERSION.to_string()),
        ],
        ModelKind::OpenAiCompatible => vec![("authorization", format!("Bearer {key}"))],
        ModelKind::Ollama => Vec::new(),
    })
}

/// Anthropic 的 Base URL 可以包含 /v1，拼路径时只加一次。
fn anthropic_url(base: &str, endpoint: &str) -> String {
    let root = base.strip_suffix("/v1").unwrap_or(base);
    format!("{root}/v1/{endpoint}")
}

pub fn build(model: &ModelConfig, key: Option<&str>, req: &Request) -> Result<HttpRequest> {
    if model.model.trim().is_empty() {
        return Err(ModelError::new(
            FailureKind::Setup,
            trf!(
                "「{}」还没有填写模型名",
                "Enter a model name for “{}”",
                model.name
            ),
        )
        .into());
    }
    let headers = auth_headers(model, key)?;
    let base = base(model);
    let name = model.model.trim();
    Ok(match model.kind {
        ModelKind::Anthropic => HttpRequest {
            url: anthropic_url(&base, "messages"),
            headers,
            body: json!({
                "model": name,
                "max_tokens": req.max_tokens,
                "system": req.system,
                "messages": [{ "role": "user", "content": req.user }],
            }),
        },
        ModelKind::OpenAiCompatible => HttpRequest {
            url: format!("{base}/chat/completions"),
            headers,
            body: json!({
                "model": name,
                "messages": [
                    { "role": "system", "content": req.system },
                    { "role": "user", "content": req.user },
                ],
            }),
        },
        ModelKind::Ollama => {
            let base = base.strip_suffix("/api").unwrap_or(&base);
            HttpRequest {
                url: format!("{base}/api/chat"),
                headers,
                body: json!({
                    "model": name,
                    "stream": false,
                    "messages": [
                        { "role": "system", "content": req.system },
                        { "role": "user", "content": req.user },
                    ],
                    "options": { "num_predict": req.max_tokens },
                }),
            }
        }
    })
}

/// 构造列模型的 GET 请求，设置页可以在模型名还没填写时调用。
pub fn build_list(model: &ModelConfig, key: Option<&str>) -> Result<HttpRequest> {
    let headers = auth_headers(model, key)?;
    let url = match model.kind {
        ModelKind::Anthropic => anthropic_url(&base(model), "models?limit=1000"),
        ModelKind::OpenAiCompatible => format!("{}/models", base(model)),
        ModelKind::Ollama => ollama_tags_url(&model.base_url),
    };
    Ok(HttpRequest {
        url,
        headers,
        body: Value::Null,
    })
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        // 4xx / 5xx 也读出响应体，里面有服务端给的错误原因。
        .http_status_as_error(false)
        .build()
        .into()
}

/// 发请求并取出回复文本。
pub fn send(model: &ModelConfig, key: Option<&str>, req: &Request) -> Result<String> {
    let http = build(model, key, req)?;
    let mut request = agent(TIMEOUT)
        .post(&http.url)
        .header("content-type", "application/json");
    for (name, value) in &http.headers {
        request = request.header(*name, value);
    }
    let mut response = request
        .send(http.body.to_string())
        .map_err(|err| transport_error(model, &http.url, TIMEOUT, err))?;
    let status = response.status().as_u16();
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|err| transport_error(model, &http.url, TIMEOUT, err))?;
    if !(200..300).contains(&status) {
        return Err(
            ModelError::new(status_kind(status), status_error(model.kind, status, &text)).into(),
        );
    }
    parse_reply(model.kind, &text)
        .map_err(|err| ModelError::new(FailureKind::BadResponse, err.to_string()).into())
}

/// 拉取模型列表。阻塞，设置页在后台线程调用。
pub fn list(model: &ModelConfig, key: Option<&str>) -> Result<Vec<String>> {
    let http = build_list(model, key)?;
    let mut request = agent(LIST_TIMEOUT)
        .get(&http.url)
        .config()
        // ureq 的重定向鉴权策略只处理 Authorization，Anthropic 用的 x-api-key 会被转发。
        .max_redirects(0)
        .build();
    for (name, value) in &http.headers {
        request = request.header(*name, value);
    }
    let transport = |err| {
        let mut err = transport_error(model, &http.url, LIST_TIMEOUT, err);
        if let Some(key) = key.map(str::trim).filter(|key| !key.is_empty()) {
            err.message = err.message.replace(key, "[REDACTED]");
        }
        err
    };
    let mut response = request.call().map_err(transport)?;
    let status = response.status().as_u16();
    if matches!(status, 404 | 405 | 501) {
        return Err(ModelError::new(
            FailureKind::BadResponse,
            tr(
                "这个服务商不提供模型列表，请手动填写模型名",
                "This provider doesn't list its models. Enter the model name manually.",
            ),
        )
        .into());
    }
    if !response.status().is_success() {
        // 服务商可能在错误响应里回显凭据，只显示本地化的 HTTP 状态。
        return Err(
            ModelError::new(status_kind(status), status_error(model.kind, status, "")).into(),
        );
    }
    let text = response.body_mut().read_to_string().map_err(transport)?;
    parse_model_list(model.kind, &text)
}

/// 提取各服务商的模型名，按字典序排序并去重。
pub fn parse_model_list(kind: ModelKind, body: &str) -> Result<Vec<String>> {
    let value: Value = serde_json::from_str(body).map_err(|_| {
        ModelError::new(
            FailureKind::BadResponse,
            tr(
                "返回内容不是 JSON，可能 Base URL 填错了",
                "The response isn't JSON; the base URL may be wrong",
            ),
        )
    })?;
    let (list_field, name_field) = match kind {
        ModelKind::Anthropic | ModelKind::OpenAiCompatible => ("data", "id"),
        ModelKind::Ollama => ("models", "name"),
    };
    let mut names: Vec<_> = value[list_field]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .map(|item| item[name_field].as_str().map(str::to_owned))
                .collect::<Option<_>>()
        })
        .ok_or_else(|| {
            ModelError::new(
                FailureKind::BadResponse,
                tr(
                    "返回的模型列表格式不正确",
                    "The model list response has an unexpected format",
                ),
            )
        })?;
    names.sort_unstable();
    names.dedup();
    Ok(names)
}

/// 取出回复文本。
pub fn parse_reply(kind: ModelKind, body: &str) -> Result<String> {
    let value: Value = serde_json::from_str(body).map_err(|err| {
        anyhow!(trf!(
            "返回内容不是 JSON，可能 Base URL 填错了：{err}",
            "The response isn't JSON; the base URL may be wrong: {err}"
        ))
    })?;
    let text = match kind {
        ModelKind::Anthropic => value["content"].as_array().map(|blocks| {
            blocks
                .iter()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect::<String>()
        }),
        ModelKind::OpenAiCompatible => value["choices"]
            .as_array()
            .and_then(|choices| choices.first())
            .and_then(|choice| choice["message"]["content"].as_str())
            .map(str::to_string),
        ModelKind::Ollama => value["message"]["content"].as_str().map(str::to_string),
    };
    text.ok_or_else(|| {
        anyhow!(trf!(
            "返回内容里没有找到回复文本：{}",
            "No reply text found in the response: {}",
            clip(body, 300)
        ))
    })
}

/// 服务端返回的错误原因：Anthropic / OpenAI 是 `error.message`，Ollama 是 `error`。
fn server_message(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let error = &value["error"];
    error["message"]
        .as_str()
        .or_else(|| error.as_str())
        .or_else(|| value["message"].as_str())
        .map(str::to_string)
}

/// 把 HTTP 状态码翻译成用户能看懂的话。
pub fn status_error(kind: ModelKind, status: u16, body: &str) -> String {
    let headline = match status {
        401 => tr("API Key 无效", "Invalid API key"),
        403 => tr(
            "API Key 没有权限访问这个模型",
            "This API key doesn't have access to this model",
        ),
        404 if kind == ModelKind::Ollama => tr(
            "本机 Ollama 里没有这个模型（先 ollama pull）",
            "This model isn't installed in Ollama (run ollama pull first)",
        ),
        404 => tr("地址或模型名不对", "Wrong address or model name"),
        408 => tr("服务端等待超时", "The server timed out"),
        413 => tr("文字太长了", "The text is too long"),
        429 => tr(
            "请求太频繁，或账户额度已用完",
            "Too many requests, or your account is out of credit",
        ),
        400..=499 => tr("请求被拒绝", "Request rejected"),
        529 => tr(
            "服务端繁忙，请稍后再试",
            "The server is busy; try again later",
        ),
        500..=599 => tr("服务端出错，请稍后再试", "Server error; try again later"),
        _ => tr("意外的响应", "Unexpected response"),
    };
    let detail = server_message(body).unwrap_or_else(|| clip(body.trim(), 200));
    if detail.is_empty() {
        trf!("{headline}（HTTP {status}）", "{headline} (HTTP {status})")
    } else {
        trf!(
            "{headline}（HTTP {status}）：{detail}",
            "{headline} (HTTP {status}): {detail}"
        )
    }
}

/// 网络层错误：连不上、域名解析失败、超时、证书……
fn transport_error(
    model: &ModelConfig,
    url: &str,
    timeout: Duration,
    err: ureq::Error,
) -> ModelError {
    let hint = if model.kind == ModelKind::Ollama {
        tr("，确认 Ollama 已经启动", "; make sure Ollama is running")
    } else {
        ""
    };
    let secs = timeout.as_secs();
    let timeout = || {
        trf!(
            "等了 {secs} 秒没有响应：{url}",
            "No response after {secs} seconds: {url}"
        )
    };
    let refused = || {
        trf!(
            "地址不可达，连不上 {url}{hint}",
            "Address unreachable: couldn't connect to {url}{hint}"
        )
    };
    let (kind, message) = match &err {
        ureq::Error::Timeout(_) => (FailureKind::Timeout, timeout()),
        ureq::Error::HostNotFound => (
            FailureKind::Unreachable,
            trf!(
                "地址不可达，找不到这个域名：{url}",
                "Address unreachable: couldn't find the host for {url}"
            ),
        ),
        ureq::Error::ConnectionFailed => (FailureKind::Unreachable, refused()),
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            (FailureKind::Unreachable, refused())
        }
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            (FailureKind::Timeout, timeout())
        }
        ureq::Error::BadUri(_) | ureq::Error::Http(_) => (
            FailureKind::Setup,
            trf!("Base URL 格式不对：{url}", "Invalid base URL: {url}"),
        ),
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_) => (
            FailureKind::Unreachable,
            trf!(
                "HTTPS 证书校验失败：{url}",
                "HTTPS certificate verification failed: {url}"
            ),
        ),
        _ => (
            FailureKind::Other,
            trf!("请求 {url} 失败", "Request to {url} failed"),
        ),
    };
    ModelError::new(kind, trf!("{message}（{err}）", "{message} ({err})"))
}

fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

fn ollama_tags_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let base = if base.is_empty() {
        default_base_url(ModelKind::Ollama)
    } else {
        base.strip_suffix("/api").unwrap_or(base)
    };
    format!("{base}/api/tags")
}

/// Ollama 是否在线：`GET {base}/api/tags`，2 秒超时。
pub fn ollama_online(base_url: &str) -> bool {
    agent(Duration::from_secs(2))
        .get(ollama_tags_url(base_url))
        .call()
        .is_ok_and(|response| response.status().is_success())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// 收到的请求：请求行、小写头、体。
    #[derive(Debug)]
    pub struct Received {
        pub line: String,
        pub headers: Vec<(String, String)>,
        pub body: Value,
    }

    impl Received {
        pub fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        }
    }

    /// 本地 mock：只接一个连接，回固定状态码和响应体，把收到的请求送回测试线程。
    pub fn mock(status: u16, body: &'static str) -> (String, mpsc::Receiver<Received>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut headers = Vec::new();
            let mut length = 0;
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                let (key, value) = header.split_once(':').unwrap();
                let (key, value) = (key.trim().to_lowercase(), value.trim().to_string());
                if key == "content-length" {
                    length = value.parse().unwrap();
                }
                headers.push((key, value));
            }
            let mut request = vec![0; length];
            reader.read_exact(&mut request).unwrap();
            let mut stream = stream;
            write!(
                stream,
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            tx.send(Received {
                line: line.trim_end().to_string(),
                headers,
                body: serde_json::from_slice(&request).unwrap_or(Value::Null),
            })
            .unwrap();
        });
        (format!("http://{addr}"), rx)
    }

    pub fn model(kind: ModelKind, base_url: &str, name: &str) -> ModelConfig {
        ModelConfig {
            id: "t".into(),
            name: "测试模型".into(),
            kind,
            base_url: base_url.into(),
            model: name.into(),
        }
    }

    const REQ: Request = Request {
        system: "翻译成英文",
        user: "你好",
        max_tokens: 64,
    };

    #[test]
    fn anthropic_request_matches_messages_api() {
        // 头或字段名写错，Anthropic 直接 400/401；用户只会看到「翻译失败」而不知道是我们的错。
        let (url, rx) = mock(
            200,
            r#"{"type":"message","content":[{"type":"text","text":"Hello"}]}"#,
        );
        let m = model(ModelKind::Anthropic, &url, "claude-haiku-5-5");
        assert_eq!(send(&m, Some("sk-ant-test"), &REQ).unwrap(), "Hello");
        let got = rx.recv().unwrap();
        assert_eq!(got.line, "POST /v1/messages HTTP/1.1");
        assert_eq!(got.header("x-api-key"), Some("sk-ant-test"));
        assert_eq!(got.header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(got.header("content-type"), Some("application/json"));
        assert_eq!(got.header("authorization"), None);
        assert_eq!(
            got.body,
            json!({
                "model": "claude-haiku-5-5",
                "max_tokens": 64,
                "system": "翻译成英文",
                "messages": [{"role": "user", "content": "你好"}],
            })
        );
    }

    #[test]
    fn anthropic_accepts_base_url_with_v1() {
        // 用户从文档里复制的地址常常带 /v1，不能拼成 /v1/v1/messages。
        let m = model(ModelKind::Anthropic, "https://api.anthropic.com/v1/", "x");
        let http = build(&m, Some("k"), &REQ).unwrap();
        assert_eq!(http.url, "https://api.anthropic.com/v1/messages");
    }

    #[test]
    fn openai_request_matches_chat_completions() {
        // DeepSeek 等兼容接口只认 system 角色与 Bearer 头；格式不对就接不上一大类模型。
        let (url, rx) = mock(
            200,
            r#"{"choices":[{"index":0,"message":{"role":"assistant","content":"Hi"}}]}"#,
        );
        let m = model(
            ModelKind::OpenAiCompatible,
            &format!("{url}/v1"),
            "deepseek-chat",
        );
        assert_eq!(send(&m, Some("sk-test"), &REQ).unwrap(), "Hi");
        let got = rx.recv().unwrap();
        assert_eq!(got.line, "POST /v1/chat/completions HTTP/1.1");
        assert_eq!(got.header("authorization"), Some("Bearer sk-test"));
        assert_eq!(
            got.body,
            json!({
                "model": "deepseek-chat",
                "messages": [
                    {"role": "system", "content": "翻译成英文"},
                    {"role": "user", "content": "你好"},
                ],
            })
        );
    }

    #[test]
    fn ollama_request_is_non_streaming_chat() {
        // 不传 stream:false 时 Ollama 返回 NDJSON 流，整体解析会失败。
        let (url, rx) = mock(
            200,
            r#"{"model":"qwen3","message":{"role":"assistant","content":"Bonjour"},"done":true}"#,
        );
        let m = model(ModelKind::Ollama, &url, "qwen3:8b");
        assert_eq!(send(&m, None, &REQ).unwrap(), "Bonjour");
        let got = rx.recv().unwrap();
        assert_eq!(got.line, "POST /api/chat HTTP/1.1");
        assert_eq!(got.header("authorization"), None);
        assert_eq!(got.body["stream"], json!(false));
        assert_eq!(got.body["model"], json!("qwen3:8b"));
        assert_eq!(
            got.body["messages"][0],
            json!({"role": "system", "content": "翻译成英文"})
        );
        assert_eq!(
            got.body["messages"][1],
            json!({"role": "user", "content": "你好"})
        );
    }

    #[test]
    fn unauthorized_says_key_is_invalid() {
        // 401 是最常见的配置错误，必须直接告诉用户「Key 无效」，并带上服务端原话。
        let (url, _rx) = mock(
            401,
            r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
        );
        let m = model(ModelKind::Anthropic, &url, "x");
        let err = send(&m, Some("bad"), &REQ).unwrap_err().to_string();
        assert_eq!(err, "API Key 无效（HTTP 401）：invalid x-api-key");
    }

    #[test]
    fn openai_errors_use_error_message() {
        let (url, _rx) = mock(
            429,
            r#"{"error":{"message":"Insufficient Balance","type":"x","code":"y"}}"#,
        );
        let m = model(ModelKind::OpenAiCompatible, &url, "x");
        let err = send(&m, Some("k"), &REQ).unwrap_err().to_string();
        assert_eq!(
            err,
            "请求太频繁，或账户额度已用完（HTTP 429）：Insufficient Balance"
        );
    }

    #[test]
    fn ollama_missing_model_suggests_pull() {
        let (url, _rx) = mock(
            404,
            r#"{"error":"model \"nope\" not found, try pulling it first"}"#,
        );
        let m = model(ModelKind::Ollama, &url, "nope");
        let err = send(&m, None, &REQ).unwrap_err().to_string();
        assert!(err.starts_with("本机 Ollama 里没有这个模型"), "{err}");
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn unreachable_address_says_so() {
        // 端口上没人监听：要说「地址不可达」，而不是一串 io error。
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let m = model(ModelKind::Ollama, &url, "qwen3");
        let err = send(&m, None, &REQ).unwrap_err().to_string();
        assert!(err.starts_with("地址不可达"), "{err}");
        assert!(err.contains("确认 Ollama 已经启动"), "{err}");
    }

    #[test]
    fn missing_key_fails_before_sending() {
        // 没填 Key 时不该把请求发出去，也不该显示成 401。
        let m = model(ModelKind::OpenAiCompatible, "http://127.0.0.1:9", "x");
        let err = send(&m, Some("  "), &REQ).unwrap_err().to_string();
        assert_eq!(err, "「测试模型」还没有填写 API Key");
    }

    #[test]
    fn errors_follow_english_interface() {
        // 测试连接、翻译失败时这些话直接显示给用户：英文界面下必须是英文，
        // 服务端原话、HTTP 状态码照旧附上（排查要靠它们）。
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            assert_eq!(
                status_error(
                    ModelKind::Anthropic,
                    401,
                    r#"{"error":{"message":"invalid x-api-key"}}"#
                ),
                "Invalid API key (HTTP 401): invalid x-api-key"
            );
            assert_eq!(
                status_error(ModelKind::Ollama, 404, ""),
                "This model isn't installed in Ollama (run ollama pull first) (HTTP 404)"
            );
            let m = model(ModelKind::OpenAiCompatible, "http://127.0.0.1:9", "x");
            let err = send(&m, None, &REQ).unwrap_err().to_string();
            assert_eq!(err, "Enter an API key for “测试模型”");
            let err = parse_reply(ModelKind::Ollama, "<html>")
                .unwrap_err()
                .to_string();
            assert!(err.starts_with("The response isn't JSON"), "{err}");
        });
    }

    #[test]
    fn failures_are_classified_for_the_ui() {
        // 界面按类别给出路：Key 无效 / 没填 Key 要「打开设置」，超时 / 限流 / 连不上要「重试」。
        // 类别丢了，用户只会看到一串报错而不知道下一步该干什么。
        let (url, _rx) = mock(401, r#"{"error":{"message":"bad key"}}"#);
        let err = send(&model(ModelKind::Ollama, &url, "x"), None, &REQ).unwrap_err();
        assert_eq!(failure_kind(&err), Some(FailureKind::Auth));
        let (url, _rx) = mock(429, r#"{"error":"slow down"}"#);
        let err = send(&model(ModelKind::Ollama, &url, "x"), None, &REQ).unwrap_err();
        assert_eq!(failure_kind(&err), Some(FailureKind::RateLimited));
        let (url, _rx) = mock(503, "");
        let err = send(&model(ModelKind::Ollama, &url, "x"), None, &REQ).unwrap_err();
        assert_eq!(failure_kind(&err), Some(FailureKind::Server));
        let m = model(ModelKind::OpenAiCompatible, "http://127.0.0.1:9", "x");
        let err = send(&m, None, &REQ).unwrap_err();
        assert_eq!(failure_kind(&err), Some(FailureKind::Setup));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let err = send(&model(ModelKind::Ollama, &dead, "x"), None, &REQ).unwrap_err();
        assert_eq!(failure_kind(&err), Some(FailureKind::Unreachable));
        // 加了上下文（「用 X 翻译失败」）也要认得出来。
        let wrapped = err.context("用「X」翻译失败");
        assert_eq!(failure_kind(&wrapped), Some(FailureKind::Unreachable));
        assert_eq!(failure_kind(&anyhow!("别的错误")), None);
    }

    #[test]
    fn garbage_reply_is_reported() {
        let (url, _rx) = mock(200, "<html>nginx</html>");
        let m = model(ModelKind::OpenAiCompatible, &url, "x");
        let err = send(&m, Some("k"), &REQ).unwrap_err().to_string();
        assert!(err.starts_with("返回内容不是 JSON"), "{err}");
    }

    #[test]
    fn openai_lists_models_without_a_configured_model_name() {
        // 新配置通常先拉列表再选模型；用相同的 /v1 Base URL 和 Bearer Key 就应当能查。
        let (url, rx) = mock(200, r#"{"data":[{"id":"z"},{"id":"a"},{"id":"z"}]}"#);
        let m = model(ModelKind::OpenAiCompatible, &format!("{url}/v1/"), "");
        assert_eq!(list(&m, Some(" sk-test ")).unwrap(), ["a", "z"]);
        let got = rx.recv().unwrap();
        assert_eq!(got.line, "GET /v1/models HTTP/1.1");
        assert_eq!(got.header("authorization"), Some("Bearer sk-test"));
        assert_eq!(got.header("x-api-key"), None);
        assert_eq!(got.body, Value::Null);
    }

    #[test]
    fn anthropic_lists_models_with_either_base_url() {
        // 旧配置和从文档复制来的地址都要拼成同一个路径；取 ID 而非显示名才能用于对话。
        for suffix in ["", "/v1/"] {
            let (url, rx) = mock(
                200,
                r#"{"data":[{"id":"z","display_name":"Claude"},{"id":"a"},{"id":"z"}]}"#,
            );
            let m = model(ModelKind::Anthropic, &format!("{url}{suffix}"), "");
            assert_eq!(list(&m, Some("sk-ant-test")).unwrap(), ["a", "z"]);
            let got = rx.recv().unwrap();
            assert_eq!(got.line, "GET /v1/models?limit=1000 HTTP/1.1");
            assert_eq!(got.header("x-api-key"), Some("sk-ant-test"));
            assert_eq!(got.header("anthropic-version"), Some("2023-06-01"));
            assert_eq!(got.header("authorization"), None);
        }
    }

    #[test]
    fn ollama_lists_local_tags_with_either_base_url() {
        // 和在线检查用同一个地址规则；本机 Ollama 无需鉴权，选中时需要保留完整标签。
        for suffix in ["", "/api/"] {
            let (url, rx) = mock(
                200,
                r#"{"models":[{"name":"qwen:8b","model":"ignored"},{"name":"llama:latest"},{"name":"qwen:8b"}]}"#,
            );
            let m = model(ModelKind::Ollama, &format!("{url}{suffix}"), "");
            assert_eq!(list(&m, None).unwrap(), ["llama:latest", "qwen:8b"]);
            let got = rx.recv().unwrap();
            assert_eq!(got.line, "GET /api/tags HTTP/1.1");
            assert_eq!(got.header("authorization"), None);
            assert_eq!(got.header("x-api-key"), None);
        }
    }

    #[test]
    fn model_list_distinguishes_empty_and_invalid_responses() {
        // 空数组是合法结果；HTML 或缺少模型名的 JSON 应提示响应格式错误。
        let kind = ModelKind::Ollama;
        let (url, _rx) = mock(200, r#"{"models":[]}"#);
        assert!(list(&model(kind, &url, ""), None).unwrap().is_empty());
        let (url, _rx) = mock(200, "<html>nginx</html>");
        let err = list(&model(kind, &url, ""), None).unwrap_err();
        assert_eq!(failure_kind(&err), Some(FailureKind::BadResponse));
        assert!(err.to_string().starts_with("返回内容不是 JSON"));
        assert!(parse_model_list(kind, r#"{"models":[{"name":null}]}"#).is_err());
        assert!(parse_model_list(kind, "{}").is_err());
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let err = parse_model_list(kind, "<html>").unwrap_err();
            assert!(err.to_string().starts_with("The response isn't JSON"));
        });
    }

    #[test]
    fn missing_key_for_model_list_is_a_setup_error() {
        // 用户还没填 Key 时在本地提示配置问题，别误报模型名缺失或发出匿名请求。
        for kind in [ModelKind::OpenAiCompatible, ModelKind::Anthropic] {
            let m = model(kind, "http://127.0.0.1:9", "");
            for key in [None, Some("  ")] {
                let err = build_list(&m, key).unwrap_err();
                assert_eq!(failure_kind(&err), Some(FailureKind::Setup));
                assert_eq!(err.to_string(), "「测试模型」还没有填写 API Key");
            }
        }
    }

    #[test]
    fn unavailable_model_list_suggests_manual_entry() {
        // 兼容服务商可能只实现聊天接口，遇到三种不支持状态都应该引导用户手填。
        for status in [404, 405, 501] {
            let (url, _rx) = mock(status, "");
            let m = model(ModelKind::OpenAiCompatible, &url, "");
            let err = list(&m, Some("sk-test")).unwrap_err();
            assert_eq!(failure_kind(&err), Some(FailureKind::BadResponse));
            assert_eq!(
                err.to_string(),
                "这个服务商不提供模型列表，请手动填写模型名"
            );
        }
        crate::i18n::with_lang(crate::i18n::Lang::En, || {
            let (url, _rx) = mock(404, "");
            let m = model(ModelKind::OpenAiCompatible, &url, "");
            let err = list(&m, Some("sk-test")).unwrap_err();
            assert_eq!(
                err.to_string(),
                "This provider doesn't list its models. Enter the model name manually."
            );
        });
    }

    #[test]
    fn model_list_errors_do_not_expose_the_api_key() {
        // 某些服务商会回显收到的 Key，界面和后续报错上报都不能带上它。
        const KEY: &str = "sk-do-not-echo-903f";
        let (url, _rx) = mock(
            401,
            r#"{"error":{"message":"Rejected API key sk-do-not-echo-903f"}}"#,
        );
        let m = model(ModelKind::OpenAiCompatible, &url, "");
        let err = list(&m, Some(KEY)).unwrap_err();
        assert_eq!(failure_kind(&err), Some(FailureKind::Auth));
        assert_eq!(err.to_string(), "API Key 无效（HTTP 401）");
        assert!(!format!("{err:?}").contains(KEY));

        // 网络错误可能带上无效 URL；即使用户把 Key 粘进地址，也不能在错误里回显。
        let m = model(ModelKind::OpenAiCompatible, &format!("bad url {KEY}"), "");
        let err = list(&m, Some(KEY)).unwrap_err();
        assert_eq!(failure_kind(&err), Some(FailureKind::Setup));
        assert!(!format!("{err:?}").contains(KEY));
    }

    #[test]
    fn ollama_online_checks_tags() {
        let (url, rx) = mock(200, r#"{"models":[]}"#);
        assert!(ollama_online(&url));
        assert_eq!(rx.recv().unwrap().line, "GET /api/tags HTTP/1.1");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        assert!(!ollama_online(&dead));
    }
}
