use encoding_rs::{Encoding, WINDOWS_1252};
use feed_rs::parser;
use serde::{Deserialize, Serialize};
use std::io::Cursor;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

const DEFAULT_IT_FEEDS: &str = include_str!("../i18n/feed_it.txt");

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ArticleItem {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArticleSource {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub items: Vec<ArticleItem>,
}

pub fn default_italian_sources() -> Vec<ArticleSource> {
    DEFAULT_IT_FEEDS
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| {
            let (title, url) = line.split_once('|')?;
            let title = title.trim();
            let url = url.trim();
            if url.is_empty() {
                return None;
            }
            Some(ArticleSource {
                title: if title.is_empty() {
                    url.to_string()
                } else {
                    title.to_string()
                },
                url: normalize_url(url),
                items: Vec::new(),
            })
        })
        .collect()
}

pub fn normalize_url(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() {
        return String::new();
    }
    if s.starts_with("//") {
        return format!("https:{s}");
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        return s.to_string();
    }
    format!("https://{s}")
}

#[cfg(target_os = "macos")]
pub fn bundled_curl_impersonate_libraries() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        candidates.push(exe_dir.to_path_buf());

        #[cfg(target_os = "macos")]
        if let Some(contents_dir) = exe_dir.parent() {
            candidates.push(contents_dir.join("Frameworks"));
            candidates.push(contents_dir.join("MacOS"));
        }
    }

    if let Ok(current_dir) = std::env::current_dir() {
        candidates.push(current_dir);
    }

    let mut dylibs = Vec::new();
    for dir in candidates {
        if !dir.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("libcurl") && name.ends_with(".dylib"))
                {
                    dylibs.push(path);
                }
            }
        }
    }

    dylibs.sort();
    dylibs.dedup();
    dylibs
}

pub async fn fetch_source(source: &ArticleSource) -> Result<ArticleSource, String> {
    let url = normalize_url(&source.url);
    if url.is_empty() {
        return Err("URL fonte vuoto".to_string());
    }

    let client = reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|err| err.to_string())?;

    let bytes = client
        .get(&url)
        .send()
        .await
        .map_err(|err| err.to_string())?
        .error_for_status()
        .map_err(|err| err.to_string())?
        .bytes()
        .await
        .map_err(|err| err.to_string())?;

    let (title, items) = parse_feed_bytes(bytes.to_vec(), &source.title)?;
    Ok(ArticleSource { title, url, items })
}

pub async fn fetch_article_text(item: &ArticleItem) -> Result<String, String> {
    let mut url = normalize_url(&item.link);
    if url.is_empty() {
        return Err("URL articolo vuoto".to_string());
    }

    if is_google_news_article_url(&url) {
        let original = url.clone();
        if let Ok(Some(resolved_url)) =
            tokio::task::spawn_blocking(move || resolve_google_news_article_url_blocking(&original))
                .await
                .map_err(|err| err.to_string())?
        {
            url = resolved_url;
        }
    }

    let reqwest_article = fetch_article_text_via_reqwest(&url, item).await?;
    if !should_retry_with_impersonation(
        &reqwest_article.html,
        &reqwest_article.article,
        item.description.trim(),
    ) {
        return Ok(format_article_text(&reqwest_article.article));
    }

    let curl_url = url.clone();
    let curl_item = item.clone();
    let curl_article =
        tokio::task::spawn_blocking(move || fetch_article_text_via_curl(&curl_url, &curl_item))
            .await
            .map_err(|err| err.to_string())??;
    if !should_retry_with_impersonation(
        &curl_article.html,
        &curl_article.article,
        item.description.trim(),
    ) {
        return Ok(format_article_text(&curl_article.article));
    }

    let iphone_url = url.clone();
    let iphone_item = item.clone();
    let iphone_article = tokio::task::spawn_blocking(move || {
        fetch_article_text_via_iphone_curl(&iphone_url, &iphone_item)
    })
    .await
    .map_err(|err| err.to_string())??;

    if extracted_len(&iphone_article.article.content) > extracted_len(&curl_article.article.content)
    {
        Ok(format_article_text(&iphone_article.article))
    } else {
        Ok(format_article_text(&curl_article.article))
    }
}

struct ArticleFetchAttempt {
    html: String,
    article: crate::reader::ArticleContent,
}

async fn fetch_article_text_via_reqwest(
    url: &str,
    item: &ArticleItem,
) -> Result<ArticleFetchAttempt, String> {
    let client = reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|err| err.to_string())?;

    let bytes = client
        .get(url)
        .send()
        .await
        .map_err(|err| err.to_string())?
        .error_for_status()
        .map_err(|err| err.to_string())?
        .bytes()
        .await
        .map_err(|err| err.to_string())?;

    Ok(build_article_attempt(bytes.to_vec(), item))
}

