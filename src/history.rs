//! Persistent lookup history.
//!
//! Every mutation is a locked read-modify-write of `history.json` followed by an atomic
//! rename, so several open windows never overwrite each other's changes.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const MAX_ENTRIES: usize = 500;
const FILE_VERSION: u32 = 1;
const AVATAR_GRACE: Duration = Duration::from_secs(300);

/// A name change the app noticed between two lookups of the same profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenRename {
    pub from: String,
    pub to: String,
    /// Unix seconds.
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id64: u64,
    pub name: String,
    /// File name inside the avatar cache directory.
    #[serde(default)]
    pub avatar_file: Option<String>,
    #[serde(default)]
    pub custom_url: Option<String>,
    pub first_seen: i64,
    pub last_seen: i64,
    #[serde(default)]
    pub lookups: u32,
    /// Oldest first.
    #[serde(default)]
    pub renames: Vec<SeenRename>,
}

/// What a successful lookup contributes to the history.
pub struct Record<'a> {
    pub id64: u64,
    pub name: &'a str,
    pub avatar_file: Option<&'a str>,
    pub custom_url: Option<&'a str>,
}

#[derive(Default, Serialize, Deserialize)]
struct HistoryFile {
    version: u32,
    entries: Vec<HistoryEntry>,
}

#[derive(Debug, Clone)]
pub struct History {
    dir: PathBuf,
}

impl History {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `~/.local/share/steamidfinder` on Linux, `%APPDATA%\SteamIDfinder` on Windows.
    pub fn default_dir() -> Option<PathBuf> {
        let name = if cfg!(windows) {
            "SteamIDfinder"
        } else {
            "steamidfinder"
        };
        dirs::data_dir().map(|dir| dir.join(name))
    }

    pub fn avatar_dir(&self) -> PathBuf {
        self.dir.join("avatars")
    }

    fn history_path(&self) -> PathBuf {
        self.dir.join("history.json")
    }

    /// All entries, most recently looked up first. Missing or unreadable files yield an empty list.
    pub fn load(&self) -> Vec<HistoryEntry> {
        read_entries(&self.history_path()).unwrap_or_default()
    }

    /// Adds or refreshes the entry for a resolved profile and returns it.
    pub fn record(&self, record: Record<'_>, now: i64) -> io::Result<HistoryEntry> {
        self.mutate(|entries| {
            let mut entry = match entries.iter().position(|e| e.id64 == record.id64) {
                Some(i) => entries.remove(i),
                None => HistoryEntry {
                    id64: record.id64,
                    name: record.name.to_string(),
                    avatar_file: None,
                    custom_url: None,
                    first_seen: now,
                    last_seen: now,
                    lookups: 0,
                    renames: Vec::new(),
                },
            };
            if entry.name != record.name && !record.name.is_empty() && !entry.name.is_empty() {
                entry.renames.push(SeenRename {
                    from: std::mem::take(&mut entry.name),
                    to: record.name.to_string(),
                    at: now,
                });
                entry.name = record.name.to_string();
            } else if !record.name.is_empty() {
                entry.name = record.name.to_string();
            }
            if record.avatar_file.is_some() {
                entry.avatar_file = record.avatar_file.map(str::to_string);
            }
            entry.custom_url = record.custom_url.map(str::to_string);
            entry.last_seen = now;
            entry.lookups = entry.lookups.saturating_add(1);
            entries.insert(0, entry.clone());
            entries.truncate(MAX_ENTRIES);
            entry
        })
    }

    pub fn delete(&self, id64: u64) -> io::Result<()> {
        self.mutate(|entries| entries.retain(|e| e.id64 != id64))
    }

    pub fn clear(&self) -> io::Result<()> {
        self.mutate(Vec::clear)
    }

    fn mutate<R>(&self, change: impl FnOnce(&mut Vec<HistoryEntry>) -> R) -> io::Result<R> {
        fs::create_dir_all(&self.dir)?;
        let lock = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.dir.join("history.lock"))?;
        lock.lock()?;

        let path = self.history_path();
        let mut entries = match read_entries(&path) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(_) => {
                // Keep a corrupt file around instead of silently replacing it.
                let backup =
                    path.with_extension(format!("corrupt-{}.json", chrono::Utc::now().timestamp()));
                fs::rename(&path, backup)?;
                Vec::new()
            }
        };
        let result = change(&mut entries);
        write_entries(&path, &entries)?;
        self.prune_avatars(&entries);
        Ok(result)
    }

    /// Removes cached avatars no entry refers to any more. Recent files are kept: a worker may
    /// have downloaded one and not recorded its entry yet.
    fn prune_avatars(&self, entries: &[HistoryEntry]) {
        let keep: HashSet<&str> = entries
            .iter()
            .filter_map(|e| e.avatar_file.as_deref())
            .collect();
        let Ok(dir) = fs::read_dir(self.avatar_dir()) else {
            return;
        };
        for file in dir.flatten() {
            let orphaned = file.file_name().to_str().is_some_and(|n| !keep.contains(n));
            let recent = file
                .metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().unwrap_or_default() < AVATAR_GRACE);
            if orphaned && !recent {
                let _ = fs::remove_file(file.path());
            }
        }
    }
}

