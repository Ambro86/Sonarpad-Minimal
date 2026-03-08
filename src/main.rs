#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod articles;
mod curl_client;
mod edge_tts;
mod file_loader;
mod podcast_player;
mod podcasts;
mod reader;

use rodio::{Decoder, OutputStream, Sink};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::io::Cursor;
#[cfg(any(target_os = "macos", windows))]
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
#[cfg(any(target_os = "macos", windows))]
use uuid::Uuid;
use wxdragon::event::KeyboardEvent;
use wxdragon::prelude::*;
use wxdragon::timer::Timer;

const ID_OPEN: i32 = 101;
const ID_EXIT: i32 = 102;
const ID_ABOUT: i32 = 103;
const ID_DONATIONS: i32 = 104;
const ID_PLAY_PAUSE: i32 = 2001;
const ID_STOP: i32 = 2003;
const ID_SAVE: i32 = 2002;
const ID_SETTINGS: i32 = 2004;
const ID_PODCAST_BACKWARD: i32 = 2005;
const ID_PODCAST_FORWARD: i32 = 2006;
const ID_ARTICLES_ADD_SOURCE: i32 = 2100;
const ID_ARTICLES_DELETE_SOURCE: i32 = 2101;
const ID_ARTICLES_EDIT_SOURCE: i32 = 2102;
const ID_ARTICLES_REORDER_SOURCES: i32 = 2103;
const ID_PODCASTS_ADD: i32 = 2300;
const ID_PODCASTS_DELETE: i32 = 2301;
const ID_PODCASTS_CATEGORY_BASE: i32 = 2400;
const ID_PODCASTS_SOURCE_BASE: i32 = 2600;
const ID_PODCASTS_EPISODE_BASE: i32 = 30000;
const ID_ARTICLES_SOURCE_BASE: i32 = 2200;
const ID_ARTICLES_ARTICLE_BASE: i32 = 10000;
const MAX_MENU_ARTICLES_PER_SOURCE: usize = 30;
const MAX_MENU_PODCAST_EPISODES_PER_SOURCE: usize = 30;
const PODCAST_SEEK_SECONDS: f64 = 30.0;
const AUDIOBOOK_SAVE_THREADS: usize = 8;
const WXK_LEFT: i32 = 314;
const WXK_RIGHT: i32 = 316;
#[cfg(target_os = "macos")]
const APP_STORAGE_DIR_NAME: &str = "Sonarpad Minimal";

#[cfg(target_os = "macos")]
const MOD_CMD: &str = "Cmd";
#[cfg(not(target_os = "macos"))]
const MOD_CMD: &str = "Ctrl";

#[cfg(target_os = "macos")]
const MOD_ALT: &str = "Option";
#[cfg(not(target_os = "macos"))]
const MOD_ALT: &str = "Alt";

#[derive(PartialEq, Clone, Copy, Debug)]
enum PlaybackStatus {
    Stopped,
    Playing,
    Paused,
}

struct GlobalPlayback {
    sink: Option<Arc<Sink>>,
    status: PlaybackStatus,
    download_finished: bool,
    refresh_requested: bool,
}

struct ArticleMenuState {
    dirty: bool,
    loading_urls: HashSet<String>,
}

struct PodcastMenuState {
    dirty: bool,
    loading_urls: HashSet<String>,
}

struct PodcastPlaybackState {
    player: Option<podcast_player::PodcastPlayer>,
    selected_episode: Option<podcasts::PodcastEpisode>,
    current_audio_url: String,
    status: PlaybackStatus,
}

struct SaveAudiobookState {
    completed_chunks: usize,
    completed: bool,
    cancelled: bool,
    error_message: Option<String>,
}

enum PendingSaveDialog {
    Success,
    Error(String),
}

#[derive(Serialize, Deserialize, Clone)]
struct Settings {
    language: String,
    voice: String,
    rate: i32,
    pitch: i32,
    volume: i32,
    #[serde(default = "articles::default_italian_sources")]
    article_sources: Vec<articles::ArticleSource>,
    #[serde(default)]
    podcast_sources: Vec<podcasts::PodcastSource>,
}

impl Settings {
    fn load() -> Self {
        if let Some(data) = read_app_storage_text("settings.json")
            && let Ok(mut settings) = serde_json::from_str::<Settings>(&data)
        {
            normalize_article_sources(&mut settings);
            return settings;
        }
        let mut settings = Settings {
            language: "Italiano".to_string(),
            voice: "".to_string(),
            rate: 0,
            pitch: 0,
            volume: 100,
            article_sources: articles::default_italian_sources(),
            podcast_sources: Vec::new(),
        };
        normalize_article_sources(&mut settings);
        settings
    }

    fn save(&self) {
        if let Ok(data) = serde_json::to_string_pretty(self)
            && let Err(err) = write_app_storage_text("settings.json", &data)
        {
            println!("ERROR: Salvataggio impostazioni fallito: {}", err);
        }
    }
}

fn get_language_name(locale: &str) -> String {
    let base = locale.split('-').next().unwrap_or(locale).to_lowercase();
    match base.as_str() {
        "af" => "Afrikaans".to_string(),
        "am" => "Amarico".to_string(),
        "ar" => "Arabo".to_string(),
        "az" => "Azero".to_string(),
        "bg" => "Bulgaro".to_string(),
        "bn" => "Bengalese".to_string(),
        "bs" => "Bosniaco".to_string(),
        "ca" => "Catalano".to_string(),
        "cs" => "Ceco".to_string(),
        "cy" => "Gallese".to_string(),
        "da" => "Danese".to_string(),
        "it" => "Italiano".to_string(),
        "en" => "Inglese".to_string(),
        "fr" => "Francese".to_string(),
        "es" => "Spagnolo".to_string(),
        "de" => "Tedesco".to_string(),
        "el" => "Greco".to_string(),
        "et" => "Estone".to_string(),
        "fa" => "Persiano".to_string(),
        "fi" => "Finlandese".to_string(),
        "ga" => "Irlandese".to_string(),
        "gu" => "Gujarati".to_string(),
        "he" => "Ebraico".to_string(),
        "hi" => "Hindi".to_string(),
        "hr" => "Croato".to_string(),
        "hu" => "Ungherese".to_string(),
        "hy" => "Armeno".to_string(),
        "id" => "Indonesiano".to_string(),
        "is" => "Islandese".to_string(),
        "pt" => "Portoghese".to_string(),
        "kk" => "Kazako".to_string(),
        "km" => "Khmer".to_string(),
        "kn" => "Kannada".to_string(),
        "ko" => "Coreano".to_string(),
        "lo" => "Lao".to_string(),
        "lt" => "Lituano".to_string(),
        "lv" => "Lettone".to_string(),
        "mk" => "Macedone".to_string(),
        "ml" => "Malayalam".to_string(),
        "mn" => "Mongolo".to_string(),
        "mr" => "Marathi".to_string(),
        "ms" => "Malese".to_string(),
        "mt" => "Maltese".to_string(),
        "my" => "Birmano".to_string(),
        "nb" | "no" => "Norvegese".to_string(),
        "ne" => "Nepalese".to_string(),
        "nl" => "Olandese".to_string(),
        "pa" => "Punjabi".to_string(),
        "pl" => "Polacco".to_string(),
        "ro" => "Rumeno".to_string(),
        "ru" => "Russo".to_string(),
        "sk" => "Slovacco".to_string(),
        "sl" => "Sloveno".to_string(),
        "sq" => "Albanese".to_string(),
        "sr" => "Serbo".to_string(),
        "sv" => "Svedese".to_string(),
        "sw" => "Swahili".to_string(),
        "ta" => "Tamil".to_string(),
        "te" => "Telugu".to_string(),
        "th" => "Thailandese".to_string(),
        "tr" => "Turco".to_string(),
        "uk" => "Ucraino".to_string(),
        "ur" => "Urdu".to_string(),
        "uz" => "Uzbeco".to_string(),
        "vi" => "Vietnamita".to_string(),
        "zh" => "Cinese".to_string(),
        "ja" => "Giapponese".to_string(),
        "zu" => "Zulu".to_string(),
        _ => locale.to_string(),
    }
}

const RATE_PRESETS: [(&str, i32); 7] = [
    ("Molto lenta", -60),
    ("Lenta", -30),
    ("Meno veloce", -15),
    ("Normale", 0),
    ("Veloce", 15),
    ("Più veloce", 30),
    ("Molto veloce", 60),
];

const PITCH_PRESETS: [(&str, i32); 5] = [
    ("Molto basso", -40),
    ("Basso", -20),
    ("Normale", 0),
    ("Alto", 20),
    ("Molto alto", 40),
];

const VOLUME_PRESETS: [(&str, i32); 5] = [
    ("Molto basso", 40),
    ("Basso", 70),
    ("Normale", 100),
    ("Alto", 140),
    ("Molto alto", 180),
];

fn nearest_preset_index(presets: &[(&str, i32)], value: i32) -> usize {
    presets
        .iter()
        .enumerate()
        .min_by_key(|(_, (_, v))| (*v - value).abs())
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn play_button_label(status: PlaybackStatus, podcast_mode: bool) -> String {
    let shortcut = format!("{}+L", MOD_CMD);

    if podcast_mode {
        match status {
            PlaybackStatus::Stopped => format!("Riproduci Podcast ({shortcut})"),
            PlaybackStatus::Playing => format!("Pausa Podcast ({shortcut})"),
            PlaybackStatus::Paused => format!("Riprendi Podcast ({shortcut})"),
        }
    } else {
        match status {
            PlaybackStatus::Stopped => format!("Avvia Lettura ({shortcut})"),
            PlaybackStatus::Playing => format!("Pausa Lettura ({shortcut})"),
            PlaybackStatus::Paused => format!("Riprendi Lettura ({shortcut})"),
        }
    }
}

fn save_button_label() -> String {
    format!("Salva Audiolibro ({}+{}+A)", MOD_CMD, MOD_ALT)
}

fn stop_button_label(podcast_mode: bool) -> String {
    if podcast_mode {
        format!("Ferma Podcast ({}+.)", MOD_CMD)
    } else {
        format!("Ferma Lettura ({}+.)", MOD_CMD)
    }
}

fn settings_button_label() -> String {
    format!("Impostazioni ({}+,)", MOD_CMD)
}

#[cfg(target_os = "macos")]
fn command_shortcut_down(key_event: &KeyboardEvent) -> bool {
    key_event.cmd_down() || key_event.meta_down()
}

#[cfg(not(target_os = "macos"))]
fn command_shortcut_down(key_event: &KeyboardEvent) -> bool {
    key_event.cmd_down()
}

fn handle_shortcut_event(
    event: WindowEventData,
    play_action: &Rc<dyn Fn()>,
    stop_action: &Rc<dyn Fn()>,
    save_action: &Rc<dyn Fn()>,
    settings_action: &Rc<dyn Fn()>,
    podcast_seek_back: &Rc<RefCell<PodcastPlaybackState>>,
    podcast_seek_forward: &Rc<RefCell<PodcastPlaybackState>>,
) {
    if let WindowEventData::Keyboard(key_event) = event {
        #[cfg(target_os = "macos")]
        {
            let _ = (play_action, stop_action, save_action, settings_action);
            if command_shortcut_down(&key_event) && !key_event.alt_down() && !key_event.shift_down()
            {
                match key_event.get_key_code().unwrap_or_default() {
                    WXK_LEFT => {
                        if podcast_seek_back.borrow().selected_episode.is_some() {
                            seek_podcast_playback(podcast_seek_back, -PODCAST_SEEK_SECONDS);
                        }
                    }
                    WXK_RIGHT => {
                        if podcast_seek_forward.borrow().selected_episode.is_some() {
                            seek_podcast_playback(podcast_seek_forward, PODCAST_SEEK_SECONDS);
                        }
                    }
                    _ => {}
                }
            }
            return;
        }

        #[cfg(not(target_os = "macos"))]
        let key_code = key_event.get_key_code().unwrap_or_default();
        #[cfg(not(target_os = "macos"))]
        let unicode_key = key_event.get_unicode_key().unwrap_or_default();
        #[cfg(not(target_os = "macos"))]
        if command_shortcut_down(&key_event) && !key_event.alt_down() && !key_event.shift_down() {
            match key_code {
                76 | 108 => play_action(),
                WXK_LEFT => {
                    if podcast_seek_back.borrow().selected_episode.is_some() {
                        seek_podcast_playback(podcast_seek_back, -PODCAST_SEEK_SECONDS);
                    }
                }
                WXK_RIGHT => {
                    if podcast_seek_forward.borrow().selected_episode.is_some() {
                        seek_podcast_playback(podcast_seek_forward, PODCAST_SEEK_SECONDS);
                    }
                }
                _ if unicode_key == 46 => stop_action(),
                _ if unicode_key == 44 => settings_action(),
                _ => {}
            }
        } else if command_shortcut_down(&key_event)
            && key_event.alt_down()
            && !key_event.shift_down()
        {
            match key_code {
                65 | 97 => save_action(),
                _ => {}
            }
        }
    }
}

fn about_title() -> &'static str {
    "Informazioni sul programma"
}

fn about_message() -> String {
    format!(
        "Sonarpad Minimal è una versione essenziale di Sonarpad con supporto multi-formato. \
Apre file TXT, PDF, DOC, DOCX, EPUB, RTF, HTML e fogli di calcolo; legge il testo con sintesi vocale, \
crea audiolibri MP3, importa articoli e podcast e supporta la riproduzione dei podcast. \
Versione: {}. Autore: Ambrogio Riili.",
        env!("CARGO_PKG_VERSION")
    )
}

fn donations_title() -> &'static str {
    "Donazioni"
}

fn donations_message() -> &'static str {
    include_str!("../donations_it.txt")
}

fn open_donations_dialog(parent: &Frame) {
    let dialog = Dialog::builder(parent, donations_title())
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(640, 520)
        .build();
    let panel = Panel::builder(&dialog).build();
    let root = BoxSizer::builder(Orientation::Vertical).build();

    let text = TextCtrl::builder(&panel)
        .with_style(TextCtrlStyle::MultiLine | TextCtrlStyle::ReadOnly)
        .build();
    text.set_value(donations_message());
    root.add(&text, 1, SizerFlag::Expand | SizerFlag::All, 8);

    let button_row = BoxSizer::builder(Orientation::Horizontal).build();
    let btn_ok = Button::builder(&panel)
        .with_id(ID_OK)
        .with_label("OK")
        .build();
    button_row.add_spacer(1);
    button_row.add(&btn_ok, 0, SizerFlag::All, 10);
    root.add_sizer(&button_row, 0, SizerFlag::Expand, 0);

    panel.set_sizer(root, true);
    dialog.set_affirmative_id(ID_OK);
    let dialog_ok = dialog;
    btn_ok.on_click(move |_| {
        dialog_ok.end_modal(ID_OK);
    });
    dialog.show_modal();
    dialog.destroy();
}

