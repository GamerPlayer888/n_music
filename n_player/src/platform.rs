use crate::bus_server::Property;
use crate::runner::{Runner, RunnerMessage};
use async_trait::async_trait;
use flume::Sender;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

#[cfg(target_os = "android")]
use jni::jni_sig;
#[cfg(target_os = "android")]
use jni::jni_str;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn open_link_desktop(link: String) {
    open::that(link).unwrap();
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
async fn internal_dir_desktop() -> PathBuf {
    let base_dirs = directories::BaseDirs::new().unwrap();
    let local_data_dir = base_dirs.data_local_dir();
    let app_dir = local_data_dir.join("n_music");
    if !app_dir.exists() {
        tokio::fs::create_dir(app_dir.as_path()).await.unwrap();
    }
    app_dir
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
async fn ask_music_dir_desktop() -> PathBuf {
    if let Some(path) = rfd::AsyncFileDialog::new().pick_folder().await {
        PathBuf::from(path)
    } else {
        PathBuf::new()
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
async fn ask_file_desktop() -> Vec<PathBuf> {
    if let Some(path) = rfd::AsyncFileDialog::new().pick_files().await {
        path.into_iter().map(|fd| PathBuf::from(fd)).collect()
    } else {
        vec![]
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
fn set_clipboard_text_desktop(text: String) {
    use arboard::Clipboard;
    let mut clipboard = Clipboard::new().unwrap();
    clipboard.set_text(text).unwrap();
}

#[allow(unused_variables)]
#[async_trait]
/// Abstraction over a number of platforms (desktop and mobile)
pub trait Platform {
    /// Ask the platform to "copy" a given text
    fn set_clipboard_text(&mut self, text: String);

    /// Ask underlying platform to open a web link
    async fn open_link(&self, link: String);
    /// Ask underlying platform to get the app directory
    async fn internal_dir(&self) -> PathBuf;
    /// Ask underlying platform to ask user for the music dir
    async fn ask_music_dir(&self) -> PathBuf;
    /// Ask underlying platform to ask user for files
    async fn ask_file(&self) -> Vec<PathBuf>;
    /// Notify the platform that a [Runner] is ready and save it in memory
    async fn add_runner(&mut self, runner: Arc<RwLock<Runner>>, tx: Sender<RunnerMessage>)
    where
        Self: Sized,
    {
    }
    /// Notify the platform that some playback properties have changed and update those accordingly
    async fn properties_changed<P: IntoIterator<Item = Property> + Send>(&self, properties: P)
    where
        Self: Sized,
    {
    }
    /// Allows the platform to do operations once in a while
    async fn tick(&mut self)
    where
        Self: Sized,
    {
    }
}

#[cfg(target_os = "linux")]
pub struct LinuxPlatform {
    server: Option<mpris_server::Server<crate::bus_server::linux::MPRISBridge>>,
}

#[cfg(target_os = "linux")]
impl LinuxPlatform {
    pub fn new() -> Self {
        Self { server: None }
    }
}

#[cfg(target_os = "linux")]
impl LinuxPlatform {
    pub async fn create_server(
        runner: Arc<RwLock<Runner>>,
        tx: Sender<RunnerMessage>,
        unique: bool,
    ) -> Option<mpris_server::Server<crate::bus_server::linux::MPRISBridge>> {
        let name = if !unique {
            "n_music".to_string()
        } else {
            format!("n_music_{}", std::process::id())
        };

        mpris_server::Server::new(
            &name,
            crate::bus_server::linux::MPRISBridge::new(runner, tx),
        )
        .await
        .ok()
    }
}

#[cfg(target_os = "linux")]
#[async_trait]
impl Platform for LinuxPlatform {
    fn set_clipboard_text(&mut self, text: String) {
        set_clipboard_text_desktop(text);
    }

    async fn open_link(&self, link: String) {
        open_link_desktop(link)
    }

    async fn internal_dir(&self) -> PathBuf {
        internal_dir_desktop().await
    }

    async fn ask_music_dir(&self) -> PathBuf {
        ask_music_dir_desktop().await
    }

    async fn ask_file(&self) -> Vec<PathBuf> {
        ask_file_desktop().await
    }

    async fn add_runner(&mut self, runner: Arc<RwLock<Runner>>, tx: Sender<RunnerMessage>) {
        let server = Self::create_server(runner.clone(), tx.clone(), false).await;

        let server = match server {
            None => Self::create_server(runner, tx, true).await.unwrap(),
            Some(s) => s,
        };

        self.server = Some(server);
    }
    async fn properties_changed<P: IntoIterator<Item = Property> + Send>(&self, properties: P) {
        if let Some(server) = &self.server {
            let mut new_properties = vec![];
            for p in properties {
                if let Property::PositionChanged(_) = p {
                    continue;
                }
                new_properties.push(match p {
                    Property::Playing(playing) => {
                        mpris_server::Property::PlaybackStatus(if playing {
                            mpris_server::PlaybackStatus::Playing
                        } else {
                            mpris_server::PlaybackStatus::Paused
                        })
                    }
                    Property::Metadata(metadata) => {
                        let mut meta = mpris_server::Metadata::new();

                        meta.set_title(metadata.title);
                        meta.set_artist(metadata.artists);
                        meta.set_length(Some(mpris_server::Time::from_secs(
                            metadata.length as i64,
                        )));
                        meta.set_art_url(metadata.image_path);
                        meta.set_trackid(Some(
                            mpris_server::zbus::zvariant::ObjectPath::from_string_unchecked(
                                metadata.id,
                            ),
                        ));

                        mpris_server::Property::Metadata(meta)
                    }
                    Property::Volume(volume) => mpris_server::Property::Volume(volume),
                    Property::Looping(loop_status) => {
                        let loop_status = match loop_status {
                            true => {
                                mpris_server::LoopStatus::Playlist
                            }
                            false => mpris_server::LoopStatus::Track,
                        };

                        mpris_server::Property::LoopStatus(loop_status)
                    }
                    _ => unreachable!("check skipped somehow"),
                });
            }
            server.properties_changed(new_properties).await.unwrap()
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub struct DesktopPlatform {}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[async_trait]
impl Platform for DesktopPlatform {
    fn set_clipboard_text(&mut self, text: String) {
        set_clipboard_text_desktop(text);
    }

    async fn open_link(&self, link: String) {
        open_link_desktop(link)
    }

    async fn internal_dir(&self) -> PathBuf {
        internal_dir_desktop().await
    }

    async fn ask_music_dir(&self) -> PathBuf {
        ask_music_dir_desktop().await
    }

    async fn ask_file(&self) -> Vec<PathBuf> {
        ask_file_desktop().await
    }
}

#[cfg(target_os = "android")]
pub struct AndroidPlatform {
    app: slint::android::AndroidApp,
    jvm: jni::JavaVM,
    callback: jni::objects::Global<jni::objects::JObject<'static>>,
    tx: Option<Sender<RunnerMessage>>,
}

#[cfg(target_os = "android")]
impl AndroidPlatform {
    pub fn new(
        app: slint::android::AndroidApp,
        jvm: jni::JavaVM,
        callback: jni::objects::Global<jni::objects::JObject<'static>>,
    ) -> Self {
        Self {
            app,
            jvm,
            callback,
            tx: None,
        }
    }
}

#[cfg(target_os = "android")]
#[async_trait]
impl Platform for AndroidPlatform {
    fn set_clipboard_text(&mut self, text: String) {
        self.jvm.attach_current_thread(|env| {
            let java_string = env.new_string(text)?;
            env.call_method(
                &self.callback,
                jni::jni_str!("set_clipboard_text"),
                jni::jni_sig!("(Ljava/lang/String;)V"),
                &[(&java_string).into()],
            )?;
            Ok::<_, jni::errors::Error>(())
        }).unwrap();
    }

    async fn open_link(&self, link: String) {
        self.jvm.attach_current_thread(|env| {
            let java_string = env.new_string(link)?;
            env.call_method(
                &self.callback,
                jni::jni_str!("openLink"),
                jni::jni_sig!("(Ljava/lang/String;)V"),
                &[(&java_string).into()],
            )?;
            Ok::<_, jni::errors::Error>(())
        }).unwrap();
    }

    async fn internal_dir(&self) -> PathBuf {
        let path = self
            .app
            .external_data_path()
            .expect("can't get external data path")
            .join("config/");
        if !path.exists() {
            std::fs::create_dir(&path).unwrap();
        }
        path
    }

    async fn ask_music_dir(&self) -> PathBuf {
        self.jvm.attach_current_thread(|env| {
            env.call_method(
                &self.callback,
                jni_str!("askDirectory"),
                jni_sig!("()V"),
                &[]
            )?;
            Ok::<_, jni::errors::Error>(())
        }).unwrap();

        while let Ok(message) = crate::ANDROID_TX.recv() {
            if let crate::MessageAndroidToRust::Directory(path) = message {
                println!("got directory from user");
                return PathBuf::from(path);
            } else {
                crate::ANDROID_TX.send(message).unwrap();
            }
        }
        PathBuf::new()
    }

    async fn ask_file(&self) -> Vec<PathBuf> {
        self.jvm.attach_current_thread(|env| {
            env.call_method(
                &self.callback,
                jni_str!("askFile"),
                jni_sig!("()V"),
                &[]
            )?;
            Ok::<_, jni::errors::Error>(())
        }).unwrap();
        while let Ok(message) = crate::ANDROID_TX.recv() {
            if let crate::MessageAndroidToRust::File(path) = message {
                return vec![PathBuf::from(path)];
            } else {
                crate::ANDROID_TX.send(message).unwrap();
            }
        }
        vec![]
    }

    async fn add_runner(&mut self, runner: Arc<RwLock<Runner>>, tx: Sender<RunnerMessage>) {
        self.jvm.attach_current_thread(|env| {
            env.call_method(
                &self.callback,
                jni_str!("createNotification"),
                jni_sig!("()V"),
                &[]
            )?;
            Ok::<_, jni::errors::Error>(())
        }).unwrap();

        self.tx = Some(tx.clone());

        tokio::spawn(async move {
            while let Ok(message) = crate::ANDROID_TX.recv_async().await {
                if let crate::MessageAndroidToRust::Callback(msg) = message {
                    tx.send_async(msg)
                        .await
                        .expect("error sending callback command to runner");
                } else {
                    crate::ANDROID_TX.send_async(message).await.unwrap();
                }
            }
        });
    }

    async fn properties_changed<P: IntoIterator<Item = Property> + Send>(&self, properties: P) {
        self.jvm.attach_current_thread(|env| {
            for p in properties {
                match p {
                    Property::Playing(playing) => {
                        env.call_method(
                            &self.callback,
                            jni_str!("changePlaybackStatus"),
                            jni_sig!("(Z)V"),
                            &[playing.into()],
                        )?;
                    }
                    Property::Metadata(metadata) => {
                        let title = env
                            .new_string(metadata.title.unwrap_or(String::new()))?;
                        let artist = env
                            .new_string(metadata.artists.unwrap_or(vec![String::new()]).join(", "))?;
                        let cover_path = env
                            .new_string(metadata.image_path.unwrap_or(String::new()))?;
                        env.call_method(
                            &self.callback,
                            jni_str!("changeNotification"),
                            jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;D)V"),
                            &[
                                (&title).into(),
                                (&artist).into(),
                                (&cover_path).into(),
                                jni::JValue::Double(metadata.length),
                            ],
                        )?;
                    }
                    Property::PositionChanged(seek) => {
                        env.call_method(
                            &self.callback,
                            jni_str!("changePlaybackSeek"),
                            jni_sig!("(D)V"),
                            &[jni::JValue::Double(seek)]
                        )?;
                    }
                    Property::Looping(is_looping) => {
                        env.call_method(
                            &self.callback,
                            jni_str!("changeLoopingStatus"),
                            jni_sig!("(Z)V"),
                            &[jni::JValue::Bool(is_looping)],
                        )?;
                    }
                    _ => {}
                }
            }
            Ok::<_, jni::errors::Error>(())
        }).unwrap();
    }
}
