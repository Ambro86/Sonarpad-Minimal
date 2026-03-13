use anyhow::{Result, anyhow};
use chrono::Local;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async, tungstenite::client::IntoClientRequest,
    tungstenite::http::HeaderValue, tungstenite::protocol::Message,
};
use url::Url;
use uuid::Uuid;

pub const TRUSTED_CLIENT_TOKEN: &str = "6A5AA1D4EAFF4E9FB37E23D68491D6F4";
pub const WSS_URL_BASE: &str =
    "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";
pub const VOICE_LIST_URL: &str =
    "https://speech.platform.bing.com/consumer/speech/synthesize/readaloud/voices/list";
pub const EDGE_TTS_MAX_BYTES: usize = 800;
pub const EDGE_TTS_REALTIME_MAX_BYTES: usize = EDGE_TTS_MAX_BYTES;
const EDGE_TTS_SEND_TIMEOUT: Duration = Duration::from_secs(5);
const EDGE_TTS_READ_TIMEOUT: Duration = Duration::from_secs(3);

fn retry_backoff_delay(attempt: usize) -> Duration {
    Duration::from_millis((500 * attempt as u64).min(3_000))
}

fn is_retry_forever_timeout(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    message.contains("WebSocket connect timeout")
        || message.contains("send timeout")
        || message.contains("audio read timeout")
        || message.contains("Realtime speech.config send timeout")
        || message.contains("Realtime SSML send timeout")
        || message.contains("Realtime audio read timeout")
}

type EdgeSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type EdgeSocketWriter = futures_util::stream::SplitSink<EdgeSocket, Message>;
type EdgeSocketReader = futures_util::stream::SplitStream<EdgeSocket>;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct VoiceInfo {
    #[serde(rename = "ShortName")]
    pub short_name: String,
    #[serde(rename = "FriendlyName")]
    pub friendly_name: String,
    #[serde(rename = "Locale")]
    pub locale: String,
    #[serde(rename = "SuggestedCodec")]
    pub suggested_codec: String,
}

pub async fn get_edge_voices() -> Result<Vec<VoiceInfo>> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36 Edg/132.0.0.0")
        .build()?;

    let url = format!(
        "{}?TrustedClientToken={}",
        VOICE_LIST_URL, TRUSTED_CLIENT_TOKEN
    );
    let res = client
        .get(url)
        .send()
        .await?
        .json::<Vec<VoiceInfo>>()
        .await?;
    Ok(res)
}

pub async fn synthesize_text_with_retry(
    text: &str,
    voice: &str,
    rate: i32,
    pitch: i32,
    volume: i32,
    max_retries: usize,
) -> Result<Vec<u8>> {
    let mut last_err = anyhow!("Sintesi fallita");
    let mut attempt = 1usize;
    loop {
        match synthesize_text_chunk(text, voice, rate, pitch, volume).await {
            Ok(data) => {
                if data.is_empty() {
                    return Ok(Vec::new());
                }
                return Ok(data);
            }
            Err(e) => {
                let retry_forever = is_retry_forever_timeout(&e);
                println!(
                    "DEBUG: Tentativo {}/{} fallito: {}",
                    attempt,
                    if retry_forever {
                        "inf".to_string()
                    } else {
                        max_retries.to_string()
                    },
                    e
                );
                last_err = e;
                if retry_forever || attempt < max_retries {
                    tokio::time::sleep(retry_backoff_delay(attempt)).await;
                    attempt += 1;
                    continue;
                }
                return Err(last_err);
            }
        }
    }
}

pub struct EdgeRealtimeSession {
    write: EdgeSocketWriter,
    read: EdgeSocketReader,
}

fn format_rate(rate: i32) -> String {
    format!("{:+}%", rate)
}

fn format_pitch(pitch: i32) -> String {
    format!("{:+}Hz", pitch)
}

fn format_volume(volume: i32) -> String {
    let delta = volume.saturating_sub(100);
    format!("{:+}%", delta)
}

