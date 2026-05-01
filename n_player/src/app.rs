use crate::localization::{get_locale_denominator, localize};
use crate::runner::{run, RunnerMessage, RunnerSeek};
use crate::{
    add_all_tracks_to_player, bus_server, get_image_squared, AppData, FileTrack, Localization,
    MainWindow, SettingsData, Theme, TrackData, WindowSize,
};
use flume::{Receiver, Sender};
use n_audio::music_track::MusicTrack;
use n_audio::queue::QueuePlayer;
use n_audio::remove_ext;
use pollster::FutureExt;
use slint::{ComponentHandle, Model, VecModel, Weak};
use std::collections::HashMap;
use std::ops::DerefMut;
use std::sync::Arc;
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::sync::{Mutex, RwLock};

pub type Runner = Arc<RwLock<crate::runner::Runner>>;
pub type Settings = Arc<RwLock<crate::settings::Settings>>;
#[allow(type_alias_bounds)]
pub type Platform<P: crate::platform::Platform + Send + 'static> = Arc<RwLock<P>>;

enum Changes {
    Tracks(Vec<TrackData>),
    Metadata(usize, TrackData),
}

pub async fn run_app<P: crate::platform::Platform + Send + 'static + Sync>(
    settings: crate::settings::Settings,
    platform: P,
) {
    let platform = Arc::new(RwLock::new(platform));
    let settings = Arc::new(RwLock::new(settings));

    let p = platform.clone();
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default_panic(info);
        p.write().block_on().set_clipboard_text(info.to_string());
        std::process::exit(1);
    }));

    let tmp = NamedTempFile::new().unwrap();
    let (tx, rx) = flume::unbounded();

    let player = QueuePlayer::new(settings.read().await.path.clone());

    let runner = Arc::new(RwLock::new(crate::runner::Runner::new(player)));

    let r = runner.clone();
    let tx_t = tx.clone();

    let (tx_l, rx_l) = flume::unbounded();
    #[cfg(target_os = "linux")]
    let _ = slint::set_xdg_app_id("n_music");
    let main_window = MainWindow::new().unwrap();

    let p = platform.clone();
    p.write().await.add_runner(r.clone(), tx_t.clone()).await;
    let (tx_path, rx_path) = flume::unbounded();
    let settings_path = settings.read().await.path.clone();
    let check_cache = !settings_path.is_empty();

    tx_path
        .send_async((settings_path, check_cache))
        .await
        .unwrap();
    let (tx_tracks, rx_tracks) = flume::unbounded();
    let s = settings.clone();
    let future = tokio::spawn(async move {
        let runner_future = tokio::task::spawn(run(r.clone(), rx));
        let bus_future = tokio::task::spawn(bus_server::run(p.clone(), r.clone(), tmp));
        let loader_future = tokio::task::spawn(loader(r.clone(), s, p, tx_l, rx_path, tx_tracks));

        let _ = tokio::join!(runner_future, bus_future, loader_future);
    });

    let (tx_searching, rx_searching) = flume::unbounded();
    let (tx_changing, rx_changing) = flume::unbounded();
    let (tx_wake, rx_wake) = flume::unbounded::<()>();
    let (tx_vis, rx_vis) = flume::unbounded::<bool>();
    let p = platform.clone();
    p.write().await.set_visibility_sender(tx_vis).await;

    setup_data(
        settings.clone(),
        platform.clone(),
        main_window.clone_strong(),
        tx.clone(),
        tx_searching,
        tx_changing,
        tx_path,
        tx_wake.clone(),
    )
    .await;

    let window = main_window.as_weak();
    let r = runner.clone();
    let s = settings.clone();
    let p = platform.clone();
    let updater = tokio::task::spawn(updater_task(
        r,
        s,
        p,
        window,
        rx_tracks,
        rx_changing,
        rx_searching,
        rx_l,
        rx_wake.clone(),
        rx_vis,
    ));

    tokio::task::block_in_place(|| main_window.run().unwrap());

    updater.abort();
    future.abort();

    settings.write().await.volume = runner.read().await.volume();
    if settings.read().await.save_window_size {
        let width = main_window.get_last_width() as usize;
        let height = main_window.get_last_height() as usize;
        settings.write().await.window_size = WindowSize { width, height };
    } else {
        settings.write().await.window_size = WindowSize::default();
    }
    settings.read().await.save(platform.read().await).await;
}

