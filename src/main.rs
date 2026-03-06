#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod edge_tts;
mod file_loader;

use rodio::{Decoder, OutputStream, Sink};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::runtime::Runtime;
use wxdragon::prelude::*;
use wxdragon::timer::Timer;

const ID_OPEN: i32 = 101;
const ID_EXIT: i32 = 102;
const ID_PLAY_PAUSE: i32 = 2001;
const ID_STOP: i32 = 2003;
const ID_SAVE: i32 = 2002;

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

#[derive(Serialize, Deserialize, Clone)]
struct Settings {
    language: String,
    voice: String,
    rate: i32,
    pitch: i32,
    volume: i32,
}

impl Settings {
    fn load() -> Self {
        if let Ok(data) = std::fs::read_to_string("settings.json")
            && let Ok(settings) = serde_json::from_str(&data)
        {
            return settings;
        }
        Settings {
            language: "Italiano".to_string(),
            voice: "".to_string(),
            rate: 0,
            pitch: 0,
            volume: 100,
        }
    }

    fn save(&self) {
        if let Ok(data) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write("settings.json", data);
        }
    }
}

fn get_language_name(locale: &str) -> String {
    let base = locale.split('-').next().unwrap_or(locale).to_lowercase();
    match base.as_str() {
        "it" => "Italiano".to_string(),
        "en" => "Inglese".to_string(),
        "fr" => "Francese".to_string(),
        "es" => "Spagnolo".to_string(),
        "de" => "Tedesco".to_string(),
        "pt" => "Portoghese".to_string(),
        "pl" => "Polacco".to_string(),
        "ru" => "Russo".to_string(),
        "zh" => "Cinese".to_string(),
        "ja" => "Giapponese".to_string(),
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

fn main() {
    #[cfg(windows)]
    SystemOptions::set_option_by_int("msw.no-manifest-check", 1);

    let rt = Arc::new(Runtime::new().unwrap());
    let voices_data = Arc::new(Mutex::new(Vec::<edge_tts::VoiceInfo>::new()));
    let languages = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let filtered_voices = Arc::new(Mutex::new(Vec::<edge_tts::VoiceInfo>::new()));

    let settings = Arc::new(Mutex::new(Settings::load()));

    let playback = Arc::new(Mutex::new(GlobalPlayback {
        sink: None,
        status: PlaybackStatus::Stopped,
        download_finished: false,
        refresh_requested: false,
    }));

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
        file_menu.append(
            ID_EXIT,
            "&Esci\tAlt+F4",
            "Esci dal programma",
            ItemKind::Normal,
        );

        let menubar = MenuBar::builder().append(file_menu, "&File").build();
        frame.set_menu_bar(menubar);

        let panel = Panel::builder(&frame).build();
        let main_sizer = BoxSizer::builder(Orientation::Vertical).build();

        let text_ctrl = TextCtrl::builder(&panel)
            .with_style(TextCtrlStyle::MultiLine)
            .build();
        main_sizer.add(&text_ctrl, 1, SizerFlag::Expand | SizerFlag::All, 5);

        let filter_sizer = BoxSizer::builder(Orientation::Horizontal).build();
        filter_sizer.add(
            &StaticText::builder(&panel).with_label("Lingua:").build(),
            0,
            SizerFlag::AlignCenterVertical | SizerFlag::All,
            5,
        );
        let choice_lang = Choice::builder(&panel).build();
        filter_sizer.add(&choice_lang, 1, SizerFlag::Expand | SizerFlag::All, 5);

        filter_sizer.add(
            &StaticText::builder(&panel).with_label("Voce:").build(),
            0,
            SizerFlag::AlignCenterVertical | SizerFlag::All,
            5,
        );
        let choice_voices = Choice::builder(&panel).build();
        filter_sizer.add(&choice_voices, 1, SizerFlag::Expand | SizerFlag::All, 5);

        main_sizer.add_sizer(&filter_sizer, 0, SizerFlag::Expand, 0);

        // Presets accessibili (Rate, Pitch, Volume)
        let slider_sizer = BoxSizer::builder(Orientation::Horizontal).build();

        slider_sizer.add(
            &StaticText::builder(&panel).with_label("Velocità:").build(),
            0,
            SizerFlag::AlignCenterVertical | SizerFlag::All,
            5,
        );
        let choice_rate = Choice::builder(&panel).build();
        for (label, _) in RATE_PRESETS {
            choice_rate.append(label);
        }
        let initial_rate = settings.lock().unwrap().rate;
        choice_rate.set_selection(nearest_preset_index(&RATE_PRESETS, initial_rate) as u32);
        slider_sizer.add(&choice_rate, 1, SizerFlag::Expand | SizerFlag::All, 5);

        slider_sizer.add(
            &StaticText::builder(&panel).with_label("Tono:").build(),
            0,
            SizerFlag::AlignCenterVertical | SizerFlag::All,
            5,
        );
        let choice_pitch = Choice::builder(&panel).build();
        for (label, _) in PITCH_PRESETS {
            choice_pitch.append(label);
        }
        let initial_pitch = settings.lock().unwrap().pitch;
        choice_pitch.set_selection(nearest_preset_index(&PITCH_PRESETS, initial_pitch) as u32);
        slider_sizer.add(&choice_pitch, 1, SizerFlag::Expand | SizerFlag::All, 5);

        slider_sizer.add(
            &StaticText::builder(&panel).with_label("Volume:").build(),
            0,
            SizerFlag::AlignCenterVertical | SizerFlag::All,
            5,
        );
        let choice_volume = Choice::builder(&panel).build();
        for (label, _) in VOLUME_PRESETS {
            choice_volume.append(label);
        }
        let initial_volume = settings.lock().unwrap().volume;
        choice_volume.set_selection(nearest_preset_index(&VOLUME_PRESETS, initial_volume) as u32);
        slider_sizer.add(&choice_volume, 1, SizerFlag::Expand | SizerFlag::All, 5);

        main_sizer.add_sizer(&slider_sizer, 0, SizerFlag::Expand, 0);

        let btn_sizer = BoxSizer::builder(Orientation::Horizontal).build();
        let btn_play = Button::builder(&panel)
            .with_id(ID_PLAY_PAUSE)
            .with_label("Avvia Lettura")
            .build();
        btn_sizer.add(&btn_play, 1, SizerFlag::All, 10);
        let btn_stop = Button::builder(&panel)
            .with_id(ID_STOP)
            .with_label("Ferma Lettura")
            .build();
        btn_sizer.add(&btn_stop, 1, SizerFlag::All, 10);
        let btn_save = Button::builder(&panel)
            .with_id(ID_SAVE)
            .with_label("Salva Audiolibro (MP3)")
            .build();
        btn_sizer.add(&btn_save, 1, SizerFlag::All, 10);

        main_sizer.add_sizer(&btn_sizer, 0, SizerFlag::Expand, 0);
        panel.set_sizer(main_sizer, true);

        // --- Eventi Presets ---
        let s_rate = Arc::clone(&settings);
        let pb_rate = Arc::clone(&playback);
        let c_rate_evt = choice_rate;
        choice_rate.on_selection_changed(move |_| {
            if let Some(sel) = c_rate_evt.get_selection() {
                let mut s = s_rate.lock().unwrap();
                s.rate = RATE_PRESETS[sel as usize].1;
                s.save();

                let mut pb = pb_rate.lock().unwrap();
                if pb.status == PlaybackStatus::Playing {
                    pb.refresh_requested = true;
                    if let Some(ref sink) = pb.sink {
                        sink.stop();
                    }
                }
            }
        });

        let s_pitch = Arc::clone(&settings);
        let pb_pitch = Arc::clone(&playback);
        let c_pitch_evt = choice_pitch;
        choice_pitch.on_selection_changed(move |_| {
            if let Some(sel) = c_pitch_evt.get_selection() {
                let mut s = s_pitch.lock().unwrap();
                s.pitch = PITCH_PRESETS[sel as usize].1;
                s.save();

                let mut pb = pb_pitch.lock().unwrap();
                if pb.status == PlaybackStatus::Playing {
                    pb.refresh_requested = true;
                    if let Some(ref sink) = pb.sink {
                        sink.stop();
                    }
                }
            }
        });

        let s_volume = Arc::clone(&settings);
        let pb_volume = Arc::clone(&playback);
        let c_volume_evt = choice_volume;
        choice_volume.on_selection_changed(move |_| {
            if let Some(sel) = c_volume_evt.get_selection() {
                let mut s = s_volume.lock().unwrap();
                s.volume = VOLUME_PRESETS[sel as usize].1;
                s.save();

                let mut pb = pb_volume.lock().unwrap();
                if pb.status == PlaybackStatus::Playing {
                    pb.refresh_requested = true;
                    if let Some(ref sink) = pb.sink {
                        sink.stop();
                    }
                }
            }
        });

        // --- Timer per aggiornamento UI ---
        let timer = Box::leak(Box::new(Timer::new(&frame)));
        let pb_timer = Arc::clone(&playback);
        let btn_play_timer = btn_play;

        timer.on_tick(move |_| {
            let pb = pb_timer.lock().unwrap();
            if pb.status == PlaybackStatus::Stopped {
                if btn_play_timer.get_label() != "Avvia Lettura" {
                    btn_play_timer.set_label("Avvia Lettura");
                }
            } else if pb.status == PlaybackStatus::Paused {
                if btn_play_timer.get_label() != "Riprendi Lettura" {
                    btn_play_timer.set_label("Riprendi Lettura");
                }
            } else if pb.status == PlaybackStatus::Playing
                && btn_play_timer.get_label() != "Pausa Lettura"
            {
                btn_play_timer.set_label("Pausa Lettura");
            }
        });
        timer.start(200, false);

        // --- Caricamento Voci ---
        let rt_init = Arc::clone(&rt);
        let voices_init = Arc::clone(&voices_data);
        let langs_init = Arc::clone(&languages);
        let choice_lang_init = choice_lang;
        let s_init = Arc::clone(&settings);

        match rt_init.block_on(edge_tts::get_edge_voices()) {
            Ok(v_list) => {
                let mut v_lock = voices_init.lock().unwrap();
                *v_lock = v_list.clone();
                let mut l_map = BTreeMap::new();
                for v in &v_list {
                    l_map.insert(get_language_name(&v.locale), v.locale.clone());
                }
                let mut l_lock = langs_init.lock().unwrap();
                *l_lock = l_map.into_iter().collect();
                for (name, _) in &*l_lock {
                    choice_lang_init.append(name);
                }

                let saved_lang = s_init.lock().unwrap().language.clone();
                if let Some(pos) = l_lock.iter().position(|(n, _)| n == &saved_lang) {
                    choice_lang_init.set_selection(pos as u32);
                } else if let Some(pos) = l_lock.iter().position(|(n, _)| n == "Italiano") {
                    choice_lang_init.set_selection(pos as u32);
                } else if !l_lock.is_empty() {
                    choice_lang_init.set_selection(0);
                }
            }
            Err(e) => println!("ERROR: Caricamento voci fallito: {}", e),
        }

        let v_filter = Arc::clone(&voices_data);
        let l_filter = Arc::clone(&languages);
        let f_init = Arc::clone(&filtered_voices);
        let c_lang_ev = choice_lang;
        let _c_voices_lc = choice_voices;
        let s_update = Arc::clone(&settings);

        let update_voices = move |cl: &Choice, cv: &Choice| {
            if let Some(sel_idx) = cl.get_selection() {
                let locale = l_filter.lock().unwrap()[sel_idx as usize].1.clone();
                let lang_name = l_filter.lock().unwrap()[sel_idx as usize].0.clone();
                let v_list: Vec<_> = v_filter
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|v| v.locale == locale)
                    .cloned()
                    .collect();
                cv.clear();
                for v in &v_list {
                    cv.append(&v.friendly_name);
                }

                let mut s = s_update.lock().unwrap();
                s.language = lang_name;

                if let Some(pos) = v_list.iter().position(|v| v.short_name == s.voice) {
                    cv.set_selection(pos as u32);
                } else if !v_list.is_empty() {
                    cv.set_selection(0);
                    s.voice = v_list[0].short_name.clone();
                }
                s.save();
                *f_init.lock().unwrap() = v_list;
            }
        };

        update_voices(&choice_lang, &choice_voices);

        let uv_arc = Arc::new(update_voices);
        let uv_clone = Arc::clone(&uv_arc);
        let cv_lc2 = choice_voices;
        choice_lang.on_selection_changed(move |_| {
            uv_clone(&c_lang_ev, &cv_lc2);
        });

        let s_voice = Arc::clone(&settings);
        let f_voice = Arc::clone(&filtered_voices);
        let cv_voice = choice_voices;
        choice_voices.on_selection_changed(move |_| {
            if let Some(sel) = cv_voice.get_selection() {
                let voice = f_voice.lock().unwrap()[sel as usize].short_name.clone();
                let mut s = s_voice.lock().unwrap();
                s.voice = voice;
                s.save();
            }
        });

        // --- Menu ---
        let f_menu = frame;
        let tc_menu = text_ctrl;
        frame.on_menu(move |event| {
            if event.get_id() == ID_OPEN {
                let dialog = FileDialog::builder(&f_menu).with_message("Apri").with_wildcard("Supportati|*.txt;*.docx;*.pdf;*.epub;*.xlsx;*.xls;*.ods;*.html;*.htm|Tutti|*.*").build();
                if dialog.show_modal() == ID_OK
                    && let Some(path) = dialog.get_path()
                        && let Ok(c) = file_loader::load_any_file(Path::new(&path)) { tc_menu.set_value(&c); }
            } else if event.get_id() == ID_EXIT { f_menu.close(true); }
        });

        // --- Play / Pausa / Stop ---
        let rt_p = Arc::clone(&rt);
        let pb_p = Arc::clone(&playback);
        let tc_p = text_ctrl;
        let b_p_label = btn_play;
        let s_play = Arc::clone(&settings);

        btn_play.on_click(move |_| {
            let mut pb = pb_p.lock().unwrap();
            match pb.status {
                PlaybackStatus::Playing => {
                    if let Some(ref s) = pb.sink {
                        s.pause();
                        pb.status = PlaybackStatus::Paused;
                        b_p_label.set_label("Riprendi Lettura");
                    }
                }
                PlaybackStatus::Paused => {
                    if let Some(ref s) = pb.sink {
                        s.play();
                        pb.status = PlaybackStatus::Playing;
                        b_p_label.set_label("Pausa Lettura");
                    }
                }
                PlaybackStatus::Stopped => {
                    let text = tc_p.get_value();
                    if text.trim().is_empty() {
                        return;
                    }

                    b_p_label.set_label("Pausa Lettura");
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

        let pb_stop = Arc::clone(&playback);
        let b_p_reset = btn_play;
        btn_stop.on_click(move |_| {
            let mut pb = pb_stop.lock().unwrap();
            if let Some(ref s) = pb.sink {
                s.stop();
            }
            pb.sink = None;
            pb.status = PlaybackStatus::Stopped;
            pb.refresh_requested = false;
            b_p_reset.set_label("Avvia Lettura");
        });

        // --- Salva con Progress Bar (Non Bloccante) ---
        let rt_s = Arc::clone(&rt);
        let tc_s = text_ctrl;
        let f_save = frame;
        let s_save = Arc::clone(&settings);

        btn_save.on_click(move |_| {
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
                let chunks: Vec<String> = edge_tts::split_text_lazy(&text).collect();
                let total = chunks.len();

                let progress = ProgressDialog::builder(
                    &f_save,
                    "Creazione Audiolibro",
                    "Inizializzazione...",
                    total as i32,
                )
                .with_style(
                    ProgressDialogStyle::CanAbort
                        | ProgressDialogStyle::AutoHide
                        | ProgressDialogStyle::Smooth,
                )
                .build();

                let rt_save = Arc::clone(&rt_s);
                let path_buf = PathBuf::from(path);

                let progress_state = Arc::new(Mutex::new((0, false, false))); // (corrente, completato, abortito)
                let ps_thread = Arc::clone(&progress_state);

                std::thread::spawn(move || {
                    let mut full_audio = Vec::new();
                    for (i, chunk) in chunks.into_iter().enumerate() {
                        if ps_thread.lock().unwrap().2 {
                            return;
                        }
                        if let Ok(data) = rt_save.block_on(edge_tts::synthesize_text_with_retry(
                            &chunk, &voice, rate, pitch, volume, 3,
                        )) {
                            full_audio.extend(data);
                            ps_thread.lock().unwrap().0 = i + 1;
                        } else {
                            ps_thread.lock().unwrap().2 = true;
                            return;
                        }
                    }
                    let _ = std::fs::write(&path_buf, full_audio);
                    ps_thread.lock().unwrap().1 = true;
                });

                let mut save_completed = false;
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(100));

                    let (curr, done, err) = {
                        let ps = progress_state.lock().unwrap();
                        (ps.0, ps.1, ps.2)
                    };

                    if err {
                        break;
                    }
                    if done {
                        progress.update(total as i32, Some("Audiolibro salvato correttamente."));
                        save_completed = true;
                        break;
                    }

                    let msg = format!("Sintesi blocco {} di {}...", curr, total);
                    if !progress.update(curr as i32, Some(&msg)) {
                        progress_state.lock().unwrap().2 = true;
                        break;
                    }
                }

                if save_completed {
                    let done_dialog = MessageDialog::builder(
                        &f_save,
                        "Audiolibro salvato correttamente.",
                        "Salvataggio completato",
                    )
                    .with_style(MessageDialogStyle::OK | MessageDialogStyle::IconInformation)
                    .build();
                    done_dialog.show_modal();
                }
            }
        });

        frame.show(true);
        frame.centre();
    });
}