fn show_modeless_message_dialog(parent: &Frame, title: &str, message: &str) {
    let dialog = Dialog::builder(parent, title)
        .with_style(DialogStyle::Caption | DialogStyle::SystemMenu | DialogStyle::CloseBox)
        .with_size(420, 180)
        .build();
    let panel = Panel::builder(&dialog).build();
    let root = BoxSizer::builder(Orientation::Vertical).build();

    let text = StaticText::builder(&panel).with_label(message).build();
    root.add(&text, 1, SizerFlag::Expand | SizerFlag::All, 12);

    let button_row = BoxSizer::builder(Orientation::Horizontal).build();
    let btn_ok = Button::builder(&panel)
        .with_id(ID_OK)
        .with_label("OK")
        .build();
    button_row.add_spacer(1);
    button_row.add(&btn_ok, 0, SizerFlag::All, 10);
    root.add_sizer(&button_row, 0, SizerFlag::Expand, 0);

    panel.set_sizer(root, true);
    dialog.set_escape_id(ID_OK);
    let dialog_ok = dialog;
    btn_ok.on_click(move |_| {
        dialog_ok.destroy();
    });
    dialog.show(true);
}

fn percent_encode(input: &str) -> String {
    url::form_urlencoded::byte_serialize(input.as_bytes()).collect()
}

fn build_google_news_rss_url(keyword: &str) -> String {
    let query = percent_encode(keyword.trim());
    format!("https://news.google.com/rss/search?q={query}&hl=it&gl=IT&ceid=IT:it")
}

fn format_google_news_source_title(keyword: &str) -> String {
    keyword
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                let mut out = String::new();
                out.extend(first.to_uppercase());
                for ch in chars {
                    out.extend(ch.to_lowercase());
                }
                out
            } else {
                String::new()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn looks_like_article_source_url(input: &str) -> bool {
    let trimmed = input.trim();
    trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("www.")
        || trimmed.starts_with("//")
        || trimmed.contains('/')
        || trimmed.contains('.')
}

fn articles_source_menu_id(source_index: usize) -> i32 {
    ID_ARTICLES_SOURCE_BASE + source_index as i32
}

fn articles_article_menu_id(source_index: usize, item_index: usize) -> i32 {
    ID_ARTICLES_ARTICLE_BASE
        + (source_index as i32 * MAX_MENU_ARTICLES_PER_SOURCE as i32)
        + item_index as i32
}

fn decode_article_menu_id(menu_id: i32) -> Option<(usize, usize)> {
    if menu_id < ID_ARTICLES_ARTICLE_BASE {
        return None;
    }
    let offset = (menu_id - ID_ARTICLES_ARTICLE_BASE) as usize;
    let source_index = offset / MAX_MENU_ARTICLES_PER_SOURCE;
    let item_index = offset % MAX_MENU_ARTICLES_PER_SOURCE;
    Some((source_index, item_index))
}

fn podcasts_source_menu_id(source_index: usize) -> i32 {
    ID_PODCASTS_SOURCE_BASE + source_index as i32
}

fn podcasts_episode_menu_id(source_index: usize, episode_index: usize) -> i32 {
    ID_PODCASTS_EPISODE_BASE
        + (source_index as i32 * MAX_MENU_PODCAST_EPISODES_PER_SOURCE as i32)
        + episode_index as i32
}

fn decode_podcast_episode_menu_id(menu_id: i32) -> Option<(usize, usize)> {
    if menu_id < ID_PODCASTS_EPISODE_BASE {
        return None;
    }
    let offset = (menu_id - ID_PODCASTS_EPISODE_BASE) as usize;
    let source_index = offset / MAX_MENU_PODCAST_EPISODES_PER_SOURCE;
    let episode_index = offset % MAX_MENU_PODCAST_EPISODES_PER_SOURCE;
    Some((source_index, episode_index))
}

fn app_storage_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(APP_STORAGE_DIR_NAME)
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn app_storage_path(file_name: &str) -> PathBuf {
    app_storage_dir()
        .map(|dir| dir.join(file_name))
        .unwrap_or_else(|| PathBuf::from(file_name))
}

fn read_app_storage_text(file_name: &str) -> Option<String> {
    let storage_path = app_storage_path(file_name);
    if let Ok(data) = std::fs::read_to_string(&storage_path) {
        return Some(data);
    }

    let legacy_path = PathBuf::from(file_name);
    if legacy_path != storage_path {
        return std::fs::read_to_string(legacy_path).ok();
    }

    None
}

fn write_app_storage_text(file_name: &str, data: &str) -> Result<(), String> {
    let storage_path = app_storage_path(file_name);
    if let Some(parent) = storage_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("creazione cartella {} fallita: {}", parent.display(), err))?;
    }

    std::fs::write(&storage_path, data)
        .map_err(|err| format!("scrittura file {} fallita: {}", storage_path.display(), err))
}

#[cfg(any(target_os = "macos", windows))]
fn podcast_log_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Documents")
                .join("Sonarpad")
                .join("log.txt")
        });
    }

    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(|home| {
            PathBuf::from(home)
                .join("Documents")
                .join("Sonarpad")
                .join("log.txt")
        })
    }
}

#[cfg(any(target_os = "macos", windows))]
fn append_podcast_log(message: &str) {
    let Some(path) = podcast_log_path() else {
        println!("ERROR: Cartella documenti non disponibile per il log podcast");
        return;
    };

    if let Some(parent) = path.parent()
        && let Err(err) = std::fs::create_dir_all(parent)
    {
        println!(
            "ERROR: Creazione cartella log podcast {} fallita: {}",
            parent.display(),
            err
        );
        return;
    }

    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let line = format!("[{timestamp}] {message}\n");

    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut file) => {
            use std::io::Write;

            if let Err(err) = file.write_all(line.as_bytes()) {
                println!(
                    "ERROR: Scrittura log podcast {} fallita: {}",
                    path.display(),
                    err
                );
            }
        }
        Err(err) => {
            println!(
                "ERROR: Apertura log podcast {} fallita: {}",
                path.display(),
                err
            );
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn append_podcast_log(_message: &str) {}

fn log_podcast_player_snapshot(
    player: &podcast_player::PodcastPlayer,
    context: &str,
    audio_url: &str,
) {
    match player.debug_snapshot() {
        Ok(snapshot) => append_podcast_log(&format!("{context} audio_url={audio_url} {snapshot}")),
        Err(err) => append_podcast_log(&format!(
            "{context} audio_url={audio_url} snapshot_error={err}"
        )),
    }
}

fn wait_for_podcast_ready(
    parent: &Frame,
    player: &podcast_player::PodcastPlayer,
    audio_url: &str,
) -> bool {
    let progress = ProgressDialog::builder(
        parent,
        "Caricamento Podcast",
        "Preparazione stream podcast...",
        100,
    )
    .with_style(ProgressDialogStyle::CanAbort | ProgressDialogStyle::Smooth)
    .build();

    for step in 0..=40 {
        let percent = (step * 100) / 40;
        let message = format!("Scaricamento podcast... {}%", percent);
        if !progress.update(percent, Some(&message)) {
            append_podcast_log(&format!("podcast_ready.cancelled audio_url={audio_url}"));
            return false;
        }

        match player.is_ready_for_playback() {
            Ok(true) => {
                log_podcast_player_snapshot(player, "podcast_ready.success", audio_url);
                progress.update(100, Some("Podcast pronto."));
                return true;
            }
            Ok(false) => {
                log_podcast_player_snapshot(player, "podcast_ready.waiting", audio_url);
            }
            Err(err) => {
                append_podcast_log(&format!(
                    "podcast_ready.snapshot_error audio_url={} error={}",
                    audio_url, err
                ));
                return false;
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    log_podcast_player_snapshot(player, "podcast_ready.timeout", audio_url);
    false
}

#[cfg(any(target_os = "macos", windows))]
fn podcast_external_open_dir() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("Sonarpad");
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("creazione cartella download podcast fallita: {}", err))?;
    Ok(dir)
}

#[cfg(any(target_os = "macos", windows))]
fn podcast_extension_from_url(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url).ok()?;
    let last_segment = parsed
        .path_segments()
        .and_then(|mut segments| segments.rfind(|segment| !segment.is_empty()))?;
    let extension = Path::new(last_segment).extension()?.to_str()?.trim();
    if extension.is_empty() {
        None
    } else {
        Some(extension.to_ascii_lowercase())
    }
}

#[cfg(any(target_os = "macos", windows))]
fn podcast_extension_from_content_type(content_type: Option<&str>) -> &'static str {
    match content_type
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => "m4a",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/aac" | "audio/aacp" => "aac",
        "audio/wav" | "audio/x-wav" | "audio/wave" => "wav",
        "audio/ogg" | "application/ogg" => "ogg",
        "audio/flac" | "audio/x-flac" => "flac",
        _ => "mp3",
    }
}

#[cfg(any(target_os = "macos", windows))]
#[derive(Clone, Default)]
struct PodcastExternalDownloadState {
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    abort_requested: bool,
    result: Option<Result<PathBuf, String>>,
}

#[cfg(any(target_os = "macos", windows))]
fn open_podcast_download_response(
    client: &reqwest::blocking::Client,
    url: &str,
    downloaded_bytes: u64,
) -> Result<reqwest::blocking::Response, String> {
    let mut request = client.get(url).header(
        reqwest::header::USER_AGENT,
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 13_0) AppleWebKit/605.1.15 (KHTML, like Gecko)",
    );
    if downloaded_bytes > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={downloaded_bytes}-"));
    }

    let response = request
        .send()
        .map_err(|err| format!("download podcast fallito: {}", err))?;
    let status = response.status();
    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(format!(
            "download podcast fallito: HTTP {}",
            status.as_u16()
        ));
    }
    if downloaded_bytes > 0 && status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err("il server non supporta la ripresa del download podcast".to_string());
    }
    Ok(response)
}

#[cfg(any(target_os = "macos", windows))]
fn download_podcast_episode_for_external_open(
    url: &str,
    state: &Arc<Mutex<PodcastExternalDownloadState>>,
) {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        let mut locked = state.lock().unwrap();
        locked.result = Some(Err("URL episodio podcast vuoto".to_string()));
        return;
    }

    let outcome = (|| -> Result<PathBuf, String> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(900))
            .build()
            .map_err(|err| format!("inizializzazione download podcast fallita: {}", err))?;

        let mut response = open_podcast_download_response(&client, trimmed, 0)?;
        let total_bytes = response.content_length();
        state.lock().unwrap().total_bytes = total_bytes;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok());
        let extension = podcast_extension_from_url(response.url().as_str())
            .or_else(|| podcast_extension_from_url(trimmed))
            .unwrap_or_else(|| podcast_extension_from_content_type(content_type).to_string());
        let file_path = podcast_external_open_dir()?.join(format!(
            "podcast-{}.{}",
            Uuid::new_v4().simple(),
            extension
        ));

        let mut file = std::fs::File::create(&file_path)
            .map_err(|err| format!("creazione file podcast fallita: {}", err))?;
        let mut downloaded_bytes = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        let mut resume_attempts = 0_u8;

        loop {
            if state.lock().unwrap().abort_requested {
                if let Err(err) = std::fs::remove_file(&file_path) {
                    append_podcast_log(&format!(
                        "external_download.cleanup_error path={} error={}",
                        file_path.display(),
                        err
                    ));
                }
                return Err("scaricamento podcast annullato".to_string());
            }

            let read = match response.read(&mut buffer) {
                Ok(read) => {
                    resume_attempts = 0;
                    read
                }
                Err(err) if downloaded_bytes > 0 && resume_attempts < 15 => {
                    resume_attempts += 1;
                    append_podcast_log(&format!(
                        "external_download.resume_attempt url={} bytes={} attempt={} error={}",
                        trimmed, downloaded_bytes, resume_attempts, err
                    ));
                    response = open_podcast_download_response(&client, trimmed, downloaded_bytes)?;
                    if let Some(remaining_bytes) = response.content_length() {
                        state.lock().unwrap().total_bytes =
                            Some(downloaded_bytes + remaining_bytes);
                    }
                    continue;
                }
                Err(err) => return Err(format!("lettura download podcast fallita: {}", err)),
            };
            if read == 0 {
                break;
            }

            file.write_all(&buffer[..read])
                .map_err(|err| format!("scrittura file podcast fallita: {}", err))?;
            downloaded_bytes += read as u64;

            state.lock().unwrap().downloaded_bytes = downloaded_bytes;
        }

        file.flush()
            .map_err(|err| format!("finalizzazione file podcast fallita: {}", err))?;
        append_podcast_log(&format!(
            "external_download.success url={} path={} bytes={}",
            trimmed,
            file_path.display(),
            downloaded_bytes
        ));
        Ok(file_path)
    })();

    state.lock().unwrap().result = Some(outcome);
}

#[cfg(any(target_os = "macos", windows))]
fn open_podcast_episode_externally(parent: &Frame, url: &str) -> Result<(), String> {
    append_podcast_log(&format!("external_open.begin url={}", url.trim()));
    let progress = ProgressDialog::builder(
        parent,
        "Scaricamento Podcast",
        "Avvio download episodio...",
        100,
    )
    .with_style(ProgressDialogStyle::CanAbort | ProgressDialogStyle::Smooth)
    .build();

    let state = Arc::new(Mutex::new(PodcastExternalDownloadState::default()));
    let state_thread = Arc::clone(&state);
    let url_owned = url.trim().to_string();
    append_podcast_log(&format!("external_open.spawn_download url={url_owned}"));
    std::thread::spawn(move || {
        download_podcast_episode_for_external_open(&url_owned, &state_thread);
    });

    let mut fallback_percent = 0_i32;
    let mut last_logged_percent = -1_i32;
    let file_path = loop {
        std::thread::sleep(std::time::Duration::from_millis(100));

        let snapshot = state.lock().unwrap().clone();
        if let Some(result) = snapshot.result {
            let file_path = result?;
            append_podcast_log(&format!(
                "external_open.download_completed path={}",
                file_path.display()
            ));
            progress.update(100, Some("Podcast scaricato."));
            break file_path;
        }

        let (percent, message) =
            if let Some(total_bytes) = snapshot.total_bytes.filter(|size| *size > 0) {
                let percent =
                    ((snapshot.downloaded_bytes.saturating_mul(100)) / total_bytes).min(99) as i32;
                let downloaded_mb = snapshot.downloaded_bytes as f64 / (1024.0 * 1024.0);
                let total_mb = total_bytes as f64 / (1024.0 * 1024.0);
                (
                    percent,
                    format!(
                        "Scaricamento podcast... {:.1}/{:.1} MB",
                        downloaded_mb, total_mb
                    ),
                )
            } else {
                fallback_percent = (fallback_percent + 2).min(99);
                let downloaded_mb = snapshot.downloaded_bytes as f64 / (1024.0 * 1024.0);
                (
                    fallback_percent,
                    format!("Scaricamento podcast... {:.1} MB", downloaded_mb),
                )
            };

        if percent / 10 > last_logged_percent / 10 {
            last_logged_percent = percent;
            append_podcast_log(&format!(
                "external_open.progress percent={} downloaded_bytes={} total_bytes={:?}",
                percent, snapshot.downloaded_bytes, snapshot.total_bytes
            ));
        }

        if !progress.update(percent, Some(&message)) {
            append_podcast_log("external_open.cancelled_by_user");
            state.lock().unwrap().abort_requested = true;
            return Err("scaricamento podcast annullato".to_string());
        }
    };

    open_podcast_file_with_default_app(&file_path)
}