async fn setup_data<P: crate::platform::Platform + Send + 'static + Sync>(
    settings: Settings,
    platform: Platform<P>,
    main_window: MainWindow,
    tx: Sender<RunnerMessage>,
    tx_searching: Sender<String>,
    tx_changing: Sender<()>,
    tx_path: Sender<(String, bool)>,
    tx_wake: Sender<()>,
) {
    localize(
        settings.read().await.locale.clone(),
        main_window.global::<Localization>(),
    );

    let settings_data = main_window.global::<SettingsData>();
    let app_data = main_window.global::<AppData>();

    #[cfg(target_os = "android")]
    app_data.set_android(true);
    app_data.set_version(env!("CARGO_PKG_VERSION").into());

    {
        let settings = settings.read().await;
        settings_data.set_color_scheme(settings.theme.into());
        settings_data.set_theme(i32::from(settings.theme));
        settings_data.set_width(settings.window_size.width as f32);
        settings_data.set_height(settings.window_size.height as f32);
        settings_data.set_save_window_size(settings.save_window_size);
        settings_data.set_current_path(settings.path.clone().into());

        platform.read().await.set_theme(settings.theme).await;
    }

    let p = platform.clone();
    app_data.on_open_link(move |link| {
        let p = p.clone();
        slint::spawn_local(async move { p.read().await.open_link(link.into()).await }).unwrap();
    });

    let s = settings.clone();
    let window = main_window.clone_strong();
    let p = platform.clone();
    main_window
        .global::<Localization>()
        .on_set_locale(move |locale_name| {
            let denominator = get_locale_denominator(Some(locale_name.into()));
            localize(
                Some(denominator.to_string()),
                window.global::<Localization>(),
            );
            let s = s.clone();
            let p = p.clone();
            slint::spawn_local(async move {
                s.write().await.locale = Some(denominator);
                s.read().await.save(p.read().await).await;
            })
            .unwrap();
        });
    let s = settings.clone();
    let window = main_window.clone_strong();
    let p = platform.clone();
    settings_data.on_change_theme_callback(move |theme_name| {
        if let Ok(theme) = Theme::try_from(theme_name) {
            window
                .global::<SettingsData>()
                .set_color_scheme(theme.into());
            let s = s.clone();
            let p = p.clone();
            slint::spawn_local(async move {
                s.write().await.theme = theme;
                p.read().await.set_theme(theme).await;
                s.read().await.save(p.read().await).await;
            })
            .unwrap();
        }
    });
    let s = settings.clone();
    settings_data.on_toggle_save_window_size(move |save| {
        let s = s.clone();
        slint::spawn_local(async move {
            s.write().await.save_window_size = save;
        })
        .unwrap();
    });
    let path = tx_path.clone();
    settings_data.on_path(move || {
        let tx_path = path.clone();
        slint::spawn_local(async move {
            tx_path.send_async((String::new(), false)).await.unwrap();
        })
        .unwrap();
    });
    let s = settings.clone();
    settings_data.on_scan(move || {
        let tx_path = tx_path.clone();
        let settings = s.clone();
        slint::spawn_local(async move {
            tx_path
                .send_async((settings.read().await.path.clone(), false))
                .await
                .unwrap();
        })
        .unwrap();
    });
    let w = tx_wake.clone();
    let t = tx.clone();
    app_data.on_clicked(move |i| { t.send(RunnerMessage::PlayTrack(i as usize)).unwrap(); let _ = w.send(());});
    let t = tx.clone();
    let w = tx_wake.clone();
    app_data.on_play_previous(move || { t.send(RunnerMessage::PlayPrevious).unwrap();  let _ = w.send(());});
    let w = tx_wake.clone();
    let t = tx.clone();
    app_data.on_toggle_pause(move || { t.send(RunnerMessage::TogglePause).unwrap(); let _ = w.send(());});
    let w = tx_wake.clone();
    let t = tx.clone();
    app_data.on_play_next(move || { t.send(RunnerMessage::PlayNext).unwrap(); let _ = w.send(());});
    let w = tx_wake.clone();
    let t = tx.clone();
    app_data.on_toggle_repeat(move || { t.send(RunnerMessage::ToggleRepeat).unwrap(); let _ = w.send(());});
    let t = tx.clone();
    app_data.on_seek(move |time| {
        t.send(RunnerMessage::Seek(RunnerSeek::Absolute(time as f64)))
            .unwrap()
    });
    let t = tx.clone();
    app_data.on_set_volume(move |volume| t.send(RunnerMessage::SetVolume(volume as f64)).unwrap());
    app_data.on_searching(move |searching| tx_searching.send(searching.to_string()).unwrap());
    app_data.on_changing(move || tx_changing.send(()).unwrap());
}

