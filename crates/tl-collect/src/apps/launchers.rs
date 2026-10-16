//! Launcher and wrapper process detection (PROC-011).
//!
//! Scope grouping puts every game under its launcher app. These helpers identify the game a
//! process belongs to: Steam games by the `reaper` process (`SteamLaunch AppId=<n>`), Lutris
//! and Heroic games by one environment variable. The caller reads `environ` only for processes
//! of the current uid. Environment values other than the two game variables are never kept.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::apps::naming::exe_basename;
use crate::proc::detail::EnvInfo;
use crate::sanitize::sanitize_text;

/// Manifests larger than this are read only up to this many bytes.
const MANIFEST_MAX: u64 = 1024 * 1024;
/// Limit for the sanitized game name, in bytes.
const NAME_MAX: usize = 4096;

/// Identity of a game under a launcher.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum GameId {
    Steam(u32),
    Lutris(Arc<str>),
    Heroic(Arc<str>),
}

/// A game row: display name and icon theme name.
#[derive(Clone, Debug, PartialEq)]
pub struct Game {
    pub id: GameId,
    pub name: Arc<str>,
    pub icon_name: Option<Arc<str>>,
}

/// Parses `AppId=<n>`: 1 to 10 ASCII digits that fit in a `u32`.
fn parse_app_id(token: &str) -> Option<u32> {
    let digits = token.strip_prefix("AppId=")?;
    if digits.is_empty() || digits.len() > 10 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Returns the Steam app id when this is a `reaper` process launched with `SteamLaunch`.
///
/// `argv` is tried first; `cmdline` (space separated) is the fallback for empty or truncated argv.
pub fn steam_app_id(argv: &[Arc<str>], cmdline: &str) -> Option<u32> {
    let program = match argv.first() {
        Some(first) => exe_basename(first),
        None => exe_basename(cmdline.split(' ').next().unwrap_or("")),
    };
    if program != "reaper" {
        return None;
    }
    let launch = argv.iter().any(|t| &**t == "SteamLaunch")
        || cmdline.split(' ').any(|t| t == "SteamLaunch");
    if !launch {
        return None;
    }
    argv.iter()
        .find(|t| t.starts_with("AppId="))
        .and_then(|t| parse_app_id(t))
        .or_else(|| {
            cmdline
                .split(' ')
                .find(|t| t.starts_with("AppId="))
                .and_then(parse_app_id)
        })
}

/// One token of the KeyValues text format.
enum Tok {
    Str(Vec<u8>),
    Open,
    Close,
}

fn next_tok(acf: &[u8], mut pos: usize) -> Option<(Tok, usize)> {
    loop {
        let b = *acf.get(pos)?;
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => pos += 1,
            b'/' if acf.get(pos + 1) == Some(&b'/') => {
                while acf.get(pos).is_some_and(|&c| c != b'\n') {
                    pos += 1;
                }
            }
            b'{' => return Some((Tok::Open, pos + 1)),
            b'}' => return Some((Tok::Close, pos + 1)),
            b'"' => {
                pos += 1;
                let mut s = Vec::new();
                loop {
                    let c = *acf.get(pos)?;
                    pos += 1;
                    match c {
                        b'"' => return Some((Tok::Str(s), pos)),
                        b'\\' => {
                            s.push(*acf.get(pos)?);
                            pos += 1;
                        }
                        _ => s.push(c),
                    }
                }
            }
            // Unquoted junk: skip it.
            _ => pos += 1,
        }
    }
}

/// Returns the sanitized `"name"` value at the top level of an `appmanifest_<n>.acf`
/// (directly inside `"AppState"`). Nested blocks are ignored.
pub fn steam_manifest_name(acf: &[u8]) -> Option<Arc<str>> {
    let mut depth = 0usize;
    let mut pos = 0;
    let mut key: Option<Vec<u8>> = None;
    while let Some((tok, next)) = next_tok(acf, pos) {
        pos = next;
        match tok {
            Tok::Open => {
                depth += 1;
                key = None;
            }
            Tok::Close => {
                depth = depth.saturating_sub(1);
                key = None;
            }
            Tok::Str(s) => match key.take() {
                None => key = Some(s),
                Some(k) => {
                    if depth == 1 && k == b"name" {
                        let mut out = String::new();
                        sanitize_text(&s, NAME_MAX, &mut out);
                        return (!out.is_empty()).then(|| Arc::from(out));
                    }
                }
            },
        }
    }
    None
}

/// Reads at most [`MANIFEST_MAX`] bytes of `path` into `buf`. False when it cannot be read.
fn read_capped(path: &Path, buf: &mut Vec<u8>) -> bool {
    buf.clear();
    match File::open(path) {
        Ok(f) => f.take(MANIFEST_MAX).read_to_end(buf).is_ok(),
        Err(_) => false,
    }
}

/// Looks up a Steam game in `<steam_root>/steamapps/appmanifest_<app_id>.acf`.
/// A missing or unreadable manifest gives `None`.
pub fn steam_game(app_id: u32, steam_root: &Path, buf: &mut Vec<u8>) -> Option<Game> {
    let path = steam_root
        .join("steamapps")
        .join(format!("appmanifest_{app_id}.acf"));
    if !read_capped(&path, buf) {
        return None;
    }
    let name = steam_manifest_name(buf)?;
    Some(Game {
        id: GameId::Steam(app_id),
        name,
        icon_name: Some(Arc::from(format!("steam_icon_{app_id}"))),
    })
}

/// Returns the Lutris or Heroic game named by the environment. The first matching variable with
/// a non-empty value wins.
pub fn env_game(environ: &EnvInfo) -> Option<GameId> {
    for (name, value) in &environ.vars {
        if value.is_empty() {
            continue;
        }
        match &**name {
            "LUTRIS_GAME_UUID" => return Some(GameId::Lutris(Arc::from(&**value))),
            "HEROIC_APP_NAME" => return Some(GameId::Heroic(Arc::from(&**value))),
            _ => {}
        }
    }
    None
}

/// Gives every descendant of a root pid (the root included) the root's game id. A process
/// under two roots takes the nearest. `out` is cleared first.
pub fn assign_games(
    records: &[(u32, u32)],
    roots: &FxHashMap<u32, GameId>,
    out: &mut FxHashMap<u32, GameId>,
) {
    out.clear();
    if roots.is_empty() {
        return;
    }
    let parents: FxHashMap<u32, u32> = records.iter().copied().collect();
    for &(pid, _) in records {
        let mut cur = pid;
        for _ in 0..records.len() {
            if let Some(game) = roots.get(&cur) {
                out.insert(pid, game.clone());
                break;
            }
            match parents.get(&cur) {
                Some(&ppid) if ppid != 0 && ppid != cur => cur = ppid,
                _ => break,
            }
        }
    }
}
