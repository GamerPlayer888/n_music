use crate::localization::{get_locale_denominator, localize};
use crate::runner::{run, RunnerMessage, RunnerSeek};
use crate::{
    add_all_tracks_to_player, bus_server, get_image, AppData, FileTrack, Localization, MainWindow,
    SettingsData, Theme, TrackData, WindowSize,
};
use flume::{Receiver, Sender};
use n_audio::music_track::MusicTrack;
use n_audio::queue::QueuePlayer;
use n_audio::remove_ext;
use rimage::codecs::webp::WebPDecoder;
use rimage::operations::resize::{FilterType, ResizeAlg};
use slint::{ComponentHandle, Model, VecModel, Weak};
use std::io::Cursor;
use std::mem;
use std::ops::DerefMut;
use std::sync::Arc;
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::sync::{Mutex, RwLock};
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_image::image::Image;
use zune_image::traits::{DecoderTrait, OperationsTrait};
use zune_imageprocs::crop::Crop;

pub type Runner = Arc<RwLock<crate::runner::Runner>>;
pub type Settings = Arc<Mutex<crate::settings::Settings>>;
#[allow(type_alias_bounds)]
pub type Platform<P: crate::platform::Platform + Send + 'static> = Arc<Mutex<P>>;

enum Changes {
    Tracks(Vec<TrackData>),
    Metadata(u16, TrackData),
}

pub async fn run_app<P: crate::platform::Platform + Send + 'static>(
    settings: crate::settings::Settings,
    platform: P,
) {
    let platform = Arc::new(Mutex::new(platform));
    let settings = Arc::new(Mutex::new(settings));

    let tmp = NamedTempFile::new().unwrap();
    let (tx, rx) = flume::unbounded();

    let player = QueuePlayer::new(settings.lock().await.path.clone());

    let runner = Arc::new(RwLock::new(crate::runner::Runner::new(player)));

    let r = runner.clone();
    let tx_t = tx.clone();

    let (tx_l, rx_l) = flume::unbounded();
    let main_window = MainWindow::new().unwrap();

    let p = platform.clone();
    p.lock().await.add_runner(r.clone(), tx_t.clone()).await;
    let (tx_path, rx_path) = flume::unbounded();
    tx_path
        .send_async((settings.lock().await.path.clone(), true))
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

    setup_data(
        settings.clone(),
        platform.clone(),
        main_window.clone_strong(),
        tx.clone(),
        tx_searching,
        tx_changing,
        tx_path,
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
    ));

    tokio::task::block_in_place(|| main_window.run().unwrap());

    updater.abort();
    future.abort();

    settings.lock().await.volume = runner.read().await.volume();
    if settings.lock().await.save_window_size {
        let width = main_window.get_last_width() as usize;
        let height = main_window.get_last_height() as usize;
        settings.lock().await.window_size = WindowSize { width, height };
    } else {
        settings.lock().await.window_size = WindowSize::default();
    }
    settings.lock().await.save(platform.lock().await).await;
}

async fn setup_data<P: crate::platform::Platform + Send + 'static>(
    settings: Settings,
    platform: Platform<P>,
    main_window: MainWindow,
    tx: Sender<RunnerMessage>,
    tx_searching: Sender<String>,
    tx_changing: Sender<()>,
    tx_path: Sender<(String, bool)>,
) {
    localize(
        settings.lock().await.locale.clone(),
        main_window.global::<Localization>(),
    );

    let settings_data = main_window.global::<SettingsData>();
    let app_data = main_window.global::<AppData>();

    #[cfg(target_os = "android")]
    app_data.set_android(true);
    app_data.set_version(env!("CARGO_PKG_VERSION").into());

    settings_data.set_color_scheme(settings.lock().await.theme.into());
    settings_data.set_theme(i32::from(settings.lock().await.theme));
    settings_data.set_width(settings.lock().await.window_size.width as f32);
    settings_data.set_height(settings.lock().await.window_size.height as f32);
    settings_data.set_save_window_size(settings.lock().await.save_window_size);
    settings_data.set_current_path(settings.lock().await.path.clone().into());

    let p = platform.clone();
    app_data.on_open_link(move |link| {
        let p = p.clone();
        slint::spawn_local(async move { p.lock().await.open_link(link.into()).await }).unwrap();
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
                s.lock().await.locale = Some(denominator);
                s.lock().await.save(p.lock().await).await;
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
                s.lock().await.theme = theme;
                s.lock().await.save(p.lock().await).await;
            })
            .unwrap();
        }
    });
    let s = settings.clone();
    settings_data.on_toggle_save_window_size(move |save| {
        let s = s.clone();
        slint::spawn_local(async move {
            s.lock().await.save_window_size = save;
        })
        .unwrap();
    });
    settings_data.on_path(move || {
        let tx_path = tx_path.clone();
        slint::spawn_local(async move {
            tx_path.send_async((String::new(), false)).await.unwrap();
        })
        .unwrap();
    });
    let t = tx.clone();
    app_data.on_clicked(move |i| t.send(RunnerMessage::PlayTrack(i as u16)).unwrap());
    let t = tx.clone();
    app_data.on_play_previous(move || t.send(RunnerMessage::PlayPrevious).unwrap());
    let t = tx.clone();
    app_data.on_toggle_pause(move || t.send(RunnerMessage::TogglePause).unwrap());
    let t = tx.clone();
    app_data.on_play_next(move || t.send(RunnerMessage::PlayNext).unwrap());
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