pub async fn synthesize_text_chunk(
    text: &str,
    voice: &str,
    rate: i32,
    pitch: i32,
    volume: i32,
) -> Result<Vec<u8>> {
    let normalized_text = normalize_for_tts(text);
    if normalized_text.is_empty() {
        return Ok(Vec::new());
    }
    let request_id = Uuid::new_v4().simple().to_string().to_uppercase();
    let sec_ms_gec = generate_sec_ms_gec();
    let sec_ms_gec_version = "1-132.0.2917.39";

    let url_str = format!(
        "{}?TrustedClientToken={}&ConnectionId={}&Sec-MS-GEC={}&Sec-MS-GEC-Version={}",
        WSS_URL_BASE, TRUSTED_CLIENT_TOKEN, request_id, sec_ms_gec, sec_ms_gec_version
    );
    let url = Url::parse(&url_str)?;

    let mut request = url.as_str().into_client_request()?;
    let headers = request.headers_mut();
    headers.insert("Pragma", HeaderValue::from_static("no-cache"));
    headers.insert("Cache-Control", HeaderValue::from_static("no-cache"));
    headers.insert(
        "Origin",
        HeaderValue::from_static("chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold"),
    );
    headers.insert("User-Agent", HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36 Edg/132.0.0.0"));

    let cookie = format!("muid={};", generate_muid());
    headers.insert("Cookie", HeaderValue::from_str(&cookie)?);

    let (ws_stream, _) = tokio::time::timeout(Duration::from_secs(10), connect_async(request))
        .await
        .map_err(|_| anyhow!("WebSocket connect timeout"))??;

    let (mut write, mut read) = ws_stream.split::<Message>();

    let config_msg = format!(
        "X-Timestamp:{}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n{{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"false\"}},\"outputFormat\":\"audio-24khz-48kbitrate-mono-mp3\"}}}}}}}}",
        get_date_string()
    );
    tokio::time::timeout(
        EDGE_TTS_SEND_TIMEOUT,
        write.send(Message::Text(config_msg.into())),
    )
    .await
    .map_err(|_| anyhow!("speech.config send timeout"))??;

    let lang = voice.split('-').collect::<Vec<_>>();
    let lang = if lang.len() >= 3 {
        lang[0..2].join("-")
    } else {
        "en-US".to_string()
    };

    let ssml = format!(
        "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{}'><voice name='{}'><prosody pitch='{}' rate='{}' volume='{}'>{}</prosody></voice></speak>",
        lang,
        voice,
        format_pitch(pitch),
        format_rate(rate),
        format_volume(volume),
        escape_xml(&normalized_text)
    );
    let ssml_msg = format!(
        "X-RequestId:{}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{}Z\r\nPath:ssml\r\n\r\n{}",
        request_id,
        get_date_string(),
        ssml
    );
    tokio::time::timeout(
        EDGE_TTS_SEND_TIMEOUT,
        write.send(Message::Text(ssml_msg.into())),
    )
    .await
    .map_err(|_| anyhow!("SSML send timeout"))??;

    let mut audio_data = Vec::new();
    loop {
        let maybe_msg = tokio::time::timeout(EDGE_TTS_READ_TIMEOUT, read.next())
            .await
            .map_err(|_| anyhow!("audio read timeout"))?;
        let Some(msg) = maybe_msg else {
            break;
        };
        let msg = msg?;
        match msg {
            Message::Text(t) => {
                if t.contains("Path:turn.end") {
                    break;
                }
            }
            Message::Binary(data) => {
                if let Some(audio) = parse_edge_binary_audio_payload(&data)? {
                    audio_data.extend_from_slice(&audio);
                }
            }
            _ => {}
        }
    }

    Ok(audio_data)
}

impl EdgeRealtimeSession {
    pub async fn connect() -> Result<Self> {
        let request_id = Uuid::new_v4().simple().to_string().to_uppercase();
        let sec_ms_gec = generate_sec_ms_gec();
        let sec_ms_gec_version = "1-132.0.2917.39";

        let url_str = format!(
            "{}?TrustedClientToken={}&ConnectionId={}&Sec-MS-GEC={}&Sec-MS-GEC-Version={}",
            WSS_URL_BASE, TRUSTED_CLIENT_TOKEN, request_id, sec_ms_gec, sec_ms_gec_version
        );
        let url = Url::parse(&url_str)?;

        let mut request = url.as_str().into_client_request()?;
        let headers = request.headers_mut();
        headers.insert("Pragma", HeaderValue::from_static("no-cache"));
        headers.insert("Cache-Control", HeaderValue::from_static("no-cache"));
        headers.insert(
            "Origin",
            HeaderValue::from_static("chrome-extension://jdiccldimpdaibmpdkjnbmckianbfold"),
        );
        headers.insert("User-Agent", HeaderValue::from_static("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/132.0.0.0 Safari/537.36 Edg/132.0.0.0"));
        headers.insert(
            "Cookie",
            HeaderValue::from_str(&format!("muid={};", generate_muid()))?,
        );

        let (ws_stream, _) = tokio::time::timeout(Duration::from_secs(10), connect_async(request))
            .await
            .map_err(|_| anyhow!("WebSocket connect timeout"))??;
        let (mut write, read) = ws_stream.split::<Message>();

        let config_msg = format!(
            "X-Timestamp:{}\r\nContent-Type:application/json; charset=utf-8\r\nPath:speech.config\r\n\r\n{{\"context\":{{\"synthesis\":{{\"audio\":{{\"metadataoptions\":{{\"sentenceBoundaryEnabled\":\"false\",\"wordBoundaryEnabled\":\"false\"}},\"outputFormat\":\"audio-24khz-48kbitrate-mono-mp3\"}}}}}}}}",
            get_date_string()
        );
        tokio::time::timeout(
            EDGE_TTS_SEND_TIMEOUT,
            write.send(Message::Text(config_msg.into())),
        )
        .await
        .map_err(|_| anyhow!("Realtime speech.config send timeout"))??;

        Ok(Self { write, read })
    }

    pub async fn synthesize_chunk(
        &mut self,
        text: &str,
        voice: &str,
        rate: i32,
        pitch: i32,
        volume: i32,
    ) -> Result<Vec<u8>> {
        let normalized_text = normalize_for_tts(text);
        if normalized_text.is_empty() {
            return Ok(Vec::new());
        }

        let lang = voice.split('-').collect::<Vec<_>>();
        let lang = if lang.len() >= 3 {
            lang[0..2].join("-")
        } else {
            "en-US".to_string()
        };

        let ssml = format!(
            "<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='{}'><voice name='{}'><prosody pitch='{}' rate='{}' volume='{}'>{}</prosody></voice></speak>",
            lang,
            voice,
            format_pitch(pitch),
            format_rate(rate),
            format_volume(volume),
            escape_xml(&normalized_text)
        );
        let ssml_msg = format!(
            "X-RequestId:{}\r\nContent-Type:application/ssml+xml\r\nX-Timestamp:{}Z\r\nPath:ssml\r\n\r\n{}",
            Uuid::new_v4().simple(),
            get_date_string(),
            ssml
        );
        tokio::time::timeout(
            EDGE_TTS_SEND_TIMEOUT,
            self.write.send(Message::Text(ssml_msg.into())),
        )
        .await
        .map_err(|_| anyhow!("Realtime SSML send timeout"))??;

        let mut audio_data = Vec::new();
        loop {
            let maybe_msg = tokio::time::timeout(EDGE_TTS_READ_TIMEOUT, self.read.next())
                .await
                .map_err(|_| anyhow!("Realtime audio read timeout"))?;
            let Some(msg) = maybe_msg else {
                break;
            };
            let msg = msg?;
            match msg {
                Message::Text(t) => {
                    if t.contains("Path:turn.end") {
                        break;
                    }
                }
                Message::Binary(data) => {
                    if let Some(audio) = parse_edge_binary_audio_payload(&data)? {
                        audio_data.extend_from_slice(&audio);
                    }
                }
                _ => {}
            }
        }

        Ok(audio_data)
    }
}

pub async fn synthesize_realtime_chunk_with_retry(
    session: Option<EdgeRealtimeSession>,
    text: &str,
    voice: &str,
    rate: i32,
    pitch: i32,
    volume: i32,
    max_retries: usize,
) -> Result<(Vec<u8>, Option<EdgeRealtimeSession>)> {
    let mut session = session;
    let mut last_err = anyhow!("Sintesi realtime fallita");

    for attempt in 1..=max_retries {
        if session.is_none() {
            match EdgeRealtimeSession::connect().await {
                Ok(new_session) => session = Some(new_session),
                Err(err) => {
                    last_err = err;
                    if attempt < max_retries {
                        tokio::time::sleep(retry_backoff_delay(attempt)).await;
                    }
                    continue;
                }
            }
        }

        let result = if let Some(active_session) = session.as_mut() {
            active_session
                .synthesize_chunk(text, voice, rate, pitch, volume)
                .await
        } else {
            Err(anyhow!("Sessione realtime Edge non disponibile"))
        };

        match result {
            Ok(data) => {
                if data.is_empty() {
                    return Ok((Vec::new(), session));
                }
                return Ok((data, session));
            }
            Err(err) => {
                println!(
                    "DEBUG: Tentativo realtime {}/{} fallito: {}",
                    attempt, max_retries, err
                );
                last_err = err;
                session = None;
                if attempt < max_retries {
                    tokio::time::sleep(retry_backoff_delay(attempt)).await;
                }
            }
        }
    }

    Err(last_err)
}

fn generate_sec_ms_gec() -> String {
    let win_epoch = 11644473600i64;
    let ticks = Local::now().timestamp() + win_epoch;
    let ticks = (ticks - (ticks % 300)) * 10_000_000;
    let str_to_hash = format!("{}{}", ticks, TRUSTED_CLIENT_TOKEN);
    let mut hasher = Sha256::new();
    hasher.update(str_to_hash);
    hex::encode(hasher.finalize()).to_uppercase()
}

fn generate_muid() -> String {
    let mut rng = rand::thread_rng();
    let mut bytes = [0u8; 16];
    rng.fill(&mut bytes);
    hex::encode(bytes).to_uppercase()
}

fn get_date_string() -> String {
    Local::now()
        .format("%a %b %d %Y %H:%M:%S GMT+0000 (Coordinated Universal Time)")
        .to_string()
}

pub fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn normalize_for_tts(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(normalized.len());
    let mut chars = normalized.chars().peekable();
    let mut pending_space = false;

    while let Some(ch) = chars.next() {
        if ch == '\n' {
            let mut newline_count = 1usize;
            while chars.peek().is_some_and(|next| *next == '\n') {
                newline_count += 1;
                chars.next();
            }
            if newline_count >= 2 {
                while out.ends_with(' ') {
                    out.pop();
                }
                if !out.is_empty() && !out.ends_with("\n\n") {
                    out.push('\n');
                    out.push('\n');
                }
                pending_space = false;
            } else if !out.is_empty() {
                pending_space = true;
            }
            continue;
        }

        if ch.is_whitespace() {
            if !out.is_empty() {
                pending_space = true;
            }
            continue;
        }

        if pending_space
            && ch.is_uppercase()
            && out.ends_with('.')
            && !out.ends_with("..")
            && !out.ends_with(". ")
        {
            out.push(' ');
            pending_space = false;
        }

        if pending_space && !out.ends_with([' ', '\n']) {
            out.push(' ');
        }
        pending_space = false;
        out.push(ch);
    }

    out.trim().to_string()
}

fn parse_edge_binary_audio_payload(data: &[u8]) -> Result<Option<Vec<u8>>> {
    if data.len() < 2 {
        return Err(anyhow!("Edge WS: binary frame missing header length"));
    }

    let header_len = u16::from_be_bytes([data[0], data[1]]) as usize;
    if data.len() < header_len + 2 {
        return Err(anyhow!("Edge WS: invalid binary header length"));
    }

    let header_bytes = &data[2..2 + header_len];
    let payload = &data[2 + header_len..];
    let header_text = String::from_utf8_lossy(header_bytes);

    if header_text.contains("Path:audio") {
        Ok(Some(payload.to_vec()))
    } else {
        Ok(None)
    }
}

// --- Logica di segmentazione (da Sonarpad) ---

pub fn split_sentences_lazy(text: &str) -> impl Iterator<Item = &str> {
    let mut start = 0;
    let mut iter = text.char_indices().peekable();

    std::iter::from_fn(move || {
        if start >= text.len() {
            return None;
        }
        while let Some((idx, ch)) = iter.next() {
            if matches!(ch, '.' | '!' | '?' | ';' | ':') {
                let next_ch = iter.peek().map(|(_, c)| *c);
                if ch == '.' && matches!(next_ch, Some('.')) {
                    continue;
                }
                let next_is_space = iter.peek().map(|(_, c)| c.is_whitespace()).unwrap_or(true);
                if next_is_space {
                    let end = idx + ch.len_utf8();
                    if end > start {
                        let sentence = &text[start..end];
                        if sentence.chars().any(|c| c.is_alphanumeric()) {
                            start = end;
                            return Some(sentence);
                        }
                        start = end;
                    }
                }
            }
        }
        let sentence = &text[start..];
        start = text.len();
        if sentence.chars().any(|c| c.is_alphanumeric()) {
            Some(sentence)
        } else {
            None
        }
    })
}

fn split_long_text_by_whitespace(text: &str, max_bytes: usize) -> VecDeque<String> {
    let mut out = VecDeque::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if current.is_empty() {
            if word.len() > max_bytes {
                out.push_back(word.to_string());
            } else {
                current.push_str(word);
            }
            continue;
        }

        if current.len() + 1 + word.len() <= max_bytes {
            current.push(' ');
            current.push_str(word);
        } else {
            out.push_back(std::mem::take(&mut current));
            if word.len() > max_bytes {
                out.push_back(word.to_string());
            } else {
                current.push_str(word);
            }
        }
    }

    if !current.is_empty() {
        out.push_back(current);
    }

    out
}

