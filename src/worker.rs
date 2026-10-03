//! Background lookups on a small thread pool, reporting back to the UI over a channel.

use std::fs::{self, File};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::SystemTime;

use eframe::egui;

use crate::history::{self, History, HistoryEntry, Record};
use crate::profile::{Alias, Client, FetchError, Profile};
use crate::steamid::Target;

pub struct Job {
    pub card: u64,
    pub target: Target,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub profile: Profile,
    /// Steam's previous-names list, newest first. Empty when hidden or unavailable.
    pub aliases: Vec<Alias>,
    pub avatar: Option<PathBuf>,
    /// The history entry after this lookup was recorded, carrying renames the app observed.
    pub entry: Option<HistoryEntry>,
    /// Why recording the lookup in the history failed, if it did.
    pub history_error: Option<String>,
}

pub struct Outcome {
    pub card: u64,
    pub result: Result<Resolved, FetchError>,
}

pub struct Pool {
    jobs: mpsc::Sender<Job>,
    outcomes: mpsc::Receiver<Outcome>,
}

impl Pool {
    pub fn new(ctx: egui::Context, history: History, threads: usize) -> Self {
        let (job_tx, job_rx) = mpsc::channel::<Job>();
        let (out_tx, out_rx) = mpsc::channel();
        let job_rx = Arc::new(Mutex::new(job_rx));
        let client = Client::default();
        for i in 0..threads {
            let (job_rx, out_tx) = (Arc::clone(&job_rx), out_tx.clone());
            let (ctx, client, history) = (ctx.clone(), client.clone(), history.clone());
            thread::Builder::new()
                .name(format!("lookup-{i}"))
                .spawn(move || {
                    loop {
                        let job = match job_rx.lock() {
                            Ok(rx) => rx.recv(),
                            Err(_) => return,
                        };
                        let Ok(job) = job else { return };
                        let result = lookup(&client, &history, &job.target);
                        if out_tx
                            .send(Outcome {
                                card: job.card,
                                result,
                            })
                            .is_err()
                        {
                            return;
                        }
                        ctx.request_repaint();
                    }
                })
                .expect("failed to spawn lookup thread");
        }
        Self {
            jobs: job_tx,
            outcomes: out_rx,
        }
    }

    pub fn submit(&self, job: Job) {
        // The workers only stop when the pool is dropped, so this can't fail while `self` lives.
        let _ = self.jobs.send(job);
    }

    pub fn try_recv(&self) -> Option<Outcome> {
        self.outcomes.try_recv().ok()
    }
}

fn lookup(client: &Client, history: &History, target: &Target) -> Result<Resolved, FetchError> {
    let profile = client.profile(target)?;
    let aliases = client.aliases(profile.id).unwrap_or_default();
    let avatar = profile
        .avatar_url
        .as_deref()
        .and_then(|url| cache_avatar(client, history, url));
    let avatar_file = avatar
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str());
    let recorded = history.record(
        Record {
            id64: profile.id.id64(),
            name: &profile.name,
            avatar_file,
            custom_url: profile.custom_url.as_deref(),
        },
        chrono::Utc::now().timestamp(),
    );
    let (entry, history_error) = match recorded {
        Ok(entry) => (Some(entry), None),
        Err(err) => (None, Some(err.to_string())),
    };
    Ok(Resolved {
        profile,
        aliases,
        avatar,
        entry,
        history_error,
    })
}

/// Returns the cached avatar for `url`, downloading it on first use.
fn cache_avatar(client: &Client, history: &History, url: &str) -> Option<PathBuf> {
    let name = history::avatar_file_name(url)?;
    let dir = history.avatar_dir();
    let path = dir.join(&name);
    if path.is_file() {
        // Refresh the timestamp so another window's avatar pruning leaves it alone until this
        // lookup is recorded.
        if let Ok(file) = File::options().write(true).open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        return Some(path);
    }
    let bytes = client.download(url).ok()?;
    fs::create_dir_all(&dir).ok()?;
    let thread = format!("{:?}", thread::current().id()).replace(['(', ')'], "");
    let tmp = dir.join(format!(".{name}.tmp-{}-{thread}", std::process::id()));
    fs::write(&tmp, bytes).ok()?;
    fs::rename(&tmp, &path).ok()?;
    Some(path)
}