fn read_entries(path: &Path) -> io::Result<Vec<HistoryEntry>> {
    let text = fs::read_to_string(path)?;
    let file: HistoryFile =
        serde_json::from_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let mut entries = file.entries;
    entries.sort_by_key(|e| std::cmp::Reverse(e.last_seen));
    Ok(entries)
}

fn write_entries(path: &Path, entries: &[HistoryEntry]) -> io::Result<()> {
    let file = HistoryFile {
        version: FILE_VERSION,
        entries: entries.to_vec(),
    };
    let json = serde_json::to_string_pretty(&file).map_err(io::Error::other)?;
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    fs::write(&tmp, json)?;
    fs::rename(&tmp, path)
}

/// Cache file name for an avatar URL: its last path segment, restricted to safe characters.
pub fn avatar_file_name(url: &str) -> Option<String> {
    let name = url.split(['?', '#']).next()?.rsplit('/').next()?;
    let safe = !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    safe.then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;
    use std::time::SystemTime;

    fn rec<'a>(id64: u64, name: &'a str, avatar: Option<&'a str>) -> Record<'a> {
        Record {
            id64,
            name,
            avatar_file: avatar,
            custom_url: None,
        }
    }

    #[test]
    fn records_dedupes_and_tracks_renames() {
        let tmp = TempDir::new("renames");
        let history = History::new(tmp.0.clone());
        history.record(rec(1, "Alice", None), 100).unwrap();
        history.record(rec(2, "Bob", None), 200).unwrap();
        let alice = history.record(rec(1, "Alicia", None), 300).unwrap();

        assert_eq!(alice.lookups, 2);
        assert_eq!(alice.first_seen, 100);
        assert_eq!(
            alice.renames,
            [SeenRename {
                from: "Alice".into(),
                to: "Alicia".into(),
                at: 300
            }]
        );
        let ids: Vec<u64> = history.load().iter().map(|e| e.id64).collect();
        assert_eq!(ids, [1, 2], "most recent first, no duplicates");
    }

    #[test]
    fn delete_from_one_window_is_not_undone_by_another() {
        let tmp = TempDir::new("windows");
        let window_a = History::new(tmp.0.clone());
        let window_b = History::new(tmp.0.clone());
        window_a.record(rec(1, "Alice", None), 100).unwrap();
        window_a.record(rec(2, "Bob", None), 200).unwrap();
        window_b.delete(1).unwrap();
        window_a.record(rec(3, "Carol", None), 300).unwrap();
        let ids: Vec<u64> = window_b.load().iter().map(|e| e.id64).collect();
        assert_eq!(ids, [3, 2]);
    }

    #[test]
    fn caps_entries_and_prunes_orphaned_avatars() {
        let tmp = TempDir::new("cap");
        let history = History::new(tmp.0.clone());
        fs::create_dir_all(history.avatar_dir()).unwrap();
        let old = history.avatar_dir().join("old.jpg");
        fs::write(&old, b"x").unwrap();
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(SystemTime::now() - 2 * AVATAR_GRACE)
            .unwrap();
        fs::write(history.avatar_dir().join("fresh.jpg"), b"x").unwrap();
        history.record(rec(0, "First", Some("old.jpg")), 0).unwrap();
        for i in 1..=MAX_ENTRIES as u64 {
            history.record(rec(i, "n", None), i as i64).unwrap();
        }
        let entries = history.load();
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert!(entries.iter().all(|e| e.id64 != 0));
        assert!(!old.exists());
        assert!(
            history.avatar_dir().join("fresh.jpg").exists(),
            "recent downloads survive"
        );
    }

    #[test]
    fn corrupt_file_is_backed_up() {
        let tmp = TempDir::new("corrupt");
        let history = History::new(tmp.0.clone());
        fs::write(tmp.0.join("history.json"), "{not json").unwrap();
        assert!(history.load().is_empty());
        history.record(rec(1, "Alice", None), 1).unwrap();
        assert_eq!(history.load().len(), 1);
        let backups = fs::read_dir(&tmp.0)
            .unwrap()
            .flatten()
            .filter(|f| f.file_name().to_string_lossy().contains("corrupt"))
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn avatar_names_are_sanitised() {
        assert_eq!(
            avatar_file_name("https://avatars.fastly.steamstatic.com/c5d5_full.jpg").as_deref(),
            Some("c5d5_full.jpg")
        );
        assert_eq!(avatar_file_name("https://x/../..").as_deref(), None);
        assert_eq!(avatar_file_name("https://x/a b.jpg").as_deref(), None);
        assert_eq!(avatar_file_name("https://x/").as_deref(), None);
    }
}
