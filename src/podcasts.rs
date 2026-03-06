use feed_rs::parser;
use serde::{Deserialize, Serialize};
use std::io::Cursor;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PodcastEpisode {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub link: String,
    #[serde(default)]
    pub audio_url: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub guid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PodcastSource {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub episodes: Vec<PodcastEpisode>,
}

#[derive(Debug, Clone)]
pub struct PodcastSearchResult {
    pub title: String,
    pub artist: String,
    pub feed_url: String,
    pub collection_id: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct PodcastCategory {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct ItunesSearchResponse {
    #[serde(default)]
    results: Vec<ItunesSearchItem>,
}

#[derive(Debug, Deserialize)]
struct ItunesSearchItem {
    #[serde(rename = "collectionId")]
    collection_id: Option<u64>,
    #[serde(rename = "collectionName")]
    collection_name: Option<String>,
    #[serde(rename = "artistName")]
    artist_name: Option<String>,
    #[serde(rename = "feedUrl")]
    feed_url: Option<String>,
    #[serde(rename = "primaryGenreId")]
    primary_genre_id: Option<u32>,
    #[serde(rename = "genreIds")]
    genre_ids: Option<Vec<String>>,
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

pub fn apple_categories_it() -> Vec<PodcastCategory> {
    vec![
        PodcastCategory {
            id: 1301,
            name: "Arti".to_string(),
        },
        PodcastCategory {
            id: 1321,
            name: "Affari".to_string(),
        },
        PodcastCategory {
            id: 1303,
            name: "Commedia".to_string(),
        },
        PodcastCategory {
            id: 1304,
            name: "Istruzione".to_string(),
        },
        PodcastCategory {
            id: 1483,
            name: "Narrativa".to_string(),
        },
        PodcastCategory {
            id: 1511,
            name: "Governo".to_string(),
        },
        PodcastCategory {
            id: 1512,
            name: "Salute e fitness".to_string(),
        },
        PodcastCategory {
            id: 1487,
            name: "Storia".to_string(),
        },
        PodcastCategory {
            id: 1305,
            name: "Bambini e famiglia".to_string(),
        },
        PodcastCategory {
            id: 1502,
            name: "Tempo libero".to_string(),
        },
        PodcastCategory {
            id: 1310,
            name: "Musica".to_string(),
        },
        PodcastCategory {
            id: 1489,
            name: "Notizie".to_string(),
        },
        PodcastCategory {
            id: 1314,
            name: "Religione e spiritualita".to_string(),
        },
        PodcastCategory {
            id: 1533,
            name: "Scienza".to_string(),
        },
        PodcastCategory {
            id: 1324,
            name: "Societa e cultura".to_string(),
        },
        PodcastCategory {
            id: 1545,
            name: "Sport".to_string(),
        },
        PodcastCategory {
            id: 1318,
            name: "Tecnologia".to_string(),
        },
        PodcastCategory {
            id: 1488,
            name: "True crime".to_string(),
        },
        PodcastCategory {
            id: 1309,
            name: "TV e film".to_string(),
        },
    ]
}

pub async fn search_itunes_podcasts(keyword: &str) -> Result<Vec<PodcastSearchResult>, String> {
    let trimmed = keyword.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let query: String = url::form_urlencoded::byte_serialize(trimmed.as_bytes()).collect();
    let url = format!(
        "https://itunes.apple.com/search?media=podcast&entity=podcast&term={query}&country=it&limit=20"
    );
    fetch_itunes_results(&url).await
}

pub async fn search_itunes_category(category_id: u32) -> Result<Vec<PodcastSearchResult>, String> {
    let top_url =
        format!("https://itunes.apple.com/it/rss/toppodcasts/limit=20/genre={category_id}/json");
    let top_ids = fetch_apple_top_ids(&top_url).await?;
    if !top_ids.is_empty()
        && let Some(lookup_url) = build_lookup_url(&top_ids)
    {
        let results = fetch_itunes_results_filtered(&lookup_url, Some(category_id)).await?;
        if !results.is_empty() {
            return Ok(order_results_by_ids(results, &top_ids));
        }
    }

    let url = format!(
        "https://itunes.apple.com/search?media=podcast&entity=podcast&genreId={category_id}&country=it&limit=20"
    );
    let results = fetch_itunes_results_filtered(&url, Some(category_id)).await?;
    if results.is_empty() {
        fetch_itunes_results_filtered(&url, None).await
    } else {
        Ok(results)
    }
}

async fn fetch_itunes_results(url: &str) -> Result<Vec<PodcastSearchResult>, String> {
    fetch_itunes_results_filtered(url, None).await
}

async fn fetch_itunes_results_filtered(
    url: &str,
    genre_id: Option<u32>,
) -> Result<Vec<PodcastSearchResult>, String> {
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

    let parsed: ItunesSearchResponse =
        serde_json::from_slice(&bytes).map_err(|err| err.to_string())?;
    Ok(itunes_items_to_results(parsed.results, genre_id))
}

pub async fn fetch_source(source: &PodcastSource) -> Result<PodcastSource, String> {
    let url = normalize_url(&source.url);
    if url.is_empty() {
        return Err("URL podcast vuoto".to_string());
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

    let (title, episodes) = parse_feed(bytes.to_vec(), &source.title)?;
    Ok(PodcastSource {
        title,
        url,
        episodes,
    })
}

fn parse_feed(
    bytes: Vec<u8>,
    fallback_title: &str,
) -> Result<(String, Vec<PodcastEpisode>), String> {
    let cursor = Cursor::new(bytes);
    let feed = parser::parse(cursor).map_err(|err| err.to_string())?;
    let title = feed
        .title
        .map(|title| title.content)
        .unwrap_or_else(|| fallback_title.to_string());

    let mut episodes = Vec::new();
    for entry in feed.entries {
        let title = entry
            .title
            .as_ref()
            .map(|value| value.content.clone())
            .unwrap_or_else(|| "Episodio senza titolo".to_string());
        let link = select_entry_link(&entry).unwrap_or_default();
        let audio_url = select_enclosure_url(&entry).unwrap_or_default();
        let guid = if !entry.id.trim().is_empty() {
            entry.id.clone()
        } else if !link.trim().is_empty() {
            link.clone()
        } else if !audio_url.trim().is_empty() {
            audio_url.clone()
        } else {
            title.clone()
        };
        let description = entry
            .summary
            .as_ref()
            .map(|value| value.content.clone())
            .or_else(|| {
                entry
                    .content
                    .as_ref()
                    .and_then(|content| content.body.clone())
            })
            .unwrap_or_default();

        episodes.push(PodcastEpisode {
            title,
            link,
            audio_url,
            description,
            guid,
        });
    }

    dedup_episodes(&mut episodes);
    Ok((title, episodes))
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

fn select_enclosure_url(entry: &feed_rs::model::Entry) -> Option<String> {
    for link in &entry.links {
        let href = link.href.trim();
        if href.is_empty() {
            continue;
        }
        let rel = link.rel.as_deref().unwrap_or("");
        let media_type = link.media_type.as_deref().unwrap_or("");
        if rel.eq_ignore_ascii_case("enclosure")
            || media_type.to_ascii_lowercase().starts_with("audio/")
        {
            return Some(href.to_string());
        }
    }
    None
}

fn dedup_episodes(episodes: &mut Vec<PodcastEpisode>) {
    let mut seen = std::collections::HashSet::new();
    episodes.retain(|episode| {
        let key = if !episode.guid.trim().is_empty() {
            format!("guid:{}", episode.guid.trim())
        } else {
            format!("link:{}", episode.link.trim())
        };
        seen.insert(key)
    });
}

async fn fetch_apple_top_ids(url: &str) -> Result<Vec<u64>, String> {
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

    Ok(parse_apple_top_ids(&bytes))
}

fn parse_apple_top_ids(bytes: &[u8]) -> Vec<u64> {
    let mut ids = Vec::new();
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return ids;
    };
    let Some(entries) = value.get("feed").and_then(|feed| feed.get("entry")) else {
        return ids;
    };
    let list = match entries {
        serde_json::Value::Array(items) => items.clone(),
        other => vec![other.clone()],
    };
    for entry in list {
        if let Some(id_value) = entry
            .get("id")
            .and_then(|id| id.get("attributes"))
            .and_then(|attrs| attrs.get("im:id"))
            .and_then(|value| value.as_str())
            && let Ok(id) = id_value.parse::<u64>()
        {
            ids.push(id);
        }
    }
    ids
}

fn build_lookup_url(ids: &[u64]) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let joined = ids
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<String>>()
        .join(",");
    Some(format!(
        "https://itunes.apple.com/lookup?id={joined}&country=it"
    ))
}

fn order_results_by_ids(
    results: Vec<PodcastSearchResult>,
    ids: &[u64],
) -> Vec<PodcastSearchResult> {
    let mut by_id = std::collections::HashMap::new();
    for result in results {
        if let Some(id) = result.collection_id {
            by_id.insert(id, result);
        }
    }

    let mut ordered = Vec::new();
    for id in ids {
        if let Some(result) = by_id.remove(id) {
            ordered.push(result);
        }
    }
    ordered
}

fn itunes_items_to_results(
    items: Vec<ItunesSearchItem>,
    genre_id: Option<u32>,
) -> Vec<PodcastSearchResult> {
    let mut results = Vec::new();
    for item in items {
        if genre_id.is_some() && !itunes_item_matches_genre(&item, genre_id.unwrap_or_default()) {
            continue;
        }
        if let (Some(title), Some(feed_url)) = (item.collection_name, item.feed_url) {
            results.push(PodcastSearchResult {
                title,
                artist: item.artist_name.unwrap_or_default(),
                feed_url,
                collection_id: item.collection_id,
            });
        }
    }
    results
}

fn itunes_item_matches_genre(item: &ItunesSearchItem, genre_id: u32) -> bool {
    if let Some(ids) = item.genre_ids.as_ref() {
        return ids.iter().any(|id| id == &genre_id.to_string());
    }
    if let Some(primary) = item.primary_genre_id {
        return primary == genre_id;
    }
    false
}