async fn updater_task<P: crate::platform::Platform + Send + 'static + Sync>(
    r: Runner,
    s: Settings,
    p: Platform<P>,
    window: Weak<MainWindow>,
    rx_tracks: Receiver<Vec<TrackData>>,
    rx_changing: Receiver<()>,
    rx_searching: Receiver<String>,
    rx_l: Receiver<Option<(usize, FileTrack)>>,
    rx_wake: Receiver<()>,
    rx_vis: Receiver<bool>,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let mut is_app_visible = true;
    let mut searching = String::new();
    let mut old_index = usize::MAX;
    let mut loaded = 0;
    let mut saved = false;
    let mut changes = vec![];
    let mut tracks = vec![];
    if let Ok(tracks) = rx_tracks.try_recv() {
        changes.push(Changes::Tracks(tracks));
    }

    loop {
        let is_playing = r.read().await.playback();

        let mut new_loaded = false;
        let mut updated_search = false;
        let mut save_y = false;
        let mut change_time = true;
        let mut ui_needs_update = false;

        tokio::select! {
            // If ui it's visible and playback it's playing, then every 250ms update it
            _ = interval.tick(), if is_playing => {
                ui_needs_update = true;
            }

            // Event A: UI it's visible
            Ok(visible) = rx_vis.recv_async() => {
                is_app_visible = visible;
                if visible {
                    ui_needs_update = true;
                }
            }

            // Event B: UI or media control it's interacted
            Ok(()) = rx_wake.recv_async() => {
                ui_needs_update = true;
            }

            // Event C: New tracks loaded from a directory scan
            Ok(new_tracks) = rx_tracks.recv_async() => {
                changes.push(Changes::Tracks(new_tracks));
                new_loaded = true;
                loaded = 0;
                s.write().await.clear_tracks(p.read().await).await;
                ui_needs_update = true;
            }

            // Event D: Typing in the search bar
            Ok(search_string) = rx_searching.recv_async() => {
                if searching.is_empty() {
                    save_y = true;
                }
                searching = search_string;
                updated_search = true;
                ui_needs_update = true;
            }

            // Event E: Dragging the playback slider
            Ok(()) = rx_changing.recv_async() => {
                change_time = false;
                ui_needs_update = true;
            }

            // Event F: Metadata loader
            Ok(track_data) = rx_l.recv_async() => {
                let mut process_track = |data: Option<(usize, FileTrack)>| {
                    if let Some((index, file_track)) = data {
                        let file = file_track.clone();
                        tracks.push(file);
                        let mut track: TrackData = file_track.into();
                        track.index = index as i32;
                        changes.push(Changes::Metadata(index, track));
                        loaded += 1;
                        new_loaded = true;
                    } else if !saved {
                        saved = true;

                        let s_clone = s.clone();
                        let p_clone = p.clone();
                        let tracks_to_save = std::mem::take(&mut tracks);

                        tokio::spawn(async move {
                            let mut settings = s_clone.write().await;
                            settings.add_tracks(p_clone.read().await, tracks_to_save).await;
                            settings.save_timestamp().await;
                            settings.save(p_clone.read().await).await;
                        });
                        new_loaded = true;
                    }
                };

                process_track(track_data);

                while let Ok(more_data) = rx_l.try_recv() {
                    process_track(more_data);
                }
                ui_needs_update = true;
            }
        }

        if !is_app_visible {
            continue;
        }

        if !ui_needs_update {
            continue;
        }

        let guard = r.read().await;
        let mut index = guard.index();
        let len = guard.len();
        if index > len {
            index = 0;
        }

        let playback = guard.playback();
        let repeat = guard.repeat();
        let time = guard.time();
        let length = time.length;
        let time_float = time.position;
        let volume = guard.volume();
        let position = time.format_pos();

        let progress = if len == 0 { 0.0 } else { loaded as f64 / len as f64 };

        if old_index != index || new_loaded {
            old_index = index;
        }

        let mut search = searching.to_lowercase();
        let c = std::mem::take(&mut changes);

        window
            .upgrade_in_event_loop(move |window| {
                let app_data = window.global::<AppData>();
                app_data.set_playing(index as i32);
                app_data.set_position_time(position.into());
                if change_time {
                    app_data.set_time(time_float as f32);
                }
                app_data.set_length(length as f32);
                app_data.set_playback(playback);
                app_data.set_repeat(repeat);
                app_data.set_volume(volume as f32);

                if new_loaded {
                    let progress = if progress == 1.0 {
                        0.0
                    } else {
                        progress as f32
                    };
                    app_data.set_progress(progress);
                } else { // Assume loader already finished
                    app_data.set_progress(0.0);
                }

                for change in c {
                    match change {
                        Changes::Tracks(tracks) => {
                            app_data.set_tracks(VecModel::from_slice(&tracks));
                        }
                        Changes::Metadata(index, track) => {
                            app_data.get_tracks().set_row_data(index, track);
                        }
                    }
                }

                let maybe_search = app_data.get_search_text().to_string();

                if maybe_search.is_empty() && maybe_search != search {
                    updated_search = true;
                    search = maybe_search;
                }

                if updated_search || new_loaded {
                    let tracks = app_data.get_tracks();
                    let mut counter = 0;
                    if save_y {
                        app_data.set_saved_y(app_data.get_viewport_y());
                    }
                    for (index, mut track) in tracks.iter().enumerate() {
                        let title = track.title.to_lowercase();
                        let artist = track.artist.to_lowercase();
                        if search.is_empty() && !track.visible {
                            track.visible = true;
                        } else if !search.is_empty() {
                            if title.contains(&search) || artist.contains(&search) {
                                counter += 1;
                                if track.visible {
                                    continue;
                                }
                                track.visible = true;
                            } else {
                                if !track.visible {
                                    continue;
                                }
                                track.visible = false;
                            }
                        } else {
                            continue;
                        }
                        tracks.set_row_data(index, track);
                    }
                    let height = (counter * -84) as f32;
                    if height > app_data.get_viewport_y() {
                        app_data.set_viewport_y(0.0);
                    }
                    if updated_search && search.is_empty() {
                        app_data.set_viewport_y(app_data.get_saved_y());
                    }
                }
            })
            .unwrap();
    }
}