#[cfg(any(target_os = "macos", windows))]
fn open_podcast_file_with_default_app(file_path: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("/usr/bin/open")
        .arg(file_path)
        .status()
        .map_err(|err| format!("avvio app predefinita fallito: {}", err))?;

    #[cfg(windows)]
    let file_path_string = file_path.display().to_string();

    #[cfg(windows)]
    let status = std::process::Command::new("cmd")
        .args(["/C", "start", "", &file_path_string])
        .status()
        .map_err(|err| format!("avvio app predefinita fallito: {}", err))?;

    if status.success() {
        append_podcast_log(&format!(
            "external_open.success path={}",
            file_path.display()
        ));
        Ok(())
    } else {
        Err(format!(
            "apertura file podcast fallita con codice {:?}",
            status.code()
        ))
    }
}

fn load_cached_voices() -> Option<Vec<edge_tts::VoiceInfo>> {
    let data = read_app_storage_text("voices_cache.json")?;
    serde_json::from_str(&data).ok()
}

fn save_cached_voices(voices: &[edge_tts::VoiceInfo]) {
    if let Ok(data) = serde_json::to_string_pretty(voices)
        && let Err(err) = write_app_storage_text("voices_cache.json", &data)
    {
        println!("ERROR: Salvataggio cache voci fallito: {}", err);
    }
}

fn build_language_list(voices: &[edge_tts::VoiceInfo]) -> Vec<(String, String)> {
    let mut l_map = BTreeMap::new();
    for voice in voices {
        l_map.insert(get_language_name(&voice.locale), voice.locale.clone());
    }
    l_map.into_iter().collect()
}

fn normalize_article_sources(settings: &mut Settings) {
    if settings.article_sources.is_empty() {
        settings.article_sources = articles::default_italian_sources();
    }
    for source in &mut settings.article_sources {
        source.url = articles::normalize_url(&source.url);
        if source.title.trim().is_empty() {
            source.title = source.url.clone();
        }
    }
    settings
        .article_sources
        .retain(|source| !is_removed_default_article_source(&source.url));
    for source in &mut settings.podcast_sources {
        source.url = podcasts::normalize_url(&source.url);
        if source.title.trim().is_empty() {
            source.title = source.url.clone();
        }
    }
}

fn is_removed_default_article_source(url: &str) -> bool {
    matches!(
        articles::normalize_url(url).as_str(),
        "https://www.ilpost.it/feed/"
            | "https://www.fanpage.it/feed/"
            | "https://www.internazionale.it/rss"
            | "https://www.affaritaliani.it/static/rss/rssGadget.aspx?idchannel=1"
            | "https://www.hwupgrade.it/rss/news.xml"
            | "https://www.startmag.it/feed/"
    )
}

fn rebuild_articles_menu(
    articles_menu: &Menu,
    settings: &Arc<Mutex<Settings>>,
    loading_urls: &HashSet<String>,
) {
    for item in articles_menu.get_menu_items().into_iter().rev() {
        let _ = articles_menu.delete_item(&item);
    }

    let _ = articles_menu.append(
        ID_ARTICLES_ADD_SOURCE,
        "Aggiungi fonte...",
        "Aggiungi un feed RSS o una fonte",
        ItemKind::Normal,
    );
    let _ = articles_menu.append(
        ID_ARTICLES_EDIT_SOURCE,
        "Modifica fonte...",
        "Modifica una fonte RSS salvata",
        ItemKind::Normal,
    );
    let _ = articles_menu.append(
        ID_ARTICLES_DELETE_SOURCE,
        "Elimina fonte...",
        "Elimina una fonte RSS salvata",
        ItemKind::Normal,
    );
    let _ = articles_menu.append(
        ID_ARTICLES_REORDER_SOURCES,
        "Riordina fonti...",
        "Riordina le fonti RSS salvate",
        ItemKind::Normal,
    );
    articles_menu.append_separator();

    let sources = settings.lock().unwrap().article_sources.clone();
    for (source_index, source) in sources.iter().enumerate() {
        let submenu = Menu::builder().build();
        if source.items.is_empty() {
            let placeholder_id = articles_source_menu_id(source_index);
            let placeholder_label = if loading_urls.contains(&source.url) {
                "Caricamento articoli..."
            } else {
                "Nessun articolo disponibile"
            };
            let placeholder_help = if loading_urls.contains(&source.url) {
                "Attendere il caricamento degli articoli"
            } else {
                "Aggiorna la fonte per ottenere gli articoli"
            };
            let _ = submenu.append(
                placeholder_id,
                placeholder_label,
                placeholder_help,
                ItemKind::Normal,
            );
            let _ = submenu.enable_item(placeholder_id, false);
        } else {
            for (item_index, item) in source
                .items
                .iter()
                .take(MAX_MENU_ARTICLES_PER_SOURCE)
                .enumerate()
            {
                let _ = submenu.append(
                    articles_article_menu_id(source_index, item_index),
                    &item.title,
                    &item.link,
                    ItemKind::Normal,
                );
            }
        }
        let _ = articles_menu.append_submenu(submenu, &source.title, &source.url);
    }
}

fn rebuild_podcasts_menu(
    podcasts_menu: &Menu,
    settings: &Arc<Mutex<Settings>>,
    loading_urls: &HashSet<String>,
) {
    for item in podcasts_menu.get_menu_items().into_iter().rev() {
        let _ = podcasts_menu.delete_item(&item);
    }

    let _ = podcasts_menu.append(
        ID_PODCASTS_ADD,
        "Aggiungi podcast...",
        "Aggiungi un podcast cercando per parola chiave",
        ItemKind::Normal,
    );
    let categories_menu = Menu::builder().build();
    for (index, category) in podcasts::apple_categories_it().iter().enumerate() {
        let _ = categories_menu.append(
            ID_PODCASTS_CATEGORY_BASE + index as i32,
            &category.name,
            "Sfoglia i podcast della categoria",
            ItemKind::Normal,
        );
    }
    let _ = podcasts_menu.append_submenu(
        categories_menu,
        "Sfoglia per categorie",
        "Sfoglia podcast per categoria",
    );
    let _ = podcasts_menu.append(
        ID_PODCASTS_DELETE,
        "Elimina podcast...",
        "Elimina un podcast salvato",
        ItemKind::Normal,
    );
    podcasts_menu.append_separator();

    let sources = settings.lock().unwrap().podcast_sources.clone();
    for (source_index, source) in sources.iter().enumerate() {
        let submenu = Menu::builder().build();
        if source.episodes.is_empty() {
            let placeholder_id = podcasts_source_menu_id(source_index);
            let is_loading = loading_urls.contains(&source.url);
            let _ = submenu.append(
                placeholder_id,
                if is_loading {
                    "Caricamento episodi..."
                } else {
                    "Nessun episodio disponibile"
                },
                if is_loading {
                    "Attendere il caricamento degli episodi"
                } else {
                    "Aggiorna il podcast per ottenere episodi"
                },
                ItemKind::Normal,
            );
            let _ = submenu.enable_item(placeholder_id, false);
        } else {
            for (episode_index, episode) in source
                .episodes
                .iter()
                .take(MAX_MENU_PODCAST_EPISODES_PER_SOURCE)
                .enumerate()
            {
                let _ = submenu.append(
                    podcasts_episode_menu_id(source_index, episode_index),
                    &episode.title,
                    &episode.link,
                    ItemKind::Normal,
                );
            }
        }
        let _ = podcasts_menu.append_submenu(submenu, &source.title, &source.url);
    }
}

fn refresh_all_article_sources(
    rt: &Arc<Runtime>,
    settings: &Arc<Mutex<Settings>>,
    article_menu_state: &Arc<Mutex<ArticleMenuState>>,
) {
    let rt_refresh = Arc::clone(rt);
    let settings_refresh = Arc::clone(settings);
    let menu_state_refresh = Arc::clone(article_menu_state);
    std::thread::spawn(move || {
        let sources = settings_refresh.lock().unwrap().article_sources.clone();
        let mut updated_sources = Vec::with_capacity(sources.len());
        let mut changed = false;
        for source in sources {
            match rt_refresh.block_on(articles::fetch_source(&source)) {
                Ok(updated) => {
                    if updated.items != source.items || updated.title != source.title {
                        changed = true;
                    }
                    updated_sources.push(updated);
                }
                Err(err) => {
                    println!(
                        "ERROR: Aggiornamento articoli fallito per {}: {}",
                        source.title, err
                    );
                    updated_sources.push(source);
                }
            }
        }

        if changed {
            let mut locked = settings_refresh.lock().unwrap();
            locked.article_sources = updated_sources;
            locked.save();
            menu_state_refresh.lock().unwrap().dirty = true;
        }
    });
}

fn refresh_single_article_source(
    source_url: String,
    rt: &Arc<Runtime>,
    settings: &Arc<Mutex<Settings>>,
    article_menu_state: &Arc<Mutex<ArticleMenuState>>,
) {
    {
        let mut state = article_menu_state.lock().unwrap();
        state.loading_urls.insert(source_url.clone());
        state.dirty = true;
    }

    let rt_refresh = Arc::clone(rt);
    let settings_refresh = Arc::clone(settings);
    let menu_state_refresh = Arc::clone(article_menu_state);
    std::thread::spawn(move || {
        let source = {
            settings_refresh
                .lock()
                .unwrap()
                .article_sources
                .iter()
                .find(|source| source.url.eq_ignore_ascii_case(&source_url))
                .cloned()
        };

        if let Some(source) = source {
            match rt_refresh.block_on(articles::fetch_source(&source)) {
                Ok(updated) => {
                    let mut locked = settings_refresh.lock().unwrap();
                    if let Some(existing) = locked
                        .article_sources
                        .iter_mut()
                        .find(|existing| existing.url.eq_ignore_ascii_case(&source_url))
                    {
                        *existing = updated;
                        locked.save();
                    }
                }
                Err(err) => {
                    println!(
                        "ERROR: Aggiornamento articoli fallito per {}: {}",
                        source.title, err
                    );
                }
            }
        }

        let mut state = menu_state_refresh.lock().unwrap();
        state.loading_urls.remove(&source_url);
        state.dirty = true;
    });
}

fn refresh_single_podcast_source(
    source_url: String,
    rt: &Arc<Runtime>,
    settings: &Arc<Mutex<Settings>>,
    podcast_menu_state: &Arc<Mutex<PodcastMenuState>>,
) {
    {
        let mut state = podcast_menu_state.lock().unwrap();
        state.loading_urls.insert(source_url.clone());
        state.dirty = true;
    }

    let rt_refresh = Arc::clone(rt);
    let settings_refresh = Arc::clone(settings);
    let menu_state_refresh = Arc::clone(podcast_menu_state);
    std::thread::spawn(move || {
        let source = {
            settings_refresh
                .lock()
                .unwrap()
                .podcast_sources
                .iter()
                .find(|source| source.url.eq_ignore_ascii_case(&source_url))
                .cloned()
        };

        if let Some(source) = source {
            match rt_refresh.block_on(podcasts::fetch_source(&source)) {
                Ok(updated) => {
                    let mut locked = settings_refresh.lock().unwrap();
                    if let Some(existing) = locked
                        .podcast_sources
                        .iter_mut()
                        .find(|existing| existing.url.eq_ignore_ascii_case(&source_url))
                    {
                        *existing = updated;
                        locked.save();
                    }
                }
                Err(err) => {
                    println!(
                        "ERROR: Aggiornamento podcast fallito per {}: {}",
                        source.title, err
                    );
                }
            }
        }

        let mut state = menu_state_refresh.lock().unwrap();
        state.loading_urls.remove(&source_url);
        state.dirty = true;
    });
}

fn refresh_all_podcast_sources(
    rt: &Arc<Runtime>,
    settings: &Arc<Mutex<Settings>>,
    podcast_menu_state: &Arc<Mutex<PodcastMenuState>>,
) {
    let source_urls = {
        settings
            .lock()
            .unwrap()
            .podcast_sources
            .iter()
            .map(|source| source.url.clone())
            .collect::<Vec<String>>()
    };

    for source_url in source_urls {
        refresh_single_podcast_source(source_url, rt, settings, podcast_menu_state);
    }
}

fn add_article_source(
    title: String,
    url: String,
    settings: &Arc<Mutex<Settings>>,
    article_menu_state: &Arc<Mutex<ArticleMenuState>>,
    rt: &Arc<Runtime>,
) {
    let Some((normalized_url, resolved_title)) = resolve_article_source_input(&title, &url) else {
        return;
    };

    {
        let mut locked = settings.lock().unwrap();
        if locked
            .article_sources
            .iter()
            .any(|source| source.url.eq_ignore_ascii_case(&normalized_url))
        {
            return;
        }
        locked.article_sources.push(articles::ArticleSource {
            title: resolved_title,
            url: normalized_url.clone(),
            items: Vec::new(),
        });
        locked.save();
    }
    refresh_single_article_source(normalized_url, rt, settings, article_menu_state);
}

fn resolve_article_source_input(title: &str, url: &str) -> Option<(String, String)> {
    let trimmed_input = url.trim();
    if trimmed_input.is_empty() {
        return None;
    }

    let (normalized_url, resolved_title) = if looks_like_article_source_url(trimmed_input) {
        let normalized_url = articles::normalize_url(trimmed_input);
        let resolved_title = if title.trim().is_empty() {
            normalized_url.clone()
        } else {
            title.trim().to_string()
        };
        (normalized_url, resolved_title)
    } else {
        let resolved_title = if title.trim().is_empty() {
            format_google_news_source_title(trimmed_input)
        } else {
            title.trim().to_string()
        };
        (build_google_news_rss_url(trimmed_input), resolved_title)
    };

    if normalized_url.is_empty() {
        None
    } else {
        Some((normalized_url, resolved_title))
    }
}

fn edit_article_source(
    source_index: usize,
    title: String,
    url: String,
    settings: &Arc<Mutex<Settings>>,
    article_menu_state: &Arc<Mutex<ArticleMenuState>>,
    rt: &Arc<Runtime>,
) {
    let Some((normalized_url, resolved_title)) = resolve_article_source_input(&title, &url) else {
        return;
    };

    {
        let mut locked = settings.lock().unwrap();
        if source_index >= locked.article_sources.len() {
            return;
        }
        if locked
            .article_sources
            .iter()
            .enumerate()
            .any(|(index, source)| {
                index != source_index && source.url.eq_ignore_ascii_case(&normalized_url)
            })
        {
            return;
        }
        let source = &mut locked.article_sources[source_index];
        source.title = resolved_title;
        source.url = normalized_url.clone();
        source.items.clear();
        locked.save();
    }

    refresh_single_article_source(normalized_url, rt, settings, article_menu_state);
}

fn delete_article_source(
    source_index: usize,
    settings: &Arc<Mutex<Settings>>,
    article_menu_state: &Arc<Mutex<ArticleMenuState>>,
) {
    let mut locked = settings.lock().unwrap();
    if source_index >= locked.article_sources.len() {
        return;
    }
    locked.article_sources.remove(source_index);
    locked.save();
    article_menu_state.lock().unwrap().dirty = true;
}

fn save_reordered_article_sources(
    reordered_sources: Vec<articles::ArticleSource>,
    settings: &Arc<Mutex<Settings>>,
    article_menu_state: &Arc<Mutex<ArticleMenuState>>,
) {
    let mut locked = settings.lock().unwrap();
    locked.article_sources = reordered_sources;
    locked.save();
    article_menu_state.lock().unwrap().dirty = true;
}