async fn updater_task<P: crate::platform::Platform + Send + 'static>(
    r: Runner,
    s: Settings,
    p: Platform<P>,
    window: Weak<MainWindow>,
    rx_tracks: Receiver<Vec<TrackData>>,
    rx_changing: Receiver<()>,
    rx_searching: Receiver<String>,
    rx_l: Receiver<Option<(u16, FileTrack)>>,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(250));
    let mut searching = String::new();
    let mut old_index = u16::MAX;
    let mut loaded = 0;
    let mut saved = false;
    let mut changes = vec![];
    let mut tracks = vec![];
    if let Ok(tracks) = rx_tracks.recv_async().await {
        changes.push(Changes::Tracks(tracks));
    }
    loop {
        interval.tick().await;
        let guard = r.read().await;
        let mut index = guard.index();
        let len = guard.len() as u16;
        if index > len {
            index = 0;
        }
        let playback = guard.playback();
        let time = guard.time();
        let length = time.length;
        let time_float = time.position;
        let volume = guard.volume();
        let position = time.format_pos();

        let change_time = if let Ok(()) = rx_changing.try_recv() {
            false
        } else {
            true
        };

        let mut new_loaded = false;

        if let Ok(tracks) = rx_tracks.try_recv() {
            changes.push(Changes::Tracks(tracks));
            new_loaded = true;
            loaded = 0;
            s.lock().await.clear_tracks(p.lock().await).await;
        }

        while let Ok(track_data) = rx_l.try_recv() {
            if let Some((index, file_track)) = track_data {
                let file = file_track.clone();
                tracks.push(file);
                let mut track: TrackData = file_track.into();
                track.index = index as i32;
                changes.push(Changes::Metadata(index, track));
                loaded += 1;
                new_loaded = true;
            } else {
                if !saved {
                    saved = true;
                    s.lock()
                        .await
                        .add_tracks(p.lock().await, mem::take(&mut tracks))
                        .await;
                    s.lock().await.save_timestamp().await;
                    s.lock().await.save(p.lock().await).await;
                }
                new_loaded = true;
            }
        }
        let progress = loaded as f64 / len as f64;
        if old_index != index || new_loaded {
            old_index = index;
        }

        let mut updated_search = false;
        while let Ok(search_string) = rx_searching.try_recv() {
            searching = search_string;
            updated_search = true;
        }

        p.lock().await.tick().await;
        let mut search = searching.to_lowercase();

        let c = mem::take(&mut changes);
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
                app_data.set_volume(volume as f32);

                if new_loaded {
                    let progress = if progress == 1.0 {
                        0.0
                    } else {
                        progress as f32
                    };
                    app_data.set_progress(progress);
                }

                for change in c {
                    match change {
                        Changes::Tracks(tracks) => {
                            app_data.set_tracks(VecModel::from_slice(&tracks));
                        }
                        Changes::Metadata(index, track) => {
                            app_data.get_tracks().set_row_data(index as usize, track);
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
                    for (index, mut track) in tracks.iter().enumerate() {
                        let title = track.title.to_lowercase();
                        let artist = track.artist.to_lowercase();
                        if search.is_empty() {
                            track.visible = true;
                        } else {
                            if title.contains(&search) || artist.contains(&search) {
                                track.visible = true;
                            } else {
                                track.visible = false;
                            }
                        }
                        tracks.set_row_data(index, track);
                    }
                }
            })
            .unwrap();
    }
}