fn fetch_article_text_via_curl(
    url: &str,
    item: &ArticleItem,
) -> Result<ArticleFetchAttempt, String> {
    let bytes = crate::curl_client::CurlClient::fetch_url_impersonated(url)?;
    Ok(build_article_attempt(bytes, item))
}

fn fetch_article_text_via_iphone_curl(
    url: &str,
    item: &ArticleItem,
) -> Result<ArticleFetchAttempt, String> {
    let bytes = crate::curl_client::CurlClient::fetch_url_iphone_impersonated(url)?;
    Ok(build_article_attempt(bytes, item))
}

fn build_article_attempt(bytes: Vec<u8>, item: &ArticleItem) -> ArticleFetchAttempt {
    let html = decode_html_bytes(&bytes);
    let article =
        crate::reader::reader_mode_extract(&html).unwrap_or(crate::reader::ArticleContent {
            title: item.title.trim().to_string(),
            content: item.description.trim().to_string(),
        });
    ArticleFetchAttempt { html, article }
}

fn format_article_text(article: &crate::reader::ArticleContent) -> String {
    format!("{}\n\n{}", article.title.trim(), article.content.trim())
}

fn should_retry_with_impersonation(
    html: &str,
    article: &crate::reader::ArticleContent,
    fallback_description: &str,
) -> bool {
    page_looks_blocked(html) || extraction_is_weak(&article.content, fallback_description)
}

fn extraction_is_weak(content: &str, fallback_description: &str) -> bool {
    let trimmed = content.trim();
    trimmed.is_empty()
        || extracted_len(trimmed) < 80
        || (!fallback_description.trim().is_empty() && trimmed == fallback_description.trim())
}

fn extracted_len(content: &str) -> usize {
    content.chars().filter(|ch| !ch.is_whitespace()).count()
}

fn page_looks_blocked(html: &str) -> bool {
    let text = html.to_ascii_lowercase();
    let markers = [
        "just a moment",
        "captcha",
        "cf-browser-verification",
        "attention required",
        "enable js",
        "enable javascript",
        "subscribe to continue",
        "sign in to continue",
        "access denied",
        "bot detection",
    ];
    markers.iter().any(|marker| text.contains(marker))
}

fn is_google_news_article_url(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url.trim()) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or("");
    if !host.eq_ignore_ascii_case("news.google.com") {
        return false;
    }
    let path = parsed.path().to_ascii_lowercase();
    path.contains("/rss/articles/")
        || path.contains("/articles/")
        || path.contains("/read/")
        || path.contains("/__i/rss/rd/articles/")
}

fn extract_between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = s.find(start)? + start.len();
    let rest = &s[from..];
    let to = rest.find(end)?;
    Some(&rest[..to])
}

fn extract_google_news_article_id(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let mut segments = parsed.path_segments()?;
    let segments: Vec<&str> = segments.by_ref().collect();
    let pos = segments
        .iter()
        .position(|seg| seg.eq_ignore_ascii_case("articles"))?;
    let id = segments.get(pos + 1)?.trim();
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

fn extract_google_news_tokens(html: &str) -> Option<(String, String)> {
    let signature = extract_between(html, "data-n-a-sg=\"", "\"")
        .or_else(|| extract_between(html, "data-n-a-sg='", "'"))?
        .trim()
        .to_string();
    let timestamp = extract_between(html, "data-n-a-ts=\"", "\"")
        .or_else(|| extract_between(html, "data-n-a-ts='", "'"))?
        .trim()
        .to_string();
    if signature.is_empty() || timestamp.is_empty() {
        None
    } else {
        Some((signature, timestamp))
    }
}

fn extract_google_news_direct_url_from_article_html(html: &str) -> Option<String> {
    let candidate = extract_between(html, "data-n-au=\"", "\"")
        .or_else(|| extract_between(html, "data-n-au='", "'"))
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let parsed = Url::parse(candidate).ok()?;
    match parsed.scheme() {
        "http" | "https" => {
            if is_google_news_article_url(candidate) {
                None
            } else {
                Some(candidate.to_string())
            }
        }
        _ => None,
    }
}

fn fetch_google_news_article_page_html(url: &str) -> Result<String, String> {
    let html = String::from_utf8_lossy(&crate::curl_client::CurlClient::fetch_url_impersonated(
        url,
    )?)
    .to_string();
    if extract_google_news_tokens(&html).is_some() {
        return Ok(html);
    }

    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|err| err.to_string())?
        .get(url)
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|err| err.to_string())?
        .text()
        .map_err(|err| err.to_string())
}