fn split_text_lazy_with_limit<'a>(
    text: &'a str,
    max_bytes: usize,
) -> impl Iterator<Item = String> + 'a {
    let normalized = normalize_for_tts(text);
    let mut sentences: VecDeque<String> = split_sentences_lazy(&normalized)
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .map(ToString::to_string)
        .collect();
    let mut current = String::new();
    let mut pending = VecDeque::<String>::new();

    std::iter::from_fn(move || {
        if let Some(part) = pending.pop_front() {
            return Some(part);
        }

        while let Some(sentence) = sentences.pop_front() {
            if sentence.len() > max_bytes {
                if !current.is_empty() {
                    let res = std::mem::take(&mut current);
                    pending.extend(split_long_text_by_whitespace(&sentence, max_bytes));
                    return Some(res);
                }

                pending.extend(split_long_text_by_whitespace(&sentence, max_bytes));
                if let Some(part) = pending.pop_front() {
                    return Some(part);
                }
                continue;
            }

            if current.is_empty() {
                current.push_str(&sentence);
            } else if current.len() + 1 + sentence.len() > max_bytes {
                let res = std::mem::take(&mut current);
                current.push_str(&sentence);
                return Some(res);
            } else {
                current.push(' ');
                current.push_str(&sentence);
            }
        }

        if !current.is_empty() {
            return Some(std::mem::take(&mut current));
        }
        None
    })
}

pub fn split_text_lazy<'a>(text: &'a str) -> impl Iterator<Item = String> + 'a {
    split_text_lazy_with_limit(text, EDGE_TTS_MAX_BYTES)
}

pub fn split_text_realtime_lazy<'a>(text: &'a str) -> impl Iterator<Item = String> + 'a {
    split_text_lazy_with_limit(text, EDGE_TTS_REALTIME_MAX_BYTES)
}