async fn loader_task(
    runner: Runner,
    tx: Sender<Option<(usize, FileTrack)>>,
    rx_l: Arc<Mutex<Receiver<usize>>>,
) {
    loop {
        if let Ok(index) = rx_l.lock().await.recv_async().await {
            if index == usize::MAX {
                if let Err(e) = tx.send_async(None).await {
                    eprintln!("error happened when signaling end of task, probably because the app was closed: {e}");
                }
                return;
            }
            if let Some(path) = runner.read().await.get_path_for_file(index).await {
                if let Ok(track) = MusicTrack::new(path.to_string_lossy().to_string()) {
                    if let Ok(Ok(meta)) =
                        tokio::task::spawn_blocking(move || track.get_meta()).await
                    {
                        let p = path.clone();
                        let image = get_image_squared(p, 128, 128).await;

                        if let Err(e) = tx
                            .send_async(Some((
                                index,
                                FileTrack {
                                    path: remove_ext(path),
                                    title: meta.title,
                                    artist: meta.artist,
                                    length: meta.time.length,
                                    image: image
                                        .map(|i| i.flatten_to_u8()[0].clone())
                                        .unwrap_or(vec![]),
                                },
                            )))
                            .await
                        {
                            eprintln!("error happened during metadata transfer, probably because the app was closed: {e}");
                        }
                    }
                }
            }
        }
    }
}