fn encode_form_value(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn extract_decoded_google_news_url(response: &str) -> Option<String> {
    let normalized = response.replace("\\\"", "\"").replace("\\/", "/");
    let url = extract_between(&normalized, "[\"garturlres\",\"", "\",")?.trim();
    let parsed = Url::parse(url).ok()?;
    match parsed.scheme() {
        "http" | "https" => Some(url.to_string()),
        _ => None,
    }
}

fn post_google_news_batchexecute_reqwest(body: &str) -> Result<String, String> {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|err| err.to_string())?
        .post("https://news.google.com/_/DotsSplashUi/data/batchexecute?rpcids=Fbv4je")
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded;charset=UTF-8",
        )
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        )
        .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en;q=0.9")
        .header(reqwest::header::REFERER, "https://news.google.com/")
        .header(reqwest::header::ORIGIN, "https://news.google.com")
        .header("X-Same-Domain", "1")
        .body(body.to_string())
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|err| err.to_string())?
        .text()
        .map_err(|err| err.to_string())
}

fn post_google_news_batchexecute_curl(body: &str) -> Result<String, String> {
    let headers = [
        "Content-Type: application/x-www-form-urlencoded;charset=UTF-8",
        "User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36",
        "Accept-Language: en-US,en;q=0.9",
        "Referer: https://news.google.com/",
        "Origin: https://news.google.com",
        "X-Same-Domain: 1",
    ];
    let bytes = crate::curl_client::CurlClient::post_form_impersonated(
        "https://news.google.com/_/DotsSplashUi/data/batchexecute?rpcids=Fbv4je",
        body,
        &headers,
    )?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn resolve_google_news_article_url_blocking(url: &str) -> Result<Option<String>, String> {
    if !is_google_news_article_url(url) {
        return Ok(None);
    }
    let article_id = match extract_google_news_article_id(url) {
        Some(id) => id,
        None => return Ok(None),
    };

    let html = fetch_google_news_article_page_html(url)?;
    if let Some(decoded) = extract_google_news_direct_url_from_article_html(&html) {
        return Ok(Some(decoded));
    }
    let (signature, timestamp) = match extract_google_news_tokens(&html) {
        Some(tokens) => tokens,
        None => return Ok(None),
    };

    let req_inner = format!(
        r#"["garturlreq",[["en-US","US",["WEB_TEST_1_0_0"],null,null,1,1,"US:en",null,180,null,null,null,null,null,0,null,null,[1608992183,723341000]],"en-US","US",1,[2,3,4,8],1,0,"655000234",0,0,null,0],"{article_id}",{timestamp},"{signature}"]"#
    );
    let req_inner_json = serde_json::to_string(&req_inner).map_err(|err| err.to_string())?;
    let f_req = format!(r#"[[["Fbv4je",{}]]]"#, req_inner_json);
    let body = format!("f.req={}", encode_form_value(&f_req));

    let mut response = None;
    for _ in 0..2 {
        if let Ok(text) = post_google_news_batchexecute_reqwest(&body) {
            response = Some(text);
            break;
        }
        std::thread::sleep(Duration::from_millis(350));
    }
    if response.is_none()
        && let Ok(text) = post_google_news_batchexecute_curl(&body)
    {
        response = Some(text);
    }

    let Some(response) = response else {
        return Ok(None);
    };
    let Some(decoded) = extract_decoded_google_news_url(&response) else {
        return Ok(None);
    };
    if decoded == url {
        return Ok(None);
    }
    Ok(Some(decoded))
}

fn parse_feed_bytes(
    bytes: Vec<u8>,
    fallback_title: &str,
) -> Result<(String, Vec<ArticleItem>), String> {
    let cursor = Cursor::new(bytes);
    let feed = parser::parse(cursor).map_err(|err| err.to_string())?;
    let title = feed
        .title
        .map(|title| decode_basic_html_entities(&title.content))
        .unwrap_or_else(|| fallback_title.to_string());

    let mut items = Vec::new();
    for entry in feed.entries {
        let title_value = entry
            .title
            .as_ref()
            .map(|value| value.content.clone())
            .unwrap_or_else(|| "Articolo senza titolo".to_string());
        let title_value = decode_basic_html_entities(&title_value);

        let link = select_entry_link(&entry)
            .or_else(|| {
                let entry_id = entry.id.trim();
                if entry_id.starts_with("http://") || entry_id.starts_with("https://") {
                    Some(entry_id.to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let link = normalize_url(&link);
        if link.is_empty() {
            continue;
        }

        let description = entry
            .summary
            .as_ref()
            .map(|value| decode_basic_html_entities(&value.content))
            .unwrap_or_default();

        items.push(ArticleItem {
            title: title_value,
            link,
            description,
        });
    }

    dedup_items(&mut items);
    Ok((title, items))
}

fn select_entry_link(entry: &feed_rs::model::Entry) -> Option<String> {
    for link in &entry.links {
        let href = link.href.trim();
        if href.is_empty() {
            continue;
        }
        let rel = link.rel.as_deref().unwrap_or("");
        if rel.is_empty() || rel.eq_ignore_ascii_case("alternate") {
            return Some(href.to_string());
        }
    }

    entry
        .links
        .iter()
        .find(|link| !link.href.trim().is_empty())
        .map(|link| link.href.trim().to_string())
}

fn dedup_items(items: &mut Vec<ArticleItem>) {
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| {
        let key = canonicalize_url(&item.link);
        if key.is_empty() || seen.contains(&key) {
            return false;
        }
        seen.insert(key);
        true
    });
}

fn canonicalize_url(input: &str) -> String {
    let normalized = normalize_url(input);
    if let Ok(mut url) = Url::parse(&normalized) {
        url.set_fragment(None);
        let mut serialized = url.to_string();
        if let Some(stripped) = serialized.strip_prefix("https://") {
            serialized = stripped.to_string();
        } else if let Some(stripped) = serialized.strip_prefix("http://") {
            serialized = stripped.to_string();
        }
        while serialized.ends_with('/') && serialized.len() > 1 {
            serialized.pop();
        }
        serialized
    } else {
        normalized
    }
}

fn detect_charset_label_from_html(bytes: &[u8]) -> Option<String> {
    let probe_len = bytes.len().min(16 * 1024);
    let probe = String::from_utf8_lossy(&bytes[..probe_len]).to_ascii_lowercase();
    let charset_pos = probe.find("charset=")?;
    let after = &probe[charset_pos + "charset=".len()..];
    let mut out = String::new();
    let mut started = false;
    for ch in after.chars() {
        if !started && (ch == '"' || ch == '\'' || ch.is_whitespace()) {
            continue;
        }
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            started = true;
            out.push(ch);
        } else if started {
            break;
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn decode_html_bytes(bytes: &[u8]) -> String {
    if let Ok(text) = String::from_utf8(bytes.to_vec()) {
        return text;
    }

    if let Some(label) = detect_charset_label_from_html(bytes)
        && let Some(encoding) = Encoding::for_label(label.as_bytes())
    {
        let (decoded, _, _) = encoding.decode(bytes);
        return decoded.into_owned();
    }

    let (decoded, _, _) = WINDOWS_1252.decode(bytes);
    decoded.into_owned()
}

fn decode_basic_html_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '&' {
            out.push(c);
            continue;
        }

        let mut entity = String::new();
        let mut ended_with_semicolon = false;
        while let Some(&next) = chars.peek() {
            chars.next();
            if next == ';' {
                ended_with_semicolon = true;
                break;
            }
            if entity.len() >= 16 {
                entity.push(next);
                break;
            }
            entity.push(next);
        }

        let decoded = if entity.starts_with("#x") || entity.starts_with("#X") {
            u32::from_str_radix(&entity[2..], 16)
                .ok()
                .and_then(char::from_u32)
        } else if let Some(num) = entity.strip_prefix('#') {
            num.parse::<u32>().ok().and_then(char::from_u32)
        } else {
            match entity.as_str() {
                "nbsp" => Some(' '),
                "amp" => Some('&'),
                "quot" | "quote" => Some('"'),
                "apos" => Some('\''),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "laquo" => Some('«'),
                "raquo" => Some('»'),
                "hellip" => Some('…'),
                "ndash" => Some('–'),
                "mdash" => Some('—'),
                "rsquo" => Some('’'),
                "lsquo" => Some('‘'),
                "rdquo" => Some('”'),
                "ldquo" => Some('“'),
                "agrave" => Some('à'),
                "egrave" => Some('è'),
                "igrave" => Some('ì'),
                "ograve" => Some('ò'),
                "ugrave" => Some('ù'),
                _ => None,
            }
        };

        if let Some(ch) = decoded {
            out.push(ch);
        } else {
            out.push('&');
            out.push_str(&entity);
            if ended_with_semicolon {
                out.push(';');
            }
        }
    }
    out
}
