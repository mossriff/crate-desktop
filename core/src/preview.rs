//! 30-second previews of catalog songs you don't own yet (Deezer preview MP3s).
//!
//! Runs on its own audio thread with its own output, separate from the main player, so a
//! preview never disturbs the queue. The UI pauses the main player itself before previewing.
use rodio::{OutputStream, Sink};
use std::io::Cursor;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::Duration;

pub trait PreviewEvents: Send + 'static {
    /// `Some(id)` when a preview starts playing, `None` when it stops (ended, stopped or failed).
    fn previewing(&self, id: Option<u64>);
    fn failed(&self, id: u64, error: String);
}

enum Cmd {
    Play { id: u64, url: String, volume: f32 },
    Stop,
}

#[derive(Clone)]
pub struct Preview {
    tx: Sender<Cmd>,
}

impl Preview {
    pub fn spawn(events: impl PreviewEvents) -> Self {
        let (tx, rx) = channel::<Cmd>();
        std::thread::Builder::new()
            .name("preview".into())
            .spawn(move || {
                // the output is opened lazily: most sessions never preview anything
                let mut out: Option<(OutputStream, rodio::OutputStreamHandle)> = None;
                let mut cur: Option<(u64, Sink)> = None;
                // bumps on every command, so a slow download for an old request is dropped
                let mut gen: u64 = 0;
                let (loaded_tx, loaded_rx) = channel::<(u64, u64, f32, anyhow::Result<Vec<u8>>)>();
                loop {
                    match rx.recv_timeout(Duration::from_millis(150)) {
                        Ok(Cmd::Play { id, url, volume }) => {
                            gen += 1;
                            if let Some((_, s)) = cur.take() {
                                s.stop();
                            }
                            events.previewing(Some(id)); // show it as active while it loads
                            let (g, ltx) = (gen, loaded_tx.clone());
                            std::thread::spawn(move || {
                                let _ = ltx.send((g, id, volume, crate::http_get(&url)));
                            });
                        }
                        Ok(Cmd::Stop) => {
                            gen += 1;
                            if let Some((_, s)) = cur.take() {
                                s.stop();
                            }
                            events.previewing(None);
                        }
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                    // a download finished?
                    while let Ok((g, id, volume, bytes)) = loaded_rx.try_recv() {
                        if g != gen {
                            continue;
                        }
                        let started = (|| -> anyhow::Result<Sink> {
                            if out.is_none() {
                                out = Some(OutputStream::try_default()?);
                            }
                            let handle = &out.as_ref().unwrap().1;
                            let sink = Sink::try_new(handle)?;
                            sink.set_volume(volume);
                            sink.append(crate::decoder(Cursor::new(strip_id3(bytes?))).map_err(anyhow::Error::msg)?);
                            Ok(sink)
                        })();
                        match started {
                            Ok(s) => cur = Some((id, s)),
                            Err(e) => {
                                events.failed(id, format!("{e:#}"));
                                events.previewing(None);
                            }
                        }
                    }
                    // finished playing?
                    if cur.as_ref().map(|(_, s)| s.empty()).unwrap_or(false) {
                        cur = None;
                        events.previewing(None);
                    }
                }
            })
            .expect("preview thread");
        Preview { tx }
    }

    /// Play a preview (replacing any other). `volume` is linear 0..1.
    pub fn play(&self, id: u64, url: &str, volume: f32) {
        let _ = self.tx.send(Cmd::Play { id, url: url.to_string(), volume });
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

/// Drop a leading ID3v2 tag. Deezer previews start with an empty one that the MP3 decoder
/// rejects ("out of bounds"); the audio frames right after it decode fine.
fn strip_id3(b: Vec<u8>) -> Vec<u8> {
    if b.len() < 10 || &b[..3] != b"ID3" {
        return b;
    }
    let size = b[6..10].iter().fold(0usize, |acc, &x| (acc << 7) | (x & 0x7f) as usize);
    let footer = if b[5] & 0x10 != 0 { 10 } else { 0 };
    let skip = (10 + size + footer).min(b.len());
    b[skip..].to_vec()
}
