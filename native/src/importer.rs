//! State and background work for "Import from Spotify" (page: shell/import_ui.rs).
use crate::model::Model;
use crate_core::import::{self, ImportResult, PlaylistPlan};
use gpui::Context;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub enum Importing {
    #[default]
    Idle,
    Reading,
    /// plans, which ones are ticked, files that couldn't be read
    Preview(Arc<Vec<PlaylistPlan>>, Vec<bool>, Arc<Vec<String>>),
    Running,
    Done(Arc<Vec<(String, ImportResult)>>),
    Failed(String),
}

impl Importing {
    pub fn working(&self) -> bool {
        matches!(self, Importing::Reading | Importing::Running)
    }
}

/// Status line while reading or importing.
pub type ImportProgress = Arc<Mutex<String>>;

impl Model {
    /// Read the chosen CSV files and work out what each would add.
    pub fn import_read(&mut self, files: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.import.working() || files.is_empty() {
            return;
        }
        self.import = Importing::Reading;
        *self.import_progress.lock().unwrap() = "Reading your library\u{2026}".into();
        cx.notify();
        let (client, prog) = (self.client.clone(), self.import_progress.clone());
        cx.spawn(async move |this, cx| {
            let r = cx
                .background_executor()
                .spawn(async move {
                    let idx = import::LibraryIndex::load(&client)?;
                    let (mut plans, mut skipped) = (vec![], vec![]);
                    let mut files = files;
                    files.sort();
                    for (i, f) in files.iter().enumerate() {
                        *prog.lock().unwrap() = format!("Matching playlist {} of {}\u{2026}", i + 1, files.len());
                        match import::read_csv(f).and_then(|pl| import::plan(&client, &idx, pl)) {
                            Ok(p) if !p.songs.is_empty() => plans.push(p),
                            Ok(p) => skipped.push(format!("{}: no songs in it", p.name)),
                            Err(e) => skipped.push(format!("{e:#}")),
                        }
                    }
                    anyhow::Ok((plans, skipped))
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                m.import = match r {
                    Ok((plans, skipped)) if plans.is_empty() => Importing::Failed(skipped.first().cloned().unwrap_or_else(|| "No playlists found in those files.".into())),
                    Ok((plans, skipped)) => Importing::Preview(Arc::new(plans.clone()), vec![true; plans.len()], Arc::new(skipped)),
                    Err(e) => Importing::Failed(format!("{e:#}")),
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub fn import_toggle(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Importing::Preview(_, picks, _) = &mut self.import {
            if let Some(p) = picks.get_mut(i) {
                *p = !*p;
                cx.notify();
            }
        }
    }

    pub fn import_reset(&mut self, cx: &mut Context<Self>) {
        if !self.import.working() {
            self.import = Importing::Idle;
            cx.notify();
        }
    }

    /// Make the ticked playlists, then queue the songs you don't have for download into them.
    pub fn import_run(&mut self, cx: &mut Context<Self>) {
        let Importing::Preview(plans, picks, _) = &self.import else { return };
        let chosen: Vec<PlaylistPlan> = plans.iter().zip(picks).filter(|(_, on)| **on).map(|(p, _)| p.clone()).collect();
        if chosen.is_empty() {
            return;
        }
        if crate::onboarding::demo() {
            return self.toast("Demo: nothing was imported", false, cx);
        }
        self.import = Importing::Running;
        cx.notify();
        let (client, dz, prog) = (self.client.clone(), self.dz.clone(), self.import_progress.clone());
        cx.spawn(async move |this, cx| {
            let results = cx
                .background_executor()
                .spawn(async move {
                    let n = chosen.len();
                    let mut out = vec![];
                    for (k, p) in chosen.iter().enumerate() {
                        let name = p.name.clone();
                        let prog2 = prog.clone();
                        *prog.lock().unwrap() = format!("{name} ({} of {n})\u{2026}", k + 1);
                        let r = import::run(&client, &dz, p, &move |i, total| {
                            *prog2.lock().unwrap() = format!("{name} ({} of {n}): looking up songs you don\u{2019}t have, {i} of {total}\u{2026}", k + 1);
                        });
                        // one playlist failing (e.g. one Navidrome won't let be edited) doesn't stop the rest
                        out.push((p.name.clone(), r.unwrap_or_else(|e| import::ImportResult { error: Some(format!("{e:#}")), ..Default::default() })));
                    }
                    anyhow::Ok(out)
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                match results {
                    Ok(results) => {
                        if let Some(d) = &m.downloads {
                            for (_, r) in &results {
                                if !r.queued.is_empty() && !r.playlist_id.is_empty() {
                                    d.add(r.queued.clone(), Some(r.playlist_id.clone()));
                                }
                            }
                        }
                        // playlists that were added to have new songs; drop their cached copies
                        for (_, r) in &results {
                            m.cache.remove(&r.playlist_id);
                        }
                        m.import = Importing::Done(Arc::new(results));
                    }
                    Err(e) => m.import = Importing::Failed(format!("{e:#}")),
                }
                m.reload_playlists(cx);
                cx.notify();
            });
        })
        .detach();
    }
}