fn add_podcast_source(
    result: podcasts::PodcastSearchResult,
    settings: &Arc<Mutex<Settings>>,
    podcast_menu_state: &Arc<Mutex<PodcastMenuState>>,
    rt: &Arc<Runtime>,
) {
    let normalized_url = podcasts::normalize_url(&result.feed_url);
    if normalized_url.is_empty() {
        return;
    }

    {
        let mut locked = settings.lock().unwrap();
        if locked
            .podcast_sources
            .iter()
            .any(|source| source.url.eq_ignore_ascii_case(&normalized_url))
        {
            return;
        }
        let title = if result.artist.trim().is_empty() {
            result.title
        } else {
            format!("{} - {}", result.title, result.artist)
        };
        locked.podcast_sources.push(podcasts::PodcastSource {
            title,
            url: normalized_url.clone(),
            episodes: Vec::new(),
        });
        locked.save();
    }

    refresh_single_podcast_source(normalized_url, rt, settings, podcast_menu_state);
}

fn delete_podcast_source(
    source_index: usize,
    settings: &Arc<Mutex<Settings>>,
    podcast_menu_state: &Arc<Mutex<PodcastMenuState>>,
) {
    let mut locked = settings.lock().unwrap();
    if source_index >= locked.podcast_sources.len() {
        return;
    }
    locked.podcast_sources.remove(source_index);
    locked.save();
    podcast_menu_state.lock().unwrap().dirty = true;
}

