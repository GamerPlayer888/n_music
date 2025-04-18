use crate::platform::Platform;
use crate::{FileTrack, Theme, WindowSize};
use bitcode::{Decode, Encode};
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufReader, BufWriter, Cursor};
use std::ops::Deref;
use std::path::PathBuf;

#[derive(Debug, Decode, Encode)]
pub struct Settings {
    pub path: String,
    pub volume: f64,
    pub theme: Theme,
    pub window_size: WindowSize,
    pub save_window_size: bool,
    pub locale: Option<String>,
    pub timestamp: Option<u64>,
}

impl Settings {
    fn read_from_file(storage_file: PathBuf) -> Self {
        if storage_file.exists() && storage_file.is_file() {
            let mut data = vec![];
            if let Ok(_) = zstd::stream::copy_decode(
                File::open(storage_file).unwrap(),
                BufWriter::new(Cursor::new(&mut data)),
            ) {
                if let Ok(storage) = bitcode::decode(&data) {
                    storage
                } else {
                    eprintln!("not encoded");
                    Self::default()
                }
            } else {
                eprintln!("bad file");
                Self::default()
            }
        } else {
            eprintln!("file not found");
            Self::default()
        }
    }

    pub fn read_saved<P: Deref<Target = impl Platform>>(platform: P) -> Self {
        let storage_file = platform.internal_dir().join("config");
        Self::read_from_file(storage_file)
    }

    #[cfg(target_os = "android")]
    pub fn music_dir() -> PathBuf {
        PathBuf::new()
    }

    #[cfg(not(target_os = "android"))]
    pub fn music_dir() -> PathBuf {
        if let Some(user_dirs) = directories::UserDirs::new() {
            return if let Some(music_dir) = user_dirs.audio_dir() {
                music_dir.into()
            } else {
                let path = user_dirs.home_dir().join("Music");
                if !path.exists() {
                    std::fs::create_dir(&path).unwrap();
                }
                path
            };
        }
        PathBuf::new()
    }

    pub async fn check_timestamp(&self) -> bool {
        if let Some(saved_timestamp) = &self.timestamp {
            if let Ok(timestamp) = self.timestamp().await {
                return &timestamp == saved_timestamp;
            }
        }
        false
    }

    pub async fn save_timestamp(&mut self) {
        if let Ok(timestamp) = self.timestamp().await {
            self.timestamp = Some(timestamp);
        }
    }

    pub async fn timestamp(&self) -> std::io::Result<u64> {
        let mut hasher = DefaultHasher::default();
        tokio::fs::metadata(&self.path)
            .await?
            .modified()?
            .hash(&mut hasher);
        Ok(hasher.finish())
    }

    pub async fn clear_tracks<P: Deref<Target = impl Platform>>(&self, platform: P) {
        let tracks_file = platform.internal_dir().join("tracks");
        if tracks_file.exists() {
            tokio::fs::remove_file(&tracks_file).await.unwrap();
        }
    }

    pub async fn add_tracks<P: Deref<Target = impl Platform>>(
        &self,
        platform: P,
        tracks: Vec<FileTrack>,
    ) {
        let tracks_file = platform.internal_dir().join("tracks");
        let data = bitcode::encode(&tracks);
        tokio::task::spawn_blocking(move || {
            if let Ok(file) = File::create(tracks_file) {
                zstd::stream::copy_encode(BufReader::new(Cursor::new(data)), file, 9).unwrap();
            }
        })
        .await
        .unwrap();
    }

    pub async fn read_tracks<P: Deref<Target = impl Platform>>(
        &self,
        platform: P,
    ) -> Vec<FileTrack> {
        let tracks_file = platform.internal_dir().join("tracks");

        tokio::task::spawn_blocking(|| {
            if tracks_file.exists() && tracks_file.is_file() {
                let mut data = vec![];
                if let Ok(_) = zstd::stream::copy_decode(
                    File::open(tracks_file).unwrap(),
                    BufWriter::new(Cursor::new(&mut data)),
                ) {
                    if let Ok(tracks) = bitcode::decode::<Vec<FileTrack>>(&data) {
                        tracks
                    } else {
                        eprintln!("not encoded");
                        vec![]
                    }
                } else {
                    eprintln!("bad file");
                    vec![]
                }
            } else {
                eprintln!("file not found");
                vec![]
            }
        })
        .await
        .unwrap()
    }

    pub async fn save<P: Deref<Target = impl Platform>>(&self, platform: P) {
        self.save_and_compress(platform.internal_dir()).await
    }

    async fn save_and_compress(&self, config_dir: PathBuf) {
        let storage_file = config_dir.join("config");
        if storage_file.exists() {
            tokio::fs::remove_file(&storage_file).await.unwrap();
        }
        let data = bitcode::encode(self);
        tokio::task::spawn_blocking(|| {
            zstd::stream::copy_encode(
                BufReader::new(Cursor::new(data)),
                File::create(storage_file).unwrap(),
                9,
            )
            .unwrap();
        })
        .await
        .unwrap();
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            path: Self::music_dir().to_str().unwrap().to_string(),
            volume: 1.0,
            theme: Theme::default(),
            window_size: WindowSize::default(),
            save_window_size: false,
            locale: None,
            timestamp: None,
        }
    }
}