async fn loader_task(
    runner: Runner,
    tx: Sender<Option<(u16, FileTrack)>>,
    rx_l: Arc<Mutex<Receiver<u16>>>,
) {
    loop {
        if let Ok(index) = rx_l.lock().await.recv_async().await {
            if index == u16::MAX {
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
                        let mut image = if let Ok(image) =
                            tokio::task::spawn_blocking(move || get_image(p)).await
                        {
                            if !image.is_empty() {
                                let zune_image = if let Ok(image) =
                                    Image::read(ZCursor::new(&image), DecoderOptions::new_fast())
                                {
                                    Some(image)
                                } else if let Ok(mut webp_decoder) =
                                    WebPDecoder::try_new(Cursor::new(&image))
                                {
                                    if let Ok(image) = webp_decoder.decode() {
                                        Some(image)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                };

                                if let Some(mut zune_image) = zune_image {
                                    zune_image.convert_color(ColorSpace::RGB).unwrap();
                                    let (width, height) = zune_image.dimensions();
                                    if width != height {
                                        let difference = width.abs_diff(height);
                                        let min = width.min(height);
                                        let is_height = height < width;
                                        let x = if is_height { difference / 2 } else { 0 };
                                        let y = if !is_height { difference / 2 } else { 0 };
                                        tokio::task::block_in_place(|| {
                                            Crop::new(min, min, x, y)
                                                .execute(&mut zune_image)
                                                .unwrap()
                                        });
                                    }
                                    tokio::task::block_in_place(|| {
                                        rimage::operations::resize::Resize::new(
                                            128,
                                            128,
                                            ResizeAlg::Convolution(FilterType::Hamming),
                                        )
                                        .execute(&mut zune_image)
                                        .unwrap()
                                    });
                                    zune_image.flatten_to_u8()[0].clone()
                                } else {
                                    vec![]
                                }
                            } else {
                                vec![]
                            }
                        } else {
                            vec![]
                        };

                        image.shrink_to_fit();

                        if let Err(e) = tx
                            .send_async(Some((
                                index,
                                FileTrack {
                                    path: remove_ext(path),
                                    title: meta.title,
                                    artist: meta.artist,
                                    length: meta.time.length,
                                    image,
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
    tx: Sender<Option<(u16, FileTrack)>>,
    rx: Receiver<(String, bool)>,
    tx_tracks: Sender<Vec<TrackData>>,
) {
    loop {
        if let Ok((mut path, check_cache)) = rx.recv_async().await {
            if path.is_empty() && !check_cache {
                path = platform
                    .lock()
                    .await
                    .ask_music_dir()
                    .await
                    .to_str()
                    .unwrap()
                    .to_string();
                settings.lock().await.path = path.clone();
                settings.lock().await.save(platform.lock().await).await;
            }
            let len = {
                let mut guard = runner.write().await;
                guard.clear().await;
                guard.set_path(path.clone());
                add_all_tracks_to_player(guard.deref_mut(), path).await;
                guard.len() as u16
            };

            let check_timestamp = settings.lock().await.check_timestamp().await;
            let file_tracks = settings
                .lock()
                .await
                .read_tracks(platform.lock().await)
                .await;
            let is_cached = check_timestamp && !file_tracks.is_empty() && check_cache;
            println!("check timestamp: {check_timestamp}; is cached: {is_cached}");

            let mut tracks = vec![];
            for i in 0..len {
                let track_path = runner.read().await.get_path_for_file(i).await.unwrap();
                if is_cached {
                    let track_without_ext = remove_ext(track_path);
                    if let Some(file_track) = file_tracks
                        .iter()
                        .find(|file_track| file_track.path == track_without_ext)
                    {
                        let mut track: TrackData = file_track.clone().into();
                        track.index = i as i32;
                        tracks.push(track)
                    }
                } else {
                    tracks.push(TrackData {
                        artist: Default::default(),
                        cover: Default::default(),
                        time: Default::default(),
                        title: remove_ext(track_path).into(),
                        index: i as i32,
                        visible: true,
                    });
                }
            }
            tracks.shrink_to_fit();
            tx_tracks.send_async(tracks).await.unwrap();

            if is_cached {
                continue;
            }

            let mut tasks = vec![];
            let (tx_l, rx_l) = flume::unbounded();
            let rx_l = Arc::new(Mutex::new(rx_l));
            let cpus = num_cpus::get() * 8;
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
                tx_l.send_async(u16::MAX).await.unwrap();
            }
            for task in tasks {
                task.await.unwrap();
            }
        }
    }
}