fn open_add_podcast_dialog(
    parent: &Frame,
    rt: &Arc<Runtime>,
) -> Option<podcasts::PodcastSearchResult> {
    let dialog = Dialog::builder(parent, "Aggiungi podcast")
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(560, 180)
        .build();
    let panel = Panel::builder(&dialog).build();
    let root = BoxSizer::builder(Orientation::Vertical).build();

    let keyword_row = BoxSizer::builder(Orientation::Horizontal).build();
    keyword_row.add(
        &StaticText::builder(&panel)
            .with_label("Parola chiave:")
            .build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let keyword_ctrl = TextCtrl::builder(&panel).build();
    keyword_row.add(&keyword_ctrl, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&keyword_row, 0, SizerFlag::Expand, 0);

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let ok_button = Button::builder(&panel)
        .with_id(ID_OK)
        .with_label("OK")
        .build();
    buttons.add_spacer(1);
    buttons.add(&ok_button, 0, SizerFlag::All, 10);
    root.add_sizer(&buttons, 0, SizerFlag::Expand, 0);
    panel.set_sizer(root, true);

    dialog.set_affirmative_id(ID_OK);
    let dialog_ok = dialog;
    ok_button.on_click(move |_| {
        dialog_ok.end_modal(ID_OK);
    });

    let result = if dialog.show_modal() == ID_OK {
        let keyword = keyword_ctrl.get_value();
        if keyword.trim().is_empty() {
            None
        } else {
            open_podcast_search_results_dialog(parent, rt, &keyword)
        }
    } else {
        None
    };

    dialog.destroy();
    result
}

fn open_podcast_search_results_dialog(
    parent: &Frame,
    rt: &Arc<Runtime>,
    keyword: &str,
) -> Option<podcasts::PodcastSearchResult> {
    let results = rt
        .block_on(podcasts::search_itunes_podcasts(keyword))
        .ok()?;
    open_podcast_results_dialog(parent, "Scegli podcast", &results)
}

fn open_podcast_category_results_dialog(
    parent: &Frame,
    rt: &Arc<Runtime>,
    category: &podcasts::PodcastCategory,
) -> Option<podcasts::PodcastSearchResult> {
    let results = rt
        .block_on(podcasts::search_itunes_category(category.id))
        .ok()?;
    open_podcast_results_dialog(parent, &format!("Categoria: {}", category.name), &results)
}

fn open_podcast_results_dialog(
    parent: &Frame,
    title: &str,
    results: &[podcasts::PodcastSearchResult],
) -> Option<podcasts::PodcastSearchResult> {
    if results.is_empty() {
        return None;
    }

    let dialog = Dialog::builder(parent, title)
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(620, 180)
        .build();
    let panel = Panel::builder(&dialog).build();
    let root = BoxSizer::builder(Orientation::Vertical).build();

    let result_row = BoxSizer::builder(Orientation::Horizontal).build();
    result_row.add(
        &StaticText::builder(&panel).with_label("Podcast:").build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let choice_result = Choice::builder(&panel).build();
    for result in results {
        let label = if result.artist.trim().is_empty() {
            result.title.clone()
        } else {
            format!("{} - {}", result.title, result.artist)
        };
        choice_result.append(&label);
    }
    choice_result.set_selection(0);
    result_row.add(&choice_result, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&result_row, 0, SizerFlag::Expand, 0);

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let ok_button = Button::builder(&panel)
        .with_id(ID_OK)
        .with_label("OK")
        .build();
    buttons.add_spacer(1);
    buttons.add(&ok_button, 0, SizerFlag::All, 10);
    root.add_sizer(&buttons, 0, SizerFlag::Expand, 0);
    panel.set_sizer(root, true);

    dialog.set_affirmative_id(ID_OK);
    let dialog_ok = dialog;
    ok_button.on_click(move |_| {
        dialog_ok.end_modal(ID_OK);
    });

    let result = if dialog.show_modal() == ID_OK {
        choice_result
            .get_selection()
            .and_then(|selection| results.get(selection as usize).cloned())
    } else {
        None
    };

    dialog.destroy();
    result
}

fn open_delete_podcast_dialog(parent: &Frame, settings: &Arc<Mutex<Settings>>) -> Option<usize> {
    let sources = settings.lock().unwrap().podcast_sources.clone();
    if sources.is_empty() {
        return None;
    }

    let dialog = Dialog::builder(parent, "Elimina podcast")
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(520, 160)
        .build();
    let panel = Panel::builder(&dialog).build();
    let root = BoxSizer::builder(Orientation::Vertical).build();

    let row = BoxSizer::builder(Orientation::Horizontal).build();
    row.add(
        &StaticText::builder(&panel).with_label("Podcast:").build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let choice_source = Choice::builder(&panel).build();
    for source in &sources {
        choice_source.append(&source.title);
    }
    choice_source.set_selection(0);
    row.add(&choice_source, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&row, 0, SizerFlag::Expand, 0);

    let selected_index = Rc::new(RefCell::new(0usize));
    let choice_source_evt = choice_source;
    let selected_index_evt = Rc::clone(&selected_index);
    choice_source.on_selection_changed(move |_| {
        if let Some(selection) = choice_source_evt.get_selection() {
            *selected_index_evt.borrow_mut() = selection as usize;
        }
    });

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let ok_button = Button::builder(&panel)
        .with_id(ID_OK)
        .with_label("OK")
        .build();
    buttons.add_spacer(1);
    buttons.add(&ok_button, 0, SizerFlag::All, 10);
    root.add_sizer(&buttons, 0, SizerFlag::Expand, 0);
    panel.set_sizer(root, true);

    dialog.set_affirmative_id(ID_OK);
    let dialog_ok = dialog;
    ok_button.on_click(move |_| {
        dialog_ok.end_modal(ID_OK);
    });

    let result = if dialog.show_modal() == ID_OK {
        Some(*selected_index.borrow())
    } else {
        None
    };

    dialog.destroy();
    result
}

fn open_add_article_source_dialog(parent: &Frame) -> Option<(String, String)> {
    let dialog = Dialog::builder(parent, "Aggiungi fonte")
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(520, 180)
        .build();
    let panel = Panel::builder(&dialog).build();
    let root = BoxSizer::builder(Orientation::Vertical).build();

    let title_row = BoxSizer::builder(Orientation::Horizontal).build();
    title_row.add(
        &StaticText::builder(&panel).with_label("Titolo:").build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let title_ctrl = TextCtrl::builder(&panel).build();
    title_row.add(&title_ctrl, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&title_row, 0, SizerFlag::Expand, 0);

    let url_row = BoxSizer::builder(Orientation::Horizontal).build();
    url_row.add(
        &StaticText::builder(&panel)
            .with_label("URL o fonte:")
            .build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let url_ctrl = TextCtrl::builder(&panel).build();
    url_row.add(&url_ctrl, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&url_row, 0, SizerFlag::Expand, 0);

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let ok_button = Button::builder(&panel)
        .with_id(ID_OK)
        .with_label("OK")
        .build();
    buttons.add_spacer(1);
    buttons.add(&ok_button, 0, SizerFlag::All, 10);
    root.add_sizer(&buttons, 0, SizerFlag::Expand, 0);
    panel.set_sizer(root, true);

    dialog.set_affirmative_id(ID_OK);
    let dialog_ok = dialog;
    ok_button.on_click(move |_| {
        dialog_ok.end_modal(ID_OK);
    });

    let result = if dialog.show_modal() == ID_OK {
        let title = title_ctrl.get_value();
        let url = url_ctrl.get_value();
        if url.trim().is_empty() {
            None
        } else {
            Some((title, url))
        }
    } else {
        None
    };

    dialog.destroy();
    result
}

fn open_edit_article_source_dialog(
    parent: &Frame,
    settings: &Arc<Mutex<Settings>>,
) -> Option<(usize, String, String)> {
    let sources = settings.lock().unwrap().article_sources.clone();
    if sources.is_empty() {
        return None;
    }

    let dialog = Dialog::builder(parent, "Modifica fonte")
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(560, 220)
        .build();
    let panel = Panel::builder(&dialog).build();
    let root = BoxSizer::builder(Orientation::Vertical).build();

    let source_row = BoxSizer::builder(Orientation::Horizontal).build();
    source_row.add(
        &StaticText::builder(&panel).with_label("Fonte:").build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let choice_source = Choice::builder(&panel).build();
    for source in &sources {
        let label = if source.title.trim().is_empty() {
            source.url.clone()
        } else {
            source.title.clone()
        };
        choice_source.append(&label);
    }
    choice_source.set_selection(0);
    source_row.add(&choice_source, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&source_row, 0, SizerFlag::Expand, 0);

    let title_row = BoxSizer::builder(Orientation::Horizontal).build();
    title_row.add(
        &StaticText::builder(&panel).with_label("Titolo:").build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let title_ctrl = TextCtrl::builder(&panel).build();
    title_row.add(&title_ctrl, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&title_row, 0, SizerFlag::Expand, 0);

    let url_row = BoxSizer::builder(Orientation::Horizontal).build();
    url_row.add(
        &StaticText::builder(&panel)
            .with_label("URL o fonte:")
            .build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let url_ctrl = TextCtrl::builder(&panel).build();
    url_row.add(&url_ctrl, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&url_row, 0, SizerFlag::Expand, 0);

    let selected_index = Rc::new(RefCell::new(0usize));
    if let Some(source) = sources.first() {
        title_ctrl.set_value(&source.title);
        url_ctrl.set_value(&source.url);
    }

    let title_ctrl_evt = title_ctrl;
    let url_ctrl_evt = url_ctrl;
    let choice_source_evt = choice_source;
    let sources_evt = sources.clone();
    let selected_index_evt = Rc::clone(&selected_index);
    choice_source.on_selection_changed(move |_| {
        if let Some(selection) = choice_source_evt.get_selection() {
            let selection = selection as usize;
            *selected_index_evt.borrow_mut() = selection;
            if let Some(source) = sources_evt.get(selection) {
                title_ctrl_evt.set_value(&source.title);
                url_ctrl_evt.set_value(&source.url);
            }
        }
    });

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let ok_button = Button::builder(&panel)
        .with_id(ID_OK)
        .with_label("OK")
        .build();
    buttons.add_spacer(1);
    buttons.add(&ok_button, 0, SizerFlag::All, 10);
    root.add_sizer(&buttons, 0, SizerFlag::Expand, 0);
    panel.set_sizer(root, true);

    dialog.set_affirmative_id(ID_OK);
    let dialog_ok = dialog;
    ok_button.on_click(move |_| {
        dialog_ok.end_modal(ID_OK);
    });

    let result = if dialog.show_modal() == ID_OK {
        let url = url_ctrl.get_value();
        if url.trim().is_empty() {
            None
        } else {
            Some((*selected_index.borrow(), title_ctrl.get_value(), url))
        }
    } else {
        None
    };

    dialog.destroy();
    result
}

fn open_delete_article_source_dialog(
    parent: &Frame,
    settings: &Arc<Mutex<Settings>>,
) -> Option<usize> {
    let sources = settings.lock().unwrap().article_sources.clone();
    if sources.is_empty() {
        return None;
    }

    let dialog = Dialog::builder(parent, "Elimina fonte")
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(520, 160)
        .build();
    let panel = Panel::builder(&dialog).build();
    let root = BoxSizer::builder(Orientation::Vertical).build();

    let source_row = BoxSizer::builder(Orientation::Horizontal).build();
    source_row.add(
        &StaticText::builder(&panel).with_label("Fonte:").build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let choice_source = Choice::builder(&panel).build();
    for source in &sources {
        let label = if source.title.trim().is_empty() {
            source.url.clone()
        } else {
            source.title.clone()
        };
        choice_source.append(&label);
    }
    choice_source.set_selection(0);
    source_row.add(&choice_source, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&source_row, 0, SizerFlag::Expand, 0);

    let selected_index = Rc::new(RefCell::new(0usize));
    let choice_source_evt = choice_source;
    let selected_index_evt = Rc::clone(&selected_index);
    choice_source.on_selection_changed(move |_| {
        if let Some(selection) = choice_source_evt.get_selection() {
            *selected_index_evt.borrow_mut() = selection as usize;
        }
    });

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let ok_button = Button::builder(&panel)
        .with_id(ID_OK)
        .with_label("OK")
        .build();
    buttons.add_spacer(1);
    buttons.add(&ok_button, 0, SizerFlag::All, 10);
    root.add_sizer(&buttons, 0, SizerFlag::Expand, 0);
    panel.set_sizer(root, true);

    dialog.set_affirmative_id(ID_OK);
    let dialog_ok = dialog;
    ok_button.on_click(move |_| {
        dialog_ok.end_modal(ID_OK);
    });

    let result = if dialog.show_modal() == ID_OK {
        Some(*selected_index.borrow())
    } else {
        None
    };

    dialog.destroy();
    result
}

fn open_reorder_article_sources_dialog(
    parent: &Frame,
    settings: &Arc<Mutex<Settings>>,
) -> Option<Vec<articles::ArticleSource>> {
    let sources = settings.lock().unwrap().article_sources.clone();
    if sources.len() < 2 {
        return None;
    }

    let dialog = Dialog::builder(parent, "Riordina fonti")
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(560, 220)
        .build();
    let panel = Panel::builder(&dialog).build();
    let root = BoxSizer::builder(Orientation::Vertical).build();

    let working_sources = Rc::new(RefCell::new(sources));

    let source_row = BoxSizer::builder(Orientation::Horizontal).build();
    source_row.add(
        &StaticText::builder(&panel).with_label("Fonte:").build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let choice_source = Choice::builder(&panel).build();
    source_row.add(&choice_source, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&source_row, 0, SizerFlag::Expand, 0);

    let action_row = BoxSizer::builder(Orientation::Horizontal).build();
    let move_up_button = Button::builder(&panel).with_label("Sposta su").build();
    let move_down_button = Button::builder(&panel).with_label("Sposta giù").build();
    action_row.add(&move_up_button, 1, SizerFlag::All, 5);
    action_row.add(&move_down_button, 1, SizerFlag::All, 5);
    root.add_sizer(&action_row, 0, SizerFlag::Expand, 0);

    let buttons = BoxSizer::builder(Orientation::Horizontal).build();
    let ok_button = Button::builder(&panel)
        .with_id(ID_OK)
        .with_label("OK")
        .build();
    buttons.add_spacer(1);
    buttons.add(&ok_button, 0, SizerFlag::All, 10);
    root.add_sizer(&buttons, 0, SizerFlag::Expand, 0);
    panel.set_sizer(root, true);

    let refresh_choice = Rc::new({
        let working_sources = Rc::clone(&working_sources);
        move |choice: &Choice, selected_index: usize| {
            choice.clear();
            let current_sources = working_sources.borrow();
            for source in current_sources.iter() {
                let label = if source.title.trim().is_empty() {
                    source.url.clone()
                } else {
                    source.title.clone()
                };
                choice.append(&label);
            }
            let max_index = current_sources.len().saturating_sub(1);
            choice.set_selection(selected_index.min(max_index) as u32);
        }
    });

    refresh_choice(&choice_source, 0);

    let selected_index = Rc::new(RefCell::new(0usize));

    let choice_source_evt = choice_source;
    let selected_index_evt = Rc::clone(&selected_index);
    choice_source.on_selection_changed(move |_| {
        if let Some(selection) = choice_source_evt.get_selection() {
            *selected_index_evt.borrow_mut() = selection as usize;
        }
    });

    let choice_source_up = choice_source;
    let selected_index_up = Rc::clone(&selected_index);
    let working_sources_up = Rc::clone(&working_sources);
    let refresh_choice_up = Rc::clone(&refresh_choice);
    move_up_button.on_click(move |_| {
        let current_index = *selected_index_up.borrow();
        if current_index == 0 {
            return;
        }
        {
            let mut sources = working_sources_up.borrow_mut();
            sources.swap(current_index, current_index - 1);
        }
        let new_index = current_index - 1;
        *selected_index_up.borrow_mut() = new_index;
        refresh_choice_up(&choice_source_up, new_index);
    });

    let choice_source_down = choice_source;
    let selected_index_down = Rc::clone(&selected_index);
    let working_sources_down = Rc::clone(&working_sources);
    let refresh_choice_down = Rc::clone(&refresh_choice);
    move_down_button.on_click(move |_| {
        let current_index = *selected_index_down.borrow();
        let len = working_sources_down.borrow().len();
        if current_index + 1 >= len {
            return;
        }
        {
            let mut sources = working_sources_down.borrow_mut();
            sources.swap(current_index, current_index + 1);
        }
        let new_index = current_index + 1;
        *selected_index_down.borrow_mut() = new_index;
        refresh_choice_down(&choice_source_down, new_index);
    });

    dialog.set_affirmative_id(ID_OK);
    let dialog_ok = dialog;
    ok_button.on_click(move |_| {
        dialog_ok.end_modal(ID_OK);
    });

    let result = if dialog.show_modal() == ID_OK {
        Some(working_sources.borrow().clone())
    } else {
        None
    };

    dialog.destroy();
    result
}

fn apply_loaded_voices(
    settings: &Arc<Mutex<Settings>>,
    voices_data: &Arc<Mutex<Vec<edge_tts::VoiceInfo>>>,
    languages: &Arc<Mutex<Vec<(String, String)>>>,
    voices: Vec<edge_tts::VoiceInfo>,
) {
    let language_list = build_language_list(&voices);
    {
        let mut v_lock = voices_data.lock().unwrap();
        *v_lock = voices;
    }
    {
        let mut l_lock = languages.lock().unwrap();
        *l_lock = language_list.clone();
    }
    sync_settings_with_loaded_voices(settings, &voices_data.lock().unwrap(), &language_list);
}

fn refresh_playback_if_needed(playback: &Arc<Mutex<GlobalPlayback>>) {
    let mut pb = playback.lock().unwrap();
    if pb.status == PlaybackStatus::Playing {
        pb.refresh_requested = true;
        if let Some(ref sink) = pb.sink {
            sink.stop();
        }
    }
}

fn stop_tts_playback(playback: &Arc<Mutex<GlobalPlayback>>) {
    let mut pb = playback.lock().unwrap();
    if let Some(ref sink) = pb.sink {
        sink.stop();
    }
    pb.sink = None;
    pb.status = PlaybackStatus::Stopped;
    pb.refresh_requested = false;
    pb.download_finished = false;
}

fn stop_podcast_playback(state: &Rc<RefCell<PodcastPlaybackState>>) {
    let mut podcast_state = state.borrow_mut();
    let current_audio_url = podcast_state.current_audio_url.clone();
    if let Some(player) = podcast_state.player.as_ref() {
        log_podcast_player_snapshot(player, "stop_podcast.before_pause", &current_audio_url);
        if let Err(err) = player.pause() {
            println!("ERROR: Pausa podcast fallita: {}", err);
            append_podcast_log(&format!(
                "stop_podcast.pause_error audio_url={} error={}",
                current_audio_url, err
            ));
        } else {
            log_podcast_player_snapshot(player, "stop_podcast.after_pause", &current_audio_url);
        }
    }
    podcast_state.player = None;
    podcast_state.status = PlaybackStatus::Stopped;
    append_podcast_log(&format!(
        "stop_podcast.completed audio_url={} status={:?}",
        current_audio_url, podcast_state.status
    ));
}

fn seek_podcast_playback(state: &Rc<RefCell<PodcastPlaybackState>>, offset_seconds: f64) {
    let podcast_state = state.borrow();
    if let Some(player) = podcast_state.player.as_ref() {
        log_podcast_player_snapshot(
            player,
            &format!("seek_podcast.before offset_seconds={offset_seconds}"),
            &podcast_state.current_audio_url,
        );
        if let Err(err) = player.seek_by_seconds(offset_seconds) {
            println!("ERROR: Seek podcast fallito: {}", err);
            append_podcast_log(&format!(
                "seek_podcast.error audio_url={} offset_seconds={} error={}",
                podcast_state.current_audio_url, offset_seconds, err
            ));
        } else {
            log_podcast_player_snapshot(
                player,
                &format!("seek_podcast.after offset_seconds={offset_seconds}"),
                &podcast_state.current_audio_url,
            );
        }
    }
}

fn sync_settings_with_loaded_voices(
    settings: &Arc<Mutex<Settings>>,
    voices: &[edge_tts::VoiceInfo],
    languages: &[(String, String)],
) {
    if languages.is_empty() {
        return;
    }

    let mut changed = false;
    let mut s = settings.lock().unwrap();

    if !languages.iter().any(|(name, _)| name == &s.language) {
        if languages.iter().any(|(name, _)| name == "Italiano") {
            s.language = "Italiano".to_string();
        } else if let Some((name, _)) = languages.first() {
            s.language = name.clone();
        }
        changed = true;
    }

    let locale = languages
        .iter()
        .find(|(name, _)| name == &s.language)
        .map(|(_, locale)| locale.clone());
    if let Some(locale) = locale {
        let available_voices: Vec<_> = voices.iter().filter(|v| v.locale == locale).collect();
        if !available_voices
            .iter()
            .any(|voice| voice.short_name == s.voice)
            && let Some(voice) = available_voices.first()
        {
            s.voice = voice.short_name.clone();
            changed = true;
        }
    }

    if changed {
        s.save();
    }
}

fn open_settings_dialog(
    parent: &Frame,
    settings: &Arc<Mutex<Settings>>,
    voices_data: &Arc<Mutex<Vec<edge_tts::VoiceInfo>>>,
    languages: &Arc<Mutex<Vec<(String, String)>>>,
    playback: &Arc<Mutex<GlobalPlayback>>,
) {
    let settings_before = settings.lock().unwrap().clone();
    let languages_snapshot = languages.lock().unwrap().clone();
    let voices_snapshot = voices_data.lock().unwrap().clone();

    let dialog = Dialog::builder(parent, "Impostazioni")
        .with_style(DialogStyle::DefaultDialogStyle | DialogStyle::ResizeBorder)
        .with_size(560, 320)
        .build();
    let panel = Panel::builder(&dialog).build();
    let root = BoxSizer::builder(Orientation::Vertical).build();

    let lang_row = BoxSizer::builder(Orientation::Horizontal).build();
    lang_row.add(
        &StaticText::builder(&panel).with_label("Lingua:").build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let choice_lang = Choice::builder(&panel).build();
    lang_row.add(&choice_lang, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&lang_row, 0, SizerFlag::Expand, 0);

    let voice_row = BoxSizer::builder(Orientation::Horizontal).build();
    voice_row.add(
        &StaticText::builder(&panel).with_label("Voce:").build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let choice_voices = Choice::builder(&panel).build();
    voice_row.add(&choice_voices, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&voice_row, 0, SizerFlag::Expand, 0);

    let rate_row = BoxSizer::builder(Orientation::Horizontal).build();
    rate_row.add(
        &StaticText::builder(&panel).with_label("Velocità:").build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let choice_rate = Choice::builder(&panel).build();
    for (label, _) in RATE_PRESETS {
        choice_rate.append(label);
    }
    choice_rate.set_selection(nearest_preset_index(&RATE_PRESETS, settings_before.rate) as u32);
    rate_row.add(&choice_rate, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&rate_row, 0, SizerFlag::Expand, 0);

    let pitch_row = BoxSizer::builder(Orientation::Horizontal).build();
    pitch_row.add(
        &StaticText::builder(&panel).with_label("Tono:").build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let choice_pitch = Choice::builder(&panel).build();
    for (label, _) in PITCH_PRESETS {
        choice_pitch.append(label);
    }
    choice_pitch.set_selection(nearest_preset_index(&PITCH_PRESETS, settings_before.pitch) as u32);
    pitch_row.add(&choice_pitch, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&pitch_row, 0, SizerFlag::Expand, 0);

    let volume_row = BoxSizer::builder(Orientation::Horizontal).build();
    volume_row.add(
        &StaticText::builder(&panel).with_label("Volume:").build(),
        0,
        SizerFlag::AlignCenterVertical | SizerFlag::All,
        5,
    );
    let choice_volume = Choice::builder(&panel).build();
    for (label, _) in VOLUME_PRESETS {
        choice_volume.append(label);
    }
    choice_volume
        .set_selection(nearest_preset_index(&VOLUME_PRESETS, settings_before.volume) as u32);
    volume_row.add(&choice_volume, 1, SizerFlag::Expand | SizerFlag::All, 5);
    root.add_sizer(&volume_row, 0, SizerFlag::Expand, 0);

    let button_row = BoxSizer::builder(Orientation::Horizontal).build();
    let btn_ok = Button::builder(&panel)
        .with_id(ID_OK)
        .with_label("OK")
        .build();
    button_row.add_spacer(1);
    button_row.add(&btn_ok, 0, SizerFlag::All, 10);
    root.add_sizer(&button_row, 0, SizerFlag::Expand, 0);

    panel.set_sizer(root, true);

    for (name, _) in &languages_snapshot {
        choice_lang.append(name);
    }
    if let Some(pos) = languages_snapshot
        .iter()
        .position(|(name, _)| name == &settings_before.language)
    {
        choice_lang.set_selection(pos as u32);
    } else if let Some(pos) = languages_snapshot
        .iter()
        .position(|(name, _)| name == "Italiano")
    {
        choice_lang.set_selection(pos as u32);
    } else if !languages_snapshot.is_empty() {
        choice_lang.set_selection(0);
    }

    let selected_voice = Rc::new(RefCell::new(settings_before.voice.clone()));
    let filtered_voices = Rc::new(RefCell::new(Vec::<edge_tts::VoiceInfo>::new()));
    let filtered_voices_init = Rc::clone(&filtered_voices);
    let selected_voice_init = Rc::clone(&selected_voice);
    let choice_voices_fill = choice_voices;
    let choice_voices_evt = choice_voices;
    let choice_lang_evt = choice_lang;

    let populate_voices = Rc::new(move |lang_sel: u32| {
        let locale = languages_snapshot
            .get(lang_sel as usize)
            .map(|(_, locale)| locale.clone())
            .unwrap_or_default();
        let voice_list: Vec<_> = voices_snapshot
            .iter()
            .filter(|voice| voice.locale == locale)
            .cloned()
            .collect();
        choice_voices_fill.clear();
        for voice in &voice_list {
            choice_voices_fill.append(&voice.friendly_name);
        }

        let preferred = selected_voice_init.borrow().clone();
        if let Some(pos) = voice_list
            .iter()
            .position(|voice| voice.short_name == preferred)
        {
            choice_voices_fill.set_selection(pos as u32);
        } else if let Some(first) = voice_list.first() {
            choice_voices_fill.set_selection(0);
            *selected_voice_init.borrow_mut() = first.short_name.clone();
        } else {
            selected_voice_init.borrow_mut().clear();
        }
        *filtered_voices_init.borrow_mut() = voice_list;
    });

    if let Some(sel) = choice_lang.get_selection() {
        populate_voices(sel);
    }

    let populate_voices_evt = Rc::clone(&populate_voices);
    choice_lang.on_selection_changed(move |_| {
        if let Some(sel) = choice_lang_evt.get_selection() {
            populate_voices_evt(sel);
        }
    });

    let filtered_voices_choice = Rc::clone(&filtered_voices);
    let selected_voice_choice = Rc::clone(&selected_voice);
    choice_voices.on_selection_changed(move |_| {
        if let Some(sel) = choice_voices_evt.get_selection()
            && let Some(voice) = filtered_voices_choice.borrow().get(sel as usize)
        {
            *selected_voice_choice.borrow_mut() = voice.short_name.clone();
        }
    });

    dialog.set_affirmative_id(ID_OK);
    let dialog_ok = dialog;
    btn_ok.on_click(move |_| {
        dialog_ok.end_modal(ID_OK);
    });

    if dialog.show_modal() == ID_OK {
        let mut updated = settings_before.clone();
        if let Some(sel) = choice_lang.get_selection()
            && let Some((name, _)) = languages.lock().unwrap().get(sel as usize)
        {
            updated.language = name.clone();
        }
        let chosen_voice = selected_voice.borrow().clone();
        if !chosen_voice.is_empty() {
            updated.voice = chosen_voice;
        }
        if let Some(sel) = choice_rate.get_selection() {
            updated.rate = RATE_PRESETS[sel as usize].1;
        }
        if let Some(sel) = choice_pitch.get_selection() {
            updated.pitch = PITCH_PRESETS[sel as usize].1;
        }
        if let Some(sel) = choice_volume.get_selection() {
            updated.volume = VOLUME_PRESETS[sel as usize].1;
        }

        let refresh_needed = settings_before.voice != updated.voice
            || settings_before.rate != updated.rate
            || settings_before.pitch != updated.pitch
            || settings_before.volume != updated.volume;
        let changed = settings_before.language != updated.language || refresh_needed;

        if changed {
            let mut locked = settings.lock().unwrap();
            *locked = updated;
            locked.save();
        }
        if refresh_needed {
            refresh_playback_if_needed(playback);
        }
    }

    dialog.destroy();
}

fn main() {
    #[cfg(windows)]
    SystemOptions::set_option_by_int("msw.no-manifest-check", 1);

    #[cfg(target_os = "macos")]
    {
        let bundled_curl_libraries = articles::bundled_curl_impersonate_libraries();
        if bundled_curl_libraries.is_empty() {
            println!("INFO: Nessuna libreria curl-impersonate rilevata nel bundle macOS");
        } else {
            for library in bundled_curl_libraries {
                println!(
                    "INFO: Libreria curl-impersonate rilevata nel bundle macOS: {}",
                    library.display()
                );
            }
        }
    }

    let rt = Arc::new(Runtime::new().unwrap());
    let voices_data = Arc::new(Mutex::new(Vec::<edge_tts::VoiceInfo>::new()));
    let languages = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let settings = Arc::new(Mutex::new(Settings::load()));
    let article_menu_state = Arc::new(Mutex::new(ArticleMenuState {
        dirty: true,
        loading_urls: HashSet::new(),
    }));
    let podcast_menu_state = Arc::new(Mutex::new(PodcastMenuState {
        dirty: true,
        loading_urls: HashSet::new(),
    }));
    let podcast_playback = Rc::new(RefCell::new(PodcastPlaybackState {
        player: None,
        selected_episode: None,
        current_audio_url: String::new(),
        status: PlaybackStatus::Stopped,
    }));

    let playback = Arc::new(Mutex::new(GlobalPlayback {
        sink: None,
        status: PlaybackStatus::Stopped,
        download_finished: false,
        refresh_requested: false,
    }));

    if let Some(cached_voices) = load_cached_voices() {
        apply_loaded_voices(&settings, &voices_data, &languages, cached_voices);
    }

    let rt_refresh = Arc::clone(&rt);
    let settings_refresh = Arc::clone(&settings);
    let voices_refresh = Arc::clone(&voices_data);
    let languages_refresh = Arc::clone(&languages);
    std::thread::spawn(
        move || match rt_refresh.block_on(edge_tts::get_edge_voices()) {
            Ok(voices) => {
                save_cached_voices(&voices);
                apply_loaded_voices(
                    &settings_refresh,
                    &voices_refresh,
                    &languages_refresh,
                    voices,
                );
            }
            Err(err) => {
                println!("ERROR: Aggiornamento voci fallito: {}", err);
            }
        },
    );

    refresh_all_article_sources(&rt, &settings, &article_menu_state);
    refresh_all_podcast_sources(&rt, &settings, &podcast_menu_state);

    let _ = wxdragon::main(move |_| {
        let frame = Frame::builder()
            .with_title("Sonarpad Minimal")
            .with_size(Size::new(800, 700))
            .build();

        let file_menu = Menu::builder().build();
        file_menu.append(
            ID_OPEN,
            "&Apri...\tCtrl+O",
            "Apri un documento",
            ItemKind::Normal,
        );
        file_menu.append_separator();
        #[cfg(target_os = "macos")]
        let play_menu_item = file_menu.append(
            ID_PLAY_PAUSE,
            "Avvia o pausa lettura\tCtrl+L",
            "Avvia o mette in pausa la lettura",
            ItemKind::Normal,
        );
        #[cfg(target_os = "macos")]
        let stop_menu_item = file_menu.append(
            ID_STOP,
            "Ferma lettura\tCtrl+.",
            "Ferma la lettura o il podcast",
            ItemKind::Normal,
        );
        #[cfg(target_os = "macos")]
        let save_menu_item = file_menu.append(
            ID_SAVE,
            "Salva audiolibro\tCtrl+Alt+A",
            "Salva il testo corrente come audiolibro",
            ItemKind::Normal,
        );
        #[cfg(target_os = "macos")]
        let settings_menu_item = file_menu.append(
            ID_SETTINGS,
            "Impostazioni\tCtrl+,",
            "Apre le impostazioni",
            ItemKind::Normal,
        );
        #[cfg(target_os = "macos")]
        file_menu.append_separator();
        file_menu.append(
            ID_EXIT,
            "&Esci\tCtrl+Q",
            "Esci dal programma",
            ItemKind::Normal,
        );
        let help_menu = Menu::builder().build();
        help_menu.append(
            ID_ABOUT,
            "&Informazioni sul programma...",
            "Mostra informazioni sul programma",
            ItemKind::Normal,
        );
        help_menu.append(
            ID_DONATIONS,
            "&Donazioni",
            "Mostra informazioni per sostenere il progetto",
            ItemKind::Normal,
        );

        let articles_menu = Menu::builder().build();
        rebuild_articles_menu(&articles_menu, &settings, &HashSet::new());
        let articles_menu_timer = Menu::from(articles_menu.as_const_ptr());
        let podcasts_menu = Menu::builder().build();
        rebuild_podcasts_menu(&podcasts_menu, &settings, &HashSet::new());
        let podcasts_menu_timer = Menu::from(podcasts_menu.as_const_ptr());

        let menubar = MenuBar::builder()
            .append(file_menu, "&File")
            .append(articles_menu, "&Articoli")
            .append(podcasts_menu, "&Podcast")
            .append(help_menu, "&Aiuto")
            .build();
        frame.set_menu_bar(menubar);

        let panel = Panel::builder(&frame).build();
        let main_sizer = BoxSizer::builder(Orientation::Vertical).build();

        let text_ctrl = TextCtrl::builder(&panel)
            .with_style(TextCtrlStyle::MultiLine)
            .build();
        main_sizer.add(&text_ctrl, 1, SizerFlag::Expand | SizerFlag::All, 5);

        let btn_sizer = BoxSizer::builder(Orientation::Horizontal).build();
        let btn_play = Button::builder(&panel)
            .with_id(ID_PLAY_PAUSE)
            .with_label(&play_button_label(PlaybackStatus::Stopped, false))
            .build();
        btn_sizer.add(&btn_play, 1, SizerFlag::All, 10);
        let btn_stop = Button::builder(&panel)
            .with_id(ID_STOP)
            .with_label(&stop_button_label(false))
            .build();
        btn_sizer.add(&btn_stop, 1, SizerFlag::All, 10);
        let btn_podcast_back = Button::builder(&panel)
            .with_id(ID_PODCAST_BACKWARD)
            .with_label(&format!("Indietro 30s ({}+Left)", MOD_CMD))
            .build();
        btn_podcast_back.show(false);
        btn_sizer.add(&btn_podcast_back, 1, SizerFlag::All, 10);
        let btn_podcast_forward = Button::builder(&panel)
            .with_id(ID_PODCAST_FORWARD)
            .with_label(&format!("Avanti 30s ({}+Right)", MOD_CMD))
            .build();
        btn_podcast_forward.show(false);
        btn_sizer.add(&btn_podcast_forward, 1, SizerFlag::All, 10);
        let btn_save = Button::builder(&panel)
            .with_id(ID_SAVE)
            .with_label(&save_button_label())
            .build();
        btn_sizer.add(&btn_save, 1, SizerFlag::All, 10);
        let btn_settings = Button::builder(&panel)
            .with_id(ID_SETTINGS)
            .with_label(&settings_button_label())
            .build();
        btn_sizer.add(&btn_settings, 1, SizerFlag::All, 10);

        main_sizer.add_sizer(&btn_sizer, 0, SizerFlag::Expand, 0);
        panel.set_sizer(main_sizer, true);

        // --- Timer per aggiornamento UI ---
        let timer = Rc::new(Timer::new(&frame));
        let pb_timer = Arc::clone(&playback);
        let btn_play_timer = btn_play;
        let btn_stop_timer = btn_stop;
        let btn_podcast_back_timer = btn_podcast_back;
        let btn_podcast_forward_timer = btn_podcast_forward;
        let panel_timer = panel;
        let settings_timer = Arc::clone(&settings);
        let article_menu_state_timer = Arc::clone(&article_menu_state);
        let podcast_menu_state_timer = Arc::clone(&podcast_menu_state);
        let podcast_playback_timer = Rc::clone(&podcast_playback);
        let timer_tick = Rc::clone(&timer);

        timer_tick.on_tick(move |_| {
            let tts_status = pb_timer.lock().unwrap().status;
            let podcast_state = podcast_playback_timer.borrow();
            let podcast_status = podcast_state.status;
            let podcast_mode = podcast_state.selected_episode.is_some();
            let label = play_button_label(
                if podcast_status != PlaybackStatus::Stopped {
                    podcast_status
                } else {
                    tts_status
                },
                podcast_mode,
            );
            if btn_play_timer.get_label() != label {
                btn_play_timer.set_label(&label);
            }
            let stop_label = stop_button_label(podcast_mode);
            if btn_stop_timer.get_label() != stop_label {
                btn_stop_timer.set_label(&stop_label);
            }
            let seek_visible = podcast_mode;
            btn_podcast_back_timer.show(seek_visible);
            btn_podcast_forward_timer.show(seek_visible);
            panel_timer.layout();
            let article_loading_urls = {
                let mut article_state = article_menu_state_timer.lock().unwrap();
                if article_state.dirty {
                    article_state.dirty = false;
                    Some(article_state.loading_urls.clone())
                } else {
                    None
                }
            };
            if let Some(loading_urls) = article_loading_urls {
                rebuild_articles_menu(&articles_menu_timer, &settings_timer, &loading_urls);
            }

            let podcast_loading_urls = {
                let mut podcast_state = podcast_menu_state_timer.lock().unwrap();
                if podcast_state.dirty {
                    podcast_state.dirty = false;
                    Some(podcast_state.loading_urls.clone())
                } else {
                    None
                }
            };
            if let Some(loading_urls) = podcast_loading_urls {
                rebuild_podcasts_menu(&podcasts_menu_timer, &settings_timer, &loading_urls);
            }
        });
        timer.start(200, false);

        let timer_close = Rc::clone(&timer);
        frame.on_close(move |event| {
            timer_close.stop();
            event.skip(true);
        });

        let timer_destroy = Rc::clone(&timer);
        frame.on_destroy(move |event| {
            timer_destroy.stop();
            event.skip(true);
        });

        // --- Menu ---
        let f_menu = frame;
        let tc_menu = text_ctrl;
        let settings_menu = Arc::clone(&settings);
        let article_menu_state_menu = Arc::clone(&article_menu_state);
        let podcast_menu_state_menu = Arc::clone(&podcast_menu_state);
        let rt_articles_menu = Arc::clone(&rt);
        let podcast_selection_menu = Rc::clone(&podcast_playback);
        frame.on_menu(move |event| {
            if event.get_id() == ID_OPEN {
                let dialog = FileDialog::builder(&f_menu).with_message("Apri").with_wildcard("Supportati|*.txt;*.doc;*.docx;*.pdf;*.epub;*.rtf;*.xlsx;*.xls;*.ods;*.html;*.htm|Tutti|*.*").build();
                if dialog.show_modal() == ID_OK
                    && let Some(path) = dialog.get_path()
                        && let Ok(c) = file_loader::load_any_file(Path::new(&path)) {
                    podcast_selection_menu.borrow_mut().selected_episode = None;
                    tc_menu.set_value(&c);
                }
            } else if event.get_id() == ID_EXIT {
                f_menu.close(true);
            } else if event.get_id() == ID_ABOUT {
                let dialog = MessageDialog::builder(&f_menu, &about_message(), about_title())
                    .with_style(MessageDialogStyle::OK | MessageDialogStyle::IconInformation)
                    .build();
                dialog.show_modal();
            } else if event.get_id() == ID_DONATIONS {
                open_donations_dialog(&f_menu);
            } else if event.get_id() == ID_ARTICLES_ADD_SOURCE {
                if let Some((title, url)) = open_add_article_source_dialog(&f_menu) {
                    add_article_source(
                        title,
                        url,
                        &settings_menu,
                        &article_menu_state_menu,
                        &rt_articles_menu,
                    );
                }
            } else if event.get_id() == ID_ARTICLES_EDIT_SOURCE {
                if let Some((source_index, title, url)) =
                    open_edit_article_source_dialog(&f_menu, &settings_menu)
                {
                    edit_article_source(
                        source_index,
                        title,
                        url,
                        &settings_menu,
                        &article_menu_state_menu,
                        &rt_articles_menu,
                    );
                }
            } else if event.get_id() == ID_ARTICLES_DELETE_SOURCE {
                if let Some(source_index) =
                    open_delete_article_source_dialog(&f_menu, &settings_menu)
                {
                    delete_article_source(
                        source_index,
                        &settings_menu,
                        &article_menu_state_menu,
                    );
                }
            } else if event.get_id() == ID_ARTICLES_REORDER_SOURCES {
                if let Some(reordered_sources) =
                    open_reorder_article_sources_dialog(&f_menu, &settings_menu)
                {
                    save_reordered_article_sources(
                        reordered_sources,
                        &settings_menu,
                        &article_menu_state_menu,
                    );
                }
            } else if event.get_id() == ID_PODCASTS_ADD {
                if let Some(result) = open_add_podcast_dialog(&f_menu, &rt_articles_menu) {
                    add_podcast_source(
                        result,
                        &settings_menu,
                        &podcast_menu_state_menu,
                        &rt_articles_menu,
                    );
                }
            } else if event.get_id() == ID_PODCASTS_DELETE {
                if let Some(source_index) = open_delete_podcast_dialog(&f_menu, &settings_menu) {
                    delete_podcast_source(source_index, &settings_menu, &podcast_menu_state_menu);
                }
            } else if (ID_PODCASTS_CATEGORY_BASE
                ..ID_PODCASTS_CATEGORY_BASE + podcasts::apple_categories_it().len() as i32)
                .contains(&event.get_id())
            {
                let index = (event.get_id() - ID_PODCASTS_CATEGORY_BASE) as usize;
                if let Some(category) = podcasts::apple_categories_it().get(index)
                    && let Some(result) =
                        open_podcast_category_results_dialog(&f_menu, &rt_articles_menu, category)
                {
                    add_podcast_source(
                        result,
                        &settings_menu,
                        &podcast_menu_state_menu,
                        &rt_articles_menu,
                    );
                }
            } else if let Some((source_index, episode_index)) =
                decode_podcast_episode_menu_id(event.get_id())
            {
                append_podcast_log(&format!(
                    "podcast_menu.select source_index={} episode_index={} event_id={}",
                    source_index,
                    episode_index,
                    event.get_id()
                ));
                let episode = settings_menu
                    .lock()
                    .unwrap()
                    .podcast_sources
                    .get(source_index)
                    .and_then(|source| source.episodes.get(episode_index))
                    .cloned();
                if let Some(episode) = episode {
                    let description = crate::reader::collapse_blank_lines(
                        &crate::reader::clean_text(&episode.description),
                    );
                    tc_menu.set_value(&format!("{}\n\n{}", episode.title.trim(), description.trim()));

                    #[cfg(any(target_os = "macos", windows))]
                    {
                        if episode.audio_url.trim().is_empty() {
                            append_podcast_log(&format!(
                                "podcast_menu.no_audio_url title={} link={}",
                                episode.title, episode.link
                            ));
                            let dialog = MessageDialog::builder(
                                &f_menu,
                                "Questo episodio non espone un URL audio diretto nel feed RSS.\n\nNon posso scaricare la pagina web al posto dell'audio.",
                                "Audio podcast non disponibile",
                            )
                            .with_style(MessageDialogStyle::OK | MessageDialogStyle::IconError)
                            .build();
                            dialog.show_modal();
                            return;
                        }

                        let external_url = episode.audio_url.as_str();
                        append_podcast_log(&format!(
                            "podcast_menu.episode_resolved title={} audio_url={} link={} external_url={}",
                            episode.title,
                            episode.audio_url,
                            episode.link,
                            external_url
                        ));

                        let mut playback_state = podcast_selection_menu.borrow_mut();
                        if let Some(player) = playback_state.player.as_ref()
                            && let Err(err) = player.pause()
                        {
                            println!("ERROR: Pausa podcast fallita: {}", err);
                        }
                        playback_state.player = None;
                        playback_state.selected_episode = None;
                        playback_state.current_audio_url.clear();
                        playback_state.status = PlaybackStatus::Stopped;
                        drop(playback_state);
                        append_podcast_log("podcast_menu.external_open_call");

                        if let Err(err) = open_podcast_episode_externally(&f_menu, external_url) {
                            append_podcast_log(&format!(
                                "podcast_menu.external_open_error error={}",
                                err
                            ));
                            println!("ERROR: Apertura esterna podcast fallita: {}", err);
                            let dialog = MessageDialog::builder(
                                &f_menu,
                                &format!("Impossibile aprire il podcast.\n\n{err}"),
                                "Errore podcast",
                            )
                            .with_style(MessageDialogStyle::OK | MessageDialogStyle::IconError)
                            .build();
                            dialog.show_modal();
                        } else {
                            append_podcast_log("podcast_menu.external_open_ok");
                        }
                    }

                    #[cfg(not(any(target_os = "macos", windows)))]
                    {
                        podcast_selection_menu.borrow_mut().selected_episode = Some(episode.clone());
                    }
                }
            } else if let Some((source_index, item_index)) = decode_article_menu_id(event.get_id()) {
                let item = settings_menu
                    .lock()
                    .unwrap()
                    .article_sources
                    .get(source_index)
                    .and_then(|source| source.items.get(item_index))
                    .cloned();
                if let Some(item) = item
                    && let Ok(text) = rt_articles_menu.block_on(articles::fetch_article_text(&item))
                {
                    podcast_selection_menu.borrow_mut().selected_episode = None;
                    tc_menu.set_value(&text);
                }
            }
        });

        // --- Play / Pausa / Stop ---
        let rt_p = Arc::clone(&rt);
        let pb_p = Arc::clone(&playback);
        let tc_p = text_ctrl;
        let b_p_label = btn_play;
        let f_play = frame;
        let s_play = Arc::clone(&settings);
        let podcast_playback_play = Rc::clone(&podcast_playback);
        let play_action: Rc<dyn Fn()> = Rc::new(move || {
            let selected_episode = podcast_playback_play.borrow().selected_episode.clone();
            if let Some(episode) = selected_episode
                && !episode.audio_url.trim().is_empty()
            {
                stop_tts_playback(&pb_p);
                append_podcast_log(&format!(
                    "play_action.selected_episode title={} audio_url={} previous_status={:?}",
                    episode.title,
                    episode.audio_url,
                    podcast_playback_play.borrow().status
                ));

                let mut podcast_state = podcast_playback_play.borrow_mut();
                let needs_new_player = podcast_state.player.is_none()
                    || !podcast_state
                        .current_audio_url
                        .eq_ignore_ascii_case(&episode.audio_url);

                if needs_new_player {
                    match podcast_player::PodcastPlayer::new(&episode.audio_url) {
                        Ok(player) => {
                            log_podcast_player_snapshot(
                                &player,
                                "play_action.new_player",
                                &episode.audio_url,
                            );
                            podcast_state.player = Some(player);
                            podcast_state.current_audio_url = episode.audio_url.clone();
                        }
                        Err(err) => {
                            println!("ERROR: Avvio player podcast fallito: {}", err);
                            append_podcast_log(&format!(
                                "play_action.new_player_error audio_url={} error={}",
                                episode.audio_url, err
                            ));
                            podcast_state.status = PlaybackStatus::Stopped;
                            return;
                        }
                    }
                }

                match podcast_state.status {
                    PlaybackStatus::Playing => {
                        if let Some(player) = podcast_state.player.as_ref() {
                            log_podcast_player_snapshot(
                                player,
                                "play_action.pause.before",
                                &episode.audio_url,
                            );
                            if let Err(err) = player.pause() {
                                println!("ERROR: Pausa podcast fallita: {}", err);
                                append_podcast_log(&format!(
                                    "play_action.pause.error audio_url={} error={}",
                                    episode.audio_url, err
                                ));
                                podcast_state.status = PlaybackStatus::Stopped;
                                return;
                            }
                            log_podcast_player_snapshot(
                                player,
                                "play_action.pause.after",
                                &episode.audio_url,
                            );
                        }
                        podcast_state.status = PlaybackStatus::Paused;
                        b_p_label.set_label(&play_button_label(PlaybackStatus::Paused, true));
                    }
                    PlaybackStatus::Paused => {
                        if let Some(player) = podcast_state.player.as_ref() {
                            log_podcast_player_snapshot(
                                player,
                                "play_action.resume.before",
                                &episode.audio_url,
                            );
                            if let Err(err) = player.play() {
                                println!("ERROR: Ripresa podcast fallita: {}", err);
                                append_podcast_log(&format!(
                                    "play_action.resume.error audio_url={} error={}",
                                    episode.audio_url, err
                                ));
                                podcast_state.status = PlaybackStatus::Stopped;
                                return;
                            }
                            log_podcast_player_snapshot(
                                player,
                                "play_action.resume.after",
                                &episode.audio_url,
                            );
                            if needs_new_player
                                && !wait_for_podcast_ready(&f_play, player, &episode.audio_url)
                            {
                                if let Err(err) = player.pause() {
                                    println!("ERROR: Pausa podcast dopo timeout fallita: {}", err);
                                    append_podcast_log(&format!(
                                        "play_action.resume.cleanup_error audio_url={} error={}",
                                        episode.audio_url, err
                                    ));
                                }
                                podcast_state.status = PlaybackStatus::Stopped;
                                return;
                            }
                        }
                        podcast_state.status = PlaybackStatus::Playing;
                        b_p_label.set_label(&play_button_label(PlaybackStatus::Playing, true));
                    }
                    PlaybackStatus::Stopped => {
                        if let Some(player) = podcast_state.player.as_ref() {
                            log_podcast_player_snapshot(
                                player,
                                "play_action.play.before",
                                &episode.audio_url,
                            );
                            if let Err(err) = player.play() {
                                println!("ERROR: Riproduzione podcast fallita: {}", err);
                                append_podcast_log(&format!(
                                    "play_action.play.error audio_url={} error={}",
                                    episode.audio_url, err
                                ));
                                podcast_state.status = PlaybackStatus::Stopped;
                                return;
                            }
                            log_podcast_player_snapshot(
                                player,
                                "play_action.play.after",
                                &episode.audio_url,
                            );
                            if !wait_for_podcast_ready(&f_play, player, &episode.audio_url) {
                                if let Err(err) = player.pause() {
                                    println!("ERROR: Pausa podcast dopo timeout fallita: {}", err);
                                    append_podcast_log(&format!(
                                        "play_action.play.cleanup_error audio_url={} error={}",
                                        episode.audio_url, err
                                    ));
                                }
                                podcast_state.status = PlaybackStatus::Stopped;
                                return;
                            }
                        }
                        podcast_state.current_audio_url = episode.audio_url.clone();
                        podcast_state.status = PlaybackStatus::Playing;
                        b_p_label.set_label(&play_button_label(PlaybackStatus::Playing, true));
                    }
                }
                append_podcast_log(&format!(
                    "play_action.completed audio_url={} new_status={:?}",
                    episode.audio_url, podcast_state.status
                ));
                return;
            }

            stop_podcast_playback(&podcast_playback_play);
            let mut pb = pb_p.lock().unwrap();
            match pb.status {
                PlaybackStatus::Playing => {
                    if let Some(ref s) = pb.sink {
                        s.pause();
                        pb.status = PlaybackStatus::Paused;
                        b_p_label.set_label(&play_button_label(PlaybackStatus::Paused, false));
                    }
                }
                PlaybackStatus::Paused => {
                    if let Some(ref s) = pb.sink {
                        s.play();
                        pb.status = PlaybackStatus::Playing;
                        b_p_label.set_label(&play_button_label(PlaybackStatus::Playing, false));
                    }
                }
                PlaybackStatus::Stopped => {
                    let text = tc_p.get_value();
                    if text.trim().is_empty() {
                        return;
                    }

                    b_p_label.set_label(&play_button_label(PlaybackStatus::Playing, false));
                    pb.status = PlaybackStatus::Playing;
                    pb.download_finished = false;
                    pb.refresh_requested = false;

                    let rt_thread = Arc::clone(&rt_p);
                    let pb_thread = Arc::clone(&pb_p);
                    let s_thread = Arc::clone(&s_play);

                    std::thread::spawn(move || {
                        if let Ok((_stream, handle)) = OutputStream::try_default()
                            && let Ok(sink) = Sink::try_new(&handle)
                        {
                            let mut sink_arc = Arc::new(sink);
                            {
                                let mut pb_lock = pb_thread.lock().unwrap();
                                pb_lock.sink = Some(Arc::clone(&sink_arc));
                            }

                            // In riproduzione live usiamo chunk più piccoli per applicare prima i cambi di voce/rate/pitch/volume.
                            let chunks = edge_tts::split_text_realtime_lazy(&text);

                            for chunk in chunks {
                                let mut replay_chunk = true;
                                while replay_chunk {
                                    replay_chunk = false;
                                    // Evita di scaricare e mettere in coda tutto il libro.
                                    // Mantenendo solo 1 blocco in coda, le modifiche agli slider si applicano al chunk successivo.
                                    loop {
                                        {
                                            let mut pb_lock = pb_thread.lock().unwrap();
                                            if pb_lock.status == PlaybackStatus::Stopped {
                                                break;
                                            }
                                            if pb_lock.refresh_requested {
                                                pb_lock.refresh_requested = false;
                                                if let Ok(new_sink) = Sink::try_new(&handle) {
                                                    sink_arc = Arc::new(new_sink);
                                                    pb_lock.sink = Some(Arc::clone(&sink_arc));
                                                }
                                            }
                                        }
                                        if sink_arc.len() < 1 {
                                            break;
                                        }
                                        std::thread::sleep(std::time::Duration::from_millis(100));
                                    }

                                    {
                                        let pb_lock = pb_thread.lock().unwrap();
                                        if pb_lock.status == PlaybackStatus::Stopped {
                                            break;
                                        }
                                    }

                                    // Leggiamo i settaggi freschi per ogni blocco! (Aggiornamento immediato in lettura)
                                    let (voice, rate, pitch, volume) = {
                                        let s = s_thread.lock().unwrap();
                                        (s.voice.clone(), s.rate, s.pitch, s.volume)
                                    };

                                    if let Ok(data) =
                                        rt_thread.block_on(edge_tts::synthesize_text_with_retry(
                                            &chunk, &voice, rate, pitch, volume, 3,
                                        ))
                                        && let Ok(source) = Decoder::new(Cursor::new(data))
                                    {
                                        sink_arc.append(source);
                                    }

                                    loop {
                                        std::thread::sleep(std::time::Duration::from_millis(60));
                                        let mut pb_lock = pb_thread.lock().unwrap();
                                        if pb_lock.status == PlaybackStatus::Stopped {
                                            break;
                                        }
                                        if pb_lock.refresh_requested {
                                            pb_lock.refresh_requested = false;
                                            if let Ok(new_sink) = Sink::try_new(&handle) {
                                                sink_arc = Arc::new(new_sink);
                                                pb_lock.sink = Some(Arc::clone(&sink_arc));
                                            }
                                            replay_chunk = true;
                                            break;
                                        }
                                        if sink_arc.empty() {
                                            break;
                                        }
                                    }
                                }
                            }

                            {
                                let mut pb_lock = pb_thread.lock().unwrap();
                                pb_lock.download_finished = true;
                            }

                            // ATTESA FINE AUDIO
                            loop {
                                std::thread::sleep(std::time::Duration::from_millis(200));
                                let mut pb_lock = pb_thread.lock().unwrap();
                                if pb_lock.status == PlaybackStatus::Stopped {
                                    break;
                                }
                                if sink_arc.empty() && pb_lock.download_finished {
                                    pb_lock.status = PlaybackStatus::Stopped;
                                    pb_lock.sink = None;
                                    break;
                                }
                            }
                        }
                    });
                }
            }
        });

        let play_action_click = Rc::clone(&play_action);
        btn_play.on_click(move |_| {
            play_action_click();
        });
        #[cfg(target_os = "macos")]
        if let Some(item) = play_menu_item {
            let play_action_menu = Rc::clone(&play_action);
            item.on_click(move |_| {
                play_action_menu();
            });
        }

        let podcast_seek_back = Rc::clone(&podcast_playback);
        btn_podcast_back.on_click(move |_| {
            seek_podcast_playback(&podcast_seek_back, -PODCAST_SEEK_SECONDS);
        });

        let podcast_seek_forward = Rc::clone(&podcast_playback);
        btn_podcast_forward.on_click(move |_| {
            seek_podcast_playback(&podcast_seek_forward, PODCAST_SEEK_SECONDS);
        });

        let pb_stop = Arc::clone(&playback);
        let b_p_reset = btn_play;
        let podcast_playback_stop = Rc::clone(&podcast_playback);
        let stop_action: Rc<dyn Fn()> = Rc::new(move || {
            stop_podcast_playback(&podcast_playback_stop);
            let mut pb = pb_stop.lock().unwrap();
            if let Some(ref s) = pb.sink {
                s.stop();
            }
            pb.sink = None;
            pb.status = PlaybackStatus::Stopped;
            pb.refresh_requested = false;
            let podcast_mode = podcast_playback_stop.borrow().selected_episode.is_some();
            b_p_reset.set_label(&play_button_label(PlaybackStatus::Stopped, podcast_mode));
        });

        let stop_action_click = Rc::clone(&stop_action);
        btn_stop.on_click(move |_| {
            stop_action_click();
        });
        #[cfg(target_os = "macos")]
        if let Some(item) = stop_menu_item {
            let stop_action_menu = Rc::clone(&stop_action);
            item.on_click(move |_| {
                stop_action_menu();
            });
        }

        // --- Salva con Progress Bar (Non Bloccante) ---
        let rt_s = Arc::clone(&rt);
        let tc_s = text_ctrl;
        let f_save = frame;
        let s_save = Arc::clone(&settings);
        let save_action: Rc<dyn Fn()> = Rc::new(move || {
            let text = tc_s.get_value();
            if text.trim().is_empty() {
                return;
            }

            let (voice, rate, pitch, volume) = {
                let s = s_save.lock().unwrap();
                (s.voice.clone(), s.rate, s.pitch, s.volume)
            };

            let dialog = FileDialog::builder(&f_save)
                .with_message("Salva audiolibro")
                .with_wildcard("File MP3 (*.mp3)|*.mp3")
                .with_style(FileDialogStyle::Save | FileDialogStyle::OverwritePrompt)
                .build();

            if dialog.show_modal() == ID_OK
                && let Some(path) = dialog.get_path()
            {
                append_podcast_log(&format!("audiobook_save.begin path={path}"));
                let chunks: Vec<String> = edge_tts::split_text_lazy(&text).collect();
                let total = chunks.len();
                append_podcast_log(&format!("audiobook_save.chunks total={total}"));

                let progress_dialog = Dialog::builder(&f_save, "Creazione Audiolibro")
                    .with_style(
                        DialogStyle::Caption
                            | DialogStyle::SystemMenu
                            | DialogStyle::CloseBox
                            | DialogStyle::StayOnTop,
                    )
                    .with_size(420, 160)
                    .build();
                let progress_panel = Panel::builder(&progress_dialog).build();
                let progress_root = BoxSizer::builder(Orientation::Vertical).build();
                let progress_label = StaticText::builder(&progress_panel)
                    .with_label("Inizializzazione...")
                    .build();
                progress_root.add(
                    &progress_label,
                    0,
                    SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
                    12,
                );
                let progress_gauge = Gauge::builder(&progress_panel)
                    .with_range(total.max(1) as i32)
                    .build();
                progress_root.add(
                    &progress_gauge,
                    0,
                    SizerFlag::Expand | SizerFlag::Left | SizerFlag::Right | SizerFlag::Top,
                    12,
                );
                let progress_buttons = BoxSizer::builder(Orientation::Horizontal).build();
                let progress_cancel = Button::builder(&progress_panel)
                    .with_id(ID_CANCEL)
                    .with_label("Annulla")
                    .build();
                progress_buttons.add_spacer(1);
                progress_buttons.add(&progress_cancel, 0, SizerFlag::All, 10);
                progress_root.add_sizer(
                    &progress_buttons,
                    0,
                    SizerFlag::Expand | SizerFlag::Bottom,
                    0,
                );
                progress_panel.set_sizer(progress_root, true);
                progress_dialog.set_escape_id(ID_CANCEL);
                progress_dialog.show(true);

                let rt_save = Arc::clone(&rt_s);
                let path_buf = PathBuf::from(path);
                let abort_requested = Arc::new(AtomicBool::new(false));
                let abort_requested_thread = Arc::clone(&abort_requested);
                let save_state = Arc::new(Mutex::new(SaveAudiobookState {
                    completed_chunks: 0,
                    completed: false,
                    cancelled: false,
                    error_message: None,
                }));
                let save_state_thread = Arc::clone(&save_state);
                let chunks = Arc::new(chunks);
                std::thread::spawn(move || {
                    let next_index = Arc::new(Mutex::new(0usize));
                    let results = Arc::new(Mutex::new(vec![None; chunks.len()]));
                    let worker_count = chunks.len().clamp(1, AUDIOBOOK_SAVE_THREADS);
                    let mut workers = Vec::with_capacity(worker_count);

                    for _ in 0..worker_count {
                        let rt_worker = Arc::clone(&rt_save);
                        let chunks_worker = Arc::clone(&chunks);
                        let next_index_worker = Arc::clone(&next_index);
                        let results_worker = Arc::clone(&results);
                        let save_state_worker = Arc::clone(&save_state_thread);
                        let abort_worker = Arc::clone(&abort_requested_thread);
                        let voice_worker = voice.clone();
                        workers.push(std::thread::spawn(move || {
                            loop {
                                if abort_worker.load(Ordering::Relaxed) {
                                    return;
                                }

                                let index = {
                                    let mut next = next_index_worker.lock().unwrap();
                                    if *next >= chunks_worker.len() {
                                        return;
                                    }
                                    let index = *next;
                                    *next += 1;
                                    index
                                };

                                let chunk = chunks_worker[index].clone();
                                match rt_worker.block_on(edge_tts::synthesize_text_with_retry(
                                    &chunk,
                                    &voice_worker,
                                    rate,
                                    pitch,
                                    volume,
                                    3,
                                )) {
                                    Ok(data) => {
                                        results_worker.lock().unwrap()[index] = Some(data);
                                        save_state_worker.lock().unwrap().completed_chunks += 1;
                                    }
                                    Err(_) => {
                                        abort_worker.store(true, Ordering::Relaxed);
                                        save_state_worker.lock().unwrap().error_message = Some(
                                            "La conversione dell'audiolibro non è riuscita."
                                                .to_string(),
                                        );
                                        return;
                                    }
                                }
                            }
                        }));
                    }

                    for worker in workers {
                        if worker.join().is_err() {
                            abort_requested_thread.store(true, Ordering::Relaxed);
                            save_state_thread.lock().unwrap().error_message =
                                Some("La conversione dell'audiolibro non è riuscita.".to_string());
                            append_podcast_log("audiobook_save.worker_join_failed");
                            return;
                        }
                    }

                    if abort_requested_thread.load(Ordering::Relaxed) {
                        save_state_thread.lock().unwrap().cancelled = true;
                        append_podcast_log("audiobook_save.cancelled");
                        return;
                    }

                    let mut full_audio = Vec::new();
                    for maybe_data in results.lock().unwrap().iter_mut() {
                        let Some(data) = maybe_data.take() else {
                            save_state_thread.lock().unwrap().error_message =
                                Some("La conversione dell'audiolibro non è riuscita.".to_string());
                            return;
                        };
                        full_audio.extend(data);
                    }

                    if std::fs::write(&path_buf, full_audio).is_err() {
                        save_state_thread.lock().unwrap().error_message = Some(
                            "Il file audiolibro non è stato salvato correttamente.".to_string(),
                        );
                        append_podcast_log("audiobook_save.write_failed");
                        return;
                    }

                    save_state_thread.lock().unwrap().completed = true;
                    append_podcast_log("audiobook_save.completed");
                });

                let progress_timer = Rc::new(Timer::new(&f_save));
                let progress_timer_tick = Rc::clone(&progress_timer);
                let progress_timer_handle = Rc::clone(&progress_timer);
                let pending_dialog = Rc::new(RefCell::new(None::<PendingSaveDialog>));
                let pending_dialog_tick = Rc::clone(&pending_dialog);
                let progress_dialog_handle = progress_dialog;
                let progress_dialog_close = progress_dialog;
                let progress_dialog_destroy = progress_dialog;
                let progress_label_tick = progress_label;
                let progress_label_cancel = progress_label;
                let progress_label_close = progress_label;
                let progress_gauge_tick = progress_gauge;
                let progress_cancel_close = progress_cancel;
                let abort_close = Arc::clone(&abort_requested);
                let save_state_tick = Arc::clone(&save_state);
                let save_state_close = Arc::clone(&save_state);
                let cancel_pending = Rc::new(RefCell::new(false));
                let cancel_pending_tick = Rc::clone(&cancel_pending);
                let cancel_pending_close = Rc::clone(&cancel_pending);
                let finalizing = Rc::new(RefCell::new(false));
                let finalizing_tick = Rc::clone(&finalizing);
                progress_cancel.on_click(move |_| {
                    if !*cancel_pending.borrow() {
                        append_podcast_log("audiobook_save.cancel_requested_button");
                        abort_requested.store(true, Ordering::Relaxed);
                        *cancel_pending.borrow_mut() = true;
                        progress_cancel.enable(false);
                        progress_label_cancel.set_label("Annullamento audiolibro in corso...");
                    }
                });
                progress_dialog_close.on_close(move |event| {
                    append_podcast_log("audiobook_save.progress_dialog.on_close");
                    let state = save_state_close.lock().unwrap();
                    let finished =
                        state.completed || state.cancelled || state.error_message.is_some();
                    drop(state);

                    if finished {
                        append_podcast_log("audiobook_save.progress_dialog.on_close.finished");
                        event.skip(true);
                        return;
                    }

                    if !*cancel_pending_close.borrow() {
                        append_podcast_log("audiobook_save.cancel_requested_close");
                        abort_close.store(true, Ordering::Relaxed);
                        *cancel_pending_close.borrow_mut() = true;
                        progress_cancel_close.enable(false);
                        progress_label_close.set_label("Annullamento audiolibro in corso...");
                    }

                    event.skip(false);
                });
                let timer_destroy = Rc::clone(&progress_timer);
                progress_dialog_destroy.on_destroy(move |event| {
                    append_podcast_log("audiobook_save.progress_dialog.on_destroy");
                    timer_destroy.stop();
                    event.skip(true);
                });
                progress_timer_tick.on_tick(move |_| {
                    if *finalizing_tick.borrow() {
                        return;
                    }

                    let state = save_state_tick.lock().unwrap();
                    if let Some(error_message) = state.error_message.as_ref() {
                        *finalizing_tick.borrow_mut() = true;
                        append_podcast_log(&format!(
                            "audiobook_save.tick.error completed_chunks={} message={error_message}",
                            state.completed_chunks
                        ));
                        progress_timer_handle.stop();
                        progress_label_tick.set_label("Errore durante la conversione.");
                        progress_gauge_tick.set_value(state.completed_chunks as i32);
                        *pending_dialog_tick.borrow_mut() =
                            Some(PendingSaveDialog::Error(error_message.clone()));
                        append_podcast_log("audiobook_save.tick.error.destroy_progress");
                        progress_dialog_handle.destroy();
                        let Some(dialog) = pending_dialog_tick.borrow_mut().take() else {
                            return;
                        };
                        match dialog {
                            PendingSaveDialog::Success => {}
                            PendingSaveDialog::Error(error_message) => {
                                append_podcast_log(&format!(
                                    "audiobook_save.show_error message={error_message}"
                                ));
                                show_modeless_message_dialog(
                                    &f_save,
                                    "Errore conversione",
                                    &error_message,
                                );
                                append_podcast_log("audiobook_save.error_closed");
                            }
                        }
                        return;
                    }

                    if state.cancelled {
                        *finalizing_tick.borrow_mut() = true;
                        append_podcast_log(&format!(
                            "audiobook_save.tick.cancelled completed_chunks={}",
                            state.completed_chunks
                        ));
                        progress_timer_handle.stop();
                        progress_dialog_handle.destroy();
                        return;
                    }

                    if state.completed {
                        *finalizing_tick.borrow_mut() = true;
                        append_podcast_log(&format!(
                            "audiobook_save.tick.completed completed_chunks={}",
                            state.completed_chunks
                        ));
                        progress_label_tick.set_label("Audiolibro salvato correttamente.");
                        progress_gauge_tick.set_value(total.max(1) as i32);
                        progress_timer_handle.stop();
                        *pending_dialog_tick.borrow_mut() = Some(PendingSaveDialog::Success);
                        append_podcast_log("audiobook_save.tick.completed.destroy_progress");
                        progress_dialog_handle.destroy();
                        let Some(dialog) = pending_dialog_tick.borrow_mut().take() else {
                            return;
                        };
                        match dialog {
                            PendingSaveDialog::Success => {
                                append_podcast_log("audiobook_save.show_success");
                                show_modeless_message_dialog(
                                    &f_save,
                                    "Salvataggio completato",
                                    "Audiolibro salvato correttamente.",
                                );
                                append_podcast_log("audiobook_save.success_closed");
                            }
                            PendingSaveDialog::Error(_) => {}
                        }
                        return;
                    }

                    let current = state.completed_chunks as i32;
                    drop(state);

                    if *cancel_pending_tick.borrow() {
                        append_podcast_log(&format!(
                            "audiobook_save.tick.cancelling completed_chunks={current}"
                        ));
                        progress_label_tick.set_label("Annullamento audiolibro in corso...");
                        progress_gauge_tick.set_value(current);
                        return;
                    }

                    let current_display = current.min(total.max(1) as i32);
                    let msg = format!("Sintesi blocco {} di {}...", current, total);
                    progress_label_tick.set_label(&msg);
                    progress_gauge_tick.set_value(current_display);
                });
                progress_timer.start(100, false);
            }
        });

        let save_action_click = Rc::clone(&save_action);
        btn_save.on_click(move |_| {
            save_action_click();
        });
        #[cfg(target_os = "macos")]
        if let Some(item) = save_menu_item {
            let save_action_menu = Rc::clone(&save_action);
            item.on_click(move |_| {
                save_action_menu();
            });
        }

        let frame_settings = frame;
        let settings_state = Arc::clone(&settings);
        let voices_state = Arc::clone(&voices_data);
        let languages_state = Arc::clone(&languages);
        let playback_state = Arc::clone(&playback);
        let settings_action: Rc<dyn Fn()> = Rc::new(move || {
            open_settings_dialog(
                &frame_settings,
                &settings_state,
                &voices_state,
                &languages_state,
                &playback_state,
            );
        });

        let settings_action_click = Rc::clone(&settings_action);
        btn_settings.on_click(move |_| {
            settings_action_click();
        });
        #[cfg(target_os = "macos")]
        if let Some(item) = settings_menu_item {
            let settings_action_menu = Rc::clone(&settings_action);
            item.on_click(move |_| {
                settings_action_menu();
            });
        }

        #[cfg(target_os = "macos")]
        {
            let play_action_menu = Rc::clone(&play_action);
            let stop_action_menu = Rc::clone(&stop_action);
            let save_action_menu = Rc::clone(&save_action);
            let settings_action_menu = Rc::clone(&settings_action);
            frame.on_menu(move |event| match event.get_id() {
                ID_PLAY_PAUSE => play_action_menu(),
                ID_STOP => stop_action_menu(),
                ID_SAVE => save_action_menu(),
                ID_SETTINGS => settings_action_menu(),
                _ => {}
            });
        }

        #[cfg(target_os = "macos")]
        {
            let play_action_shortcut = Rc::clone(&play_action);
            let stop_action_shortcut = Rc::clone(&stop_action);
            let save_action_shortcut = Rc::clone(&save_action);
            let settings_action_shortcut = Rc::clone(&settings_action);
            let podcast_seek_back_shortcut = Rc::clone(&podcast_playback);
            let podcast_seek_forward_shortcut = Rc::clone(&podcast_playback);
            frame.on_key_down(move |event| {
                handle_shortcut_event(
                    event,
                    &play_action_shortcut,
                    &stop_action_shortcut,
                    &save_action_shortcut,
                    &settings_action_shortcut,
                    &podcast_seek_back_shortcut,
                    &podcast_seek_forward_shortcut,
                );
            });
        }

        #[cfg(target_os = "macos")]
        {
            let play_action_shortcut = Rc::clone(&play_action);
            let stop_action_shortcut = Rc::clone(&stop_action);
            let save_action_shortcut = Rc::clone(&save_action);
            let settings_action_shortcut = Rc::clone(&settings_action);
            let podcast_seek_back_shortcut = Rc::clone(&podcast_playback);
            let podcast_seek_forward_shortcut = Rc::clone(&podcast_playback);
            text_ctrl.on_key_down(move |event| {
                handle_shortcut_event(
                    event,
                    &play_action_shortcut,
                    &stop_action_shortcut,
                    &save_action_shortcut,
                    &settings_action_shortcut,
                    &podcast_seek_back_shortcut,
                    &podcast_seek_forward_shortcut,
                );
            });
        }

        #[cfg(not(target_os = "macos"))]
        {
            let play_action_shortcut = Rc::clone(&play_action);
            let stop_action_shortcut = Rc::clone(&stop_action);
            let save_action_shortcut = Rc::clone(&save_action);
            let settings_action_shortcut = Rc::clone(&settings_action);
            let podcast_seek_back_shortcut = Rc::clone(&podcast_playback);
            let podcast_seek_forward_shortcut = Rc::clone(&podcast_playback);
            text_ctrl.on_key_down(move |event| {
                handle_shortcut_event(
                    event,
                    &play_action_shortcut,
                    &stop_action_shortcut,
                    &save_action_shortcut,
                    &settings_action_shortcut,
                    &podcast_seek_back_shortcut,
                    &podcast_seek_forward_shortcut,
                );
            });
        }

        frame.show(true);
        frame.centre();
    });
}