async fn loader<P: crate::platform::Platform + Send + 'static>(
    runner: Runner,
    settings: Settings,
    platform: Platform<P>,
    tx: Sender<Option<(usize, FileTrack)>>,
    rx: Receiver<(String, bool)>,
    tx_tracks: Sender<Vec<TrackData>>,
) {
    loop {
        if let Ok((mut path, check_cache)) = rx.recv_async().await {
            if path.is_empty() && !check_cache {
                path = platform
                    .read()
                    .await
                    .ask_music_dir()
                    .await
                    .to_str()
                    .unwrap()
                    .to_string();
                settings.write().await.path = path.clone();
                settings.read().await.save(platform.read().await).await;
            }
            let len = {
                let mut guard = runner.write().await;
                guard.clear().await;
                guard.set_path(path.clone());
                add_all_tracks_to_player(guard.deref_mut(), path).await;
                guard.len()
            };

            let check_timestamp = settings.read().await.check_timestamp().await;
            let file_tracks = settings
                .read()
                .await
                .read_tracks(platform.read().await)
                .await;
            let is_cached = check_timestamp && !file_tracks.is_empty() && check_cache;
            println!("check timestamp: {check_timestamp}; is cached: {is_cached}");
            let mut cached_map = HashMap::with_capacity(file_tracks.len());
            if is_cached {
                for ft in file_tracks.into_iter() {
                    cached_map.insert(ft.path.clone(), ft);
                }
            }
            let mut tracks = Vec::with_capacity(len);
            for i in 0..len {
                let track_path = runner.read().await.get_path_for_file(i).await.unwrap();
                let track_without_ext = remove_ext(&track_path);
                if is_cached {
                    if let Some(file_track) = cached_map.get(&track_without_ext) {
                        let mut track: TrackData = file_track.clone().into();
                        track.index = i as i32;
                        tracks.push(track);
                        continue;
                    }
                }
                tracks.push(TrackData {
                    artist: Default::default(),
                    cover: Default::default(),
                    time: Default::default(),
                    title: track_without_ext.into(),
                    index: i as i32,
                    visible: true,
                });
            }
            tracks.shrink_to_fit();
            tx_tracks.send_async(tracks).await.unwrap();

            if is_cached {
                continue;
            }

            let mut tasks = vec![];
            let (tx_l, rx_l) = flume::unbounded();
            let rx_l = Arc::new(Mutex::new(rx_l));
            let cpus = num_cpus::get() * 4;
            for _ in 0..cpus {
                let runner = runner.clone();
                let tx = tx.clone();
                let rx_l = rx_l.clone();
                tasks.push(tokio::task::spawn(loader_task(runner, tx, rx_l)));
            }
            for i in 0..len {
                tx_l.send_async(i).await.unwrap();
            }
            for _ in 0..cpus {
                tx_l.send_async(usize::MAX).await.unwrap();
            }
            for task in tasks {
                task.await.unwrap();
            }
        }
    }
}
