//! MPRIS integration: media keys, KDE's "now playing" widget, and mprisence (Discord status).
use crate::model::Track;
use crate::player::{Action, Msg};
use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};
use std::sync::mpsc::Sender;
use std::time::Duration;

pub struct Media {
    controls: MediaControls,
}

impl Media {
    pub fn new(tx: Sender<Msg>) -> Result<Self, String> {
        let mut controls = MediaControls::new(PlatformConfig {
            dbus_name: "crate",
            display_name: "crate",
            hwnd: None,
        })
        .map_err(|e| format!("{e:?}"))?;
        controls
            .attach(move |ev: MediaControlEvent| {
                let a = match ev {
                    MediaControlEvent::Play => Action::Play,
                    MediaControlEvent::Pause | MediaControlEvent::Stop => Action::Pause,
                    MediaControlEvent::Toggle => Action::Toggle,
                    MediaControlEvent::Next => Action::Next,
                    MediaControlEvent::Previous => Action::Prev,
                    MediaControlEvent::SetPosition(MediaPosition(d)) => Action::Seek { secs: d.as_secs_f64() },
                    MediaControlEvent::Seek(dir) => Action::SeekBy {
                        delta: if matches!(dir, SeekDirection::Forward) { 10.0 } else { -10.0 },
                    },
                    MediaControlEvent::SeekBy(dir, d) => Action::SeekBy {
                        delta: if matches!(dir, SeekDirection::Forward) { d.as_secs_f64() } else { -d.as_secs_f64() },
                    },
                    _ => return,
                };
                let _ = tx.send(Msg::Act(a));
            })
            .map_err(|e| format!("{e:?}"))?;
        Ok(Media { controls })
    }

    pub fn set_track(&mut self, t: Option<&Track>, cover_url: Option<&str>) {
        let _ = match t {
            Some(t) => self.controls.set_metadata(MediaMetadata {
                title: Some(&t.title),
                album: Some(&t.album),
                artist: Some(&t.artist),
                cover_url,
                duration: Some(Duration::from_secs_f64(t.duration.max(0.0))),
            }),
            None => self.controls.set_metadata(MediaMetadata::default()),
        };
    }

    pub fn set_status(&mut self, playing: bool, has_track: bool, position: f64) {
        let progress = Some(MediaPosition(Duration::from_secs_f64(position.max(0.0))));
        let _ = self.controls.set_playback(if !has_track {
            MediaPlayback::Stopped
        } else if playing {
            MediaPlayback::Playing { progress }
        } else {
            MediaPlayback::Paused { progress }
        });
    }
}
