use chrono::Utc;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tauri::{Manager, State};
use walkdir::WalkDir;

mod audio;
mod network;
mod protocol;
mod tor;
mod transfer;

const CHUNK_SIZE: usize = 1024 * 1024;
const DEFAULT_NOSTR_RELAYS: &str = "wss://relay.damus.io,wss://nos.lol,wss://relay.nostr.com,wss://relay.primal.net,wss://relay.snort.social,wss://nostr.mom";
const LEGACY_DEFAULT_NOSTR_RELAYS: &str = "wss://relay.damus.io,wss://nos.lol";

struct AppState {
    db_path: Mutex<PathBuf>,
    network: Arc<network::NetworkService>,
    tor: Arc<tor::TorManager>,
    watcher: Mutex<Option<FolderWatcher>>,
}

struct FolderWatcher {
    _watcher: RecommendedWatcher,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SharedFile {
    file_id: String,
    filename: String,
    path: String,
    size: u64,
    format: String,
    chunk_count: usize,
    status: String,
    title: String,
    artist: String,
    album: String,
    mime: String,
    license: String,
    description: String,
    tags: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Transfer {
    id: i64,
    file_id: String,
    filename: String,
    size: u64,
    progress: f64,
    status: String,
    speed: String,
    destination: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    shared_folder: String,
    download_folder: String,
    nostr_relays: String,
    display_name: String,
    profile_about: String,
    profile_picture: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppSnapshot {
    files: Vec<SharedFile>,
    transfers: Vec<Transfer>,
    settings: Settings,
    indexed_bytes: u64,
    native: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IndexReport {
    file_count: usize,
    total_bytes: u64,
    errors: Vec<String>,
}

fn open_db(state: &State<'_, AppState>) -> Result<Connection, String> {
    let path = state
        .db_path
        .lock()
        .map_err(|_| "database lock poisoned")?
        .clone();
    open_connection(&path)
}

fn open_connection(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open(path).map_err(|error| error.to_string())?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    Ok(connection)
}

fn initialise_database(path: &Path, app_data: &Path) -> Result<(), String> {
    fs::create_dir_all(app_data).map_err(|error| error.to_string())?;
    let connection = open_connection(path)?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS files (
           file_id TEXT PRIMARY KEY,
           filename TEXT NOT NULL,
           path TEXT NOT NULL,
           size INTEGER NOT NULL,
           format TEXT NOT NULL,
           chunk_size INTEGER NOT NULL,
           chunk_hashes TEXT NOT NULL,
           indexed_at TEXT NOT NULL,
           title TEXT NOT NULL DEFAULT '', artist TEXT NOT NULL DEFAULT '', album TEXT NOT NULL DEFAULT '',
           mime TEXT NOT NULL DEFAULT 'application/octet-stream', license TEXT NOT NULL DEFAULT 'unspecified',
           description TEXT NOT NULL DEFAULT '', tags TEXT NOT NULL DEFAULT ''
         );
         CREATE TABLE IF NOT EXISTS transfers (
           id INTEGER PRIMARY KEY AUTOINCREMENT,
           file_id TEXT NOT NULL,
           filename TEXT NOT NULL,
           size INTEGER NOT NULL,
           progress REAL NOT NULL DEFAULT 0,
           status TEXT NOT NULL,
           speed TEXT NOT NULL DEFAULT '—',
           destination TEXT NOT NULL DEFAULT '',
           created_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS blocked_files (
           file_id TEXT PRIMARY KEY, reason TEXT NOT NULL, created_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS blocked_pubkeys (
           pubkey TEXT PRIMARY KEY, reason TEXT NOT NULL, created_at TEXT NOT NULL
         );"
    ).map_err(|error| error.to_string())?;
    for (column, declaration) in [
        ("title", "TEXT NOT NULL DEFAULT ''"),
        ("artist", "TEXT NOT NULL DEFAULT ''"),
        ("album", "TEXT NOT NULL DEFAULT ''"),
        ("mime", "TEXT NOT NULL DEFAULT 'application/octet-stream'"),
        ("license", "TEXT NOT NULL DEFAULT 'unspecified'"),
        ("description", "TEXT NOT NULL DEFAULT ''"),
        ("tags", "TEXT NOT NULL DEFAULT ''"),
    ] {
        ensure_column(&connection, "files", column, declaration)?;
    }
    network::initialise_network_schema(&connection)?;

    let downloads_path = app_data.join("Downloads");
    fs::create_dir_all(&downloads_path).map_err(|error| error.to_string())?;
    let downloads = downloads_path.to_string_lossy().into_owned();
    for (key, value) in [
        ("shared_folder", downloads.clone()),
        ("download_folder", downloads.clone()),
        ("nostr_relays", DEFAULT_NOSTR_RELAYS.to_string()),
        ("display_name", "napstr-user".to_string()),
        (
            "profile_about",
            "Sharing files privately with Napstr.".to_string(),
        ),
        ("profile_picture", "".to_string()),
    ] {
        connection
            .execute(
                "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(|error| error.to_string())?;
    }
    connection
        .execute(
            "UPDATE settings SET value=?1 WHERE key='shared_folder' AND trim(value)=''",
            [&downloads],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "UPDATE settings SET value=?1 WHERE key='nostr_relays' AND replace(value,' ','')=?2",
            params![DEFAULT_NOSTR_RELAYS, LEGACY_DEFAULT_NOSTR_RELAYS],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn get_setting(connection: &Connection, key: &str) -> Result<String, String> {
    connection
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .map_err(|error| error.to_string())
}

fn load_settings(connection: &Connection) -> Result<Settings, String> {
    Ok(Settings {
        shared_folder: get_setting(connection, "shared_folder")?,
        download_folder: get_setting(connection, "download_folder")?,
        nostr_relays: get_setting(connection, "nostr_relays")?,
        display_name: get_setting(connection, "display_name")?,
        profile_about: get_setting(connection, "profile_about")?,
        profile_picture: get_setting(connection, "profile_picture")?,
    })
}

fn load_files(connection: &Connection, query: Option<&str>) -> Result<Vec<SharedFile>, String> {
    let search = format!("%{}%", query.unwrap_or_default());
    let mut statement = connection.prepare(
        "SELECT file_id, filename, path, size, format, chunk_hashes, title, artist, album, mime, license, description, tags FROM files
         WHERE (?1 = '%%' OR lower(filename) LIKE lower(?1))
           AND format IN ('MP3','FLAC','WAV','OGG','OPUS')
           AND NOT EXISTS(SELECT 1 FROM blocked_files WHERE blocked_files.file_id=files.file_id)
         ORDER BY filename"
    ).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([search], |row| {
            let chunk_hashes: String = row.get(5)?;
            Ok(SharedFile {
                file_id: row.get(0)?,
                filename: row.get(1)?,
                path: row.get(2)?,
                size: row.get::<_, i64>(3)? as u64,
                format: row.get(4)?,
                chunk_count: chunk_hashes
                    .split(',')
                    .filter(|value| !value.is_empty())
                    .count(),
                status: "Published".into(),
                title: row.get(6)?,
                artist: row.get(7)?,
                album: row.get(8)?,
                mime: row.get(9)?,
                license: row.get(10)?,
                description: row.get(11)?,
                tags: row.get(12)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn load_transfers(connection: &Connection) -> Result<Vec<Transfer>, String> {
    let mut statement = connection.prepare("SELECT id, file_id, filename, size, progress, status, speed, destination FROM transfers ORDER BY id DESC").map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(Transfer {
                id: row.get(0)?,
                file_id: row.get(1)?,
                filename: row.get(2)?,
                size: row.get::<_, i64>(3)? as u64,
                progress: row.get(4)?,
                status: row.get(5)?,
                speed: row.get(6)?,
                destination: row.get(7)?,
            })
        })
        .map_err(|error| error.to_string())?;
    let mut transfers = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    transfers.extend(network::load_network_transfers(connection)?);
    Ok(transfers)
}

fn snapshot(connection: &Connection) -> Result<AppSnapshot, String> {
    let files = load_files(connection, None)?;
    let indexed_bytes = files.iter().map(|file| file.size).sum();
    Ok(AppSnapshot {
        files,
        transfers: load_transfers(connection)?,
        settings: load_settings(connection)?,
        indexed_bytes,
        native: true,
    })
}

fn ensure_column(
    connection: &Connection,
    table: &str,
    column: &str,
    declaration: &str,
) -> Result<(), String> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|error| error.to_string())?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if !columns.iter().any(|existing| existing == column) {
        connection
            .execute(
                &format!("ALTER TABLE {table} ADD COLUMN {column} {declaration}"),
                [],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<(String, Vec<String>, u64), String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    let size = file.metadata().map_err(|error| error.to_string())?.len();
    let mut reader = BufReader::new(file);
    let mut full_hasher = Sha256::new();
    let mut hashes = Vec::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        full_hasher.update(&buffer[..count]);
        hashes.push(hex::encode(Sha256::digest(&buffer[..count])));
    }
    Ok((hex::encode(full_hasher.finalize()), hashes, size))
}

fn index_path(connection: &mut Connection, folder: &Path) -> Result<IndexReport, String> {
    if !folder.is_dir() {
        return Err("The selected shared folder does not exist or is not a directory".into());
    }
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    transaction.execute_batch("DROP TABLE IF EXISTS temp.napstr_seen; CREATE TEMP TABLE napstr_seen(file_id TEXT PRIMARY KEY);").map_err(|error| error.to_string())?;
    let mut report = IndexReport {
        file_count: 0,
        total_bytes: 0,
        errors: Vec::new(),
    };
    for entry in WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.path();
        match audio::validate_audio(path)
            .and_then(|audio| hash_file(path).map(|hash| (audio, hash)))
        {
            Ok((audio, (file_id, chunk_hashes, size))) => {
                let blocked: bool = transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM blocked_files WHERE file_id=?1)",
                        [&file_id],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())?;
                if blocked {
                    report
                        .errors
                        .push(format!("{}: this file hash is blocked", path.display()));
                    continue;
                }
                let filename = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("Unnamed file");
                transaction.execute(
                    "INSERT INTO files (file_id, filename, path, size, format, chunk_size, chunk_hashes, indexed_at, mime)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                     ON CONFLICT(file_id) DO UPDATE SET filename=excluded.filename,path=excluded.path,size=excluded.size,
                     format=excluded.format,mime=excluded.mime,chunk_size=excluded.chunk_size,chunk_hashes=excluded.chunk_hashes,indexed_at=excluded.indexed_at",
                    params![file_id, filename, path.to_string_lossy(), size as i64, audio.format, CHUNK_SIZE as i64, chunk_hashes.join(","), Utc::now().to_rfc3339(), audio.mime]
                ).map_err(|error| error.to_string())?;
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO napstr_seen(file_id) VALUES (?1)",
                        [file_id],
                    )
                    .map_err(|error| error.to_string())?;
                report.file_count += 1;
                report.total_bytes += size;
            }
            Err(error) => report.errors.push(format!("{}: {}", path.display(), error)),
        }
    }
    transaction
        .execute(
            "DELETE FROM files WHERE file_id NOT IN (SELECT file_id FROM napstr_seen)",
            [],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DROP TABLE napstr_seen", [])
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(report)
}

fn start_folder_watcher(
    folder: PathBuf,
    db_path: PathBuf,
    network: Arc<network::NetworkService>,
) -> Result<FolderWatcher, String> {
    let (event_tx, event_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = event_tx.send(event);
    })
    .map_err(|error| error.to_string())?;
    watcher
        .watch(&folder, RecursiveMode::Recursive)
        .map_err(|error| error.to_string())?;
    std::thread::Builder::new()
        .name("napstr-folder-watch".into())
        .spawn(move || {
            while let Ok(event) = event_rx.recv() {
                let Ok(event) = event else { continue };
                if matches!(event.kind, EventKind::Access(_)) {
                    continue;
                }
                std::thread::sleep(std::time::Duration::from_millis(750));
                while event_rx.try_recv().is_ok() {}
                let Ok(mut connection) = open_connection(&db_path) else {
                    continue;
                };
                if index_path(&mut connection, &folder).is_ok() {
                    let network = network.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = network.publish_catalogue().await;
                    });
                }
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(FolderWatcher { _watcher: watcher })
}

#[tauri::command]
fn get_snapshot(state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    snapshot(&open_db(&state)?)
}

#[tauri::command]
fn search_catalog(query: String, state: State<'_, AppState>) -> Result<Vec<SharedFile>, String> {
    load_files(&open_db(&state)?, Some(&query))
}

#[tauri::command]
fn set_shared_folder(path: String, state: State<'_, AppState>) -> Result<IndexReport, String> {
    let folder = PathBuf::from(&path);
    let mut connection = open_db(&state)?;
    let report = index_path(&mut connection, &folder)?;
    connection
        .execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('shared_folder', ?1)",
            [path],
        )
        .map_err(|error| error.to_string())?;
    let db_path = state
        .db_path
        .lock()
        .map_err(|_| "database lock poisoned")?
        .clone();
    *state
        .watcher
        .lock()
        .map_err(|_| "folder watcher lock poisoned")? = Some(start_folder_watcher(
        folder,
        db_path,
        state.network.clone(),
    )?);
    Ok(report)
}

#[tauri::command]
fn rescan_shared_folder(state: State<'_, AppState>) -> Result<IndexReport, String> {
    let mut connection = open_db(&state)?;
    let folder = PathBuf::from(get_setting(&connection, "shared_folder")?);
    index_path(&mut connection, &folder)
}

#[tauri::command]
fn save_settings(settings: Settings, state: State<'_, AppState>) -> Result<AppSnapshot, String> {
    network::validate_profile_picture(&settings.profile_picture)?;
    validate_length("display name", &settings.display_name, 64)?;
    validate_length("profile about", &settings.profile_about, 500)?;
    validate_length("relay list", &settings.nostr_relays, 4096)?;
    let connection = open_db(&state)?;
    for (key, value) in [
        ("shared_folder", settings.shared_folder),
        ("download_folder", settings.download_folder),
        ("nostr_relays", settings.nostr_relays),
        ("display_name", settings.display_name),
        ("profile_about", settings.profile_about),
        ("profile_picture", settings.profile_picture),
    ] {
        connection
            .execute(
                "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
                params![key, value],
            )
            .map_err(|error| error.to_string())?;
    }
    snapshot(&connection)
}

fn unique_destination(folder: &Path, filename: &str) -> PathBuf {
    let filename = Path::new(filename)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty() && *value != "." && *value != "..")
        .unwrap_or("download.bin");
    let candidate = folder.join(filename);
    if !candidate.exists() {
        return candidate;
    }
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();
    for suffix in 1..10000 {
        let next = folder.join(format!("{stem} ({suffix}){extension}"));
        if !next.exists() {
            return next;
        }
    }
    folder.join(format!("{stem}-{}{}", Utc::now().timestamp(), extension))
}

#[tauri::command]
fn remove_transfer(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let connection = open_db(&state)?;
    if id < 0 {
        connection.execute("DELETE FROM download_chunks WHERE request_id=(SELECT request_id FROM network_downloads WHERE rowid=?1)", [-id]).map_err(|error| error.to_string())?;
        connection.execute("DELETE FROM download_sources WHERE request_id=(SELECT request_id FROM network_downloads WHERE rowid=?1)", [-id]).map_err(|error| error.to_string())?;
        connection
            .execute("DELETE FROM network_downloads WHERE rowid = ?1", [-id])
            .map_err(|error| error.to_string())?;
    } else {
        connection
            .execute("DELETE FROM transfers WHERE id = ?1", [id])
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
async fn set_downloads_paused(paused: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.network.transfers().set_paused(paused).await;
    Ok(())
}

#[tauri::command]
async fn cancel_transfer(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    if id < 0 {
        state.network.transfers().cancel_by_rowid(-id).await?;
    }
    Ok(())
}

#[tauri::command]
fn get_transfers(state: State<'_, AppState>) -> Result<Vec<Transfer>, String> {
    load_transfers(&open_db(&state)?)
}

#[tauri::command]
fn open_downloads_folder(state: State<'_, AppState>) -> Result<(), String> {
    let folder = PathBuf::from(get_setting(&open_db(&state)?, "download_folder")?);
    fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("explorer");
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command
        .arg(folder)
        .spawn()
        .map_err(|error| format!("could not open downloads folder: {error}"))?;
    Ok(())
}

fn launch_validated_audio(path: &Path, expected_file_id: &str) -> Result<(), String> {
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    let info = audio::validate_audio(&canonical)
        .map_err(|error| format!("playback rejected by the audio-only policy: {error}"))?;
    let (actual_file_id, _, _) = hash_file(&canonical)?;
    if actual_file_id != expected_file_id {
        return Err("playback rejected because the file changed after verification".into());
    }
    #[cfg(target_os = "windows")]
    let mut command = std::process::Command::new("explorer");
    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = std::process::Command::new("xdg-open");
    command.arg(&canonical).spawn().map_err(|error| {
        format!(
            "could not open {} audio in the system player: {error}",
            info.format
        )
    })?;
    Ok(())
}

#[tauri::command]
fn play_shared_audio(file_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let connection = open_db(&state)?;
    let path: Option<String> = connection.query_row(
        "SELECT path FROM files WHERE file_id=?1 AND NOT EXISTS(SELECT 1 FROM blocked_files WHERE file_id=?1)",
        [&file_id], |row| row.get(0),
    ).optional().map_err(|error| error.to_string())?;
    launch_validated_audio(
        Path::new(&path.ok_or("shared audio was not found")?),
        &file_id,
    )
}

#[tauri::command]
fn play_transfer_audio(id: i64, state: State<'_, AppState>) -> Result<(), String> {
    let connection = open_db(&state)?;
    let record: Option<(String, String, String)> = if id < 0 {
        connection
            .query_row(
                "SELECT file_id,destination,status FROM network_downloads WHERE rowid=?1",
                [-id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?
    } else {
        connection
            .query_row(
                "SELECT file_id,destination,status FROM transfers WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?
    };
    let (file_id, destination, status) = record.ok_or("completed transfer was not found")?;
    if status != "Verified · Complete" {
        return Err("audio can be played only after verification completes".into());
    }
    launch_validated_audio(Path::new(&destination), &file_id)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileMetadataUpdate {
    file_id: String,
    title: String,
    artist: String,
    album: String,
    #[serde(rename = "mime")]
    _mime: String,
    license: String,
    description: String,
    tags: String,
}

fn validate_length(label: &str, value: &str, maximum: usize) -> Result<(), String> {
    if value.chars().count() > maximum {
        Err(format!("{label} is longer than {maximum} characters"))
    } else {
        Ok(())
    }
}

#[tauri::command]
fn update_file_metadata(
    metadata: FileMetadataUpdate,
    state: State<'_, AppState>,
) -> Result<SharedFile, String> {
    for (label, value, maximum) in [
        ("title", metadata.title.as_str(), 256),
        ("artist", metadata.artist.as_str(), 256),
        ("album", metadata.album.as_str(), 256),
        ("license", metadata.license.as_str(), 128),
        ("description", metadata.description.as_str(), 2048),
        ("tags", metadata.tags.as_str(), 512),
    ] {
        validate_length(label, value, maximum)?;
    }
    let connection = open_db(&state)?;
    connection.execute(
        "UPDATE files SET title=?1,artist=?2,album=?3,license=?4,description=?5,tags=?6 WHERE file_id=?7",
        params![metadata.title.trim(), metadata.artist.trim(), metadata.album.trim(), metadata.license.trim(), metadata.description.trim(), metadata.tags.trim(), metadata.file_id],
    ).map_err(|error| error.to_string())?;
    load_files(&connection, None)?
        .into_iter()
        .find(|file| file.file_id == metadata.file_id)
        .ok_or("file ID was not found".into())
}

#[tauri::command]
async fn start_network(state: State<'_, AppState>) -> Result<network::NetworkStatus, String> {
    let mut status = state.network.start().await?;
    status.tor_running = state.tor.status().await;
    Ok(status)
}

#[tauri::command]
async fn network_status(state: State<'_, AppState>) -> Result<network::NetworkStatus, String> {
    let mut status = state.network.status().await?;
    status.tor_running = state.tor.status().await;
    Ok(status)
}

#[tauri::command]
async fn publish_catalogue(state: State<'_, AppState>) -> Result<usize, String> {
    state.network.publish_catalogue().await
}

#[tauri::command]
async fn publish_profile(state: State<'_, AppState>) -> Result<(), String> {
    state.network.publish_profile().await
}

#[tauri::command]
async fn network_search(
    query: String,
    state: State<'_, AppState>,
) -> Result<Vec<network::CatalogueResult>, String> {
    state.network.search(&query).await
}

#[tauri::command]
async fn request_network_download(
    file_id: String,
    source_pubkeys: Vec<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state
        .network
        .request_download(file_id, source_pubkeys)
        .await
}

#[tauri::command]
async fn block_file(file_id: String, state: State<'_, AppState>) -> Result<(), String> {
    if hex::decode(&file_id)
        .map(|bytes| bytes.len() == 32)
        .unwrap_or(false)
        == false
    {
        return Err("invalid SHA-256 file ID".into());
    }
    let connection = open_db(&state)?;
    connection.execute(
        "INSERT OR REPLACE INTO blocked_files(file_id,reason,created_at) VALUES(?1,'Blocked by user',?2)",
        params![file_id, Utc::now().to_rfc3339()],
    ).map_err(|error| error.to_string())?;
    connection
        .execute("DELETE FROM remote_catalogue WHERE file_id=?1", [&file_id])
        .map_err(|error| error.to_string())?;
    drop(connection);
    if state.network.status().await?.connected {
        state.network.publish_catalogue().await?;
    }
    Ok(())
}

#[tauri::command]
fn block_user(pubkey: String, state: State<'_, AppState>) -> Result<(), String> {
    nostr_sdk::PublicKey::from_hex(&pubkey).map_err(|_| "invalid Nostr public key")?;
    let connection = open_db(&state)?;
    connection.execute(
        "INSERT OR REPLACE INTO blocked_pubkeys(pubkey,reason,created_at) VALUES(?1,'Blocked by user',?2)",
        params![pubkey, Utc::now().to_rfc3339()],
    ).map_err(|error| error.to_string())?;
    connection
        .execute(
            "DELETE FROM remote_catalogue WHERE source_pubkey=?1",
            [&pubkey],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tauri::command]
async fn report_catalogue(
    file_id: String,
    source_pubkey: String,
    event_id: String,
    report_type: String,
    reason: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .network
        .report_catalogue(file_id, source_pubkey, event_id, report_type, reason)
        .await
}

#[tauri::command]
fn minimise_window(window: tauri::Window) -> Result<(), String> {
    window.minimize().map_err(|error| error.to_string())
}
#[tauri::command]
fn toggle_maximise(window: tauri::Window) -> Result<(), String> {
    let maximised = window.is_maximized().map_err(|error| error.to_string())?;
    if maximised {
        window.unmaximize()
    } else {
        window.maximize()
    }
    .map_err(|error| error.to_string())
}
#[tauri::command]
async fn close_window(window: tauri::Window, state: State<'_, AppState>) -> Result<(), String> {
    state.network.stop().await;
    state.tor.stop().await;
    window.close().map_err(|error| error.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data = app
                .path()
                .app_data_dir()
                .map_err(|error| error.to_string())?;
            let resource_dir = app
                .path()
                .resource_dir()
                .map_err(|error| error.to_string())?;
            let db_path = app_data.join("napstr.sqlite3");
            initialise_database(&db_path, &app_data)?;
            let tor = Arc::new(tor::TorManager::new(app_data, resource_dir));
            let transfers = Arc::new(transfer::TransferService::new(db_path.clone(), tor.clone()));
            let network = network::NetworkService::new(db_path.clone(), transfers);
            let existing_folder = open_connection(&db_path)
                .ok()
                .and_then(|connection| get_setting(&connection, "shared_folder").ok())
                .map(PathBuf::from);
            let watcher = existing_folder.and_then(|folder| {
                if !folder.is_dir() {
                    if let Ok(connection) = open_connection(&db_path) {
                        let _ = connection.execute("DELETE FROM files", []);
                    }
                    return None;
                }
                if let Ok(mut connection) = open_connection(&db_path) {
                    let _ = index_path(&mut connection, &folder);
                }
                start_folder_watcher(folder, db_path.clone(), network.clone()).ok()
            });
            app.manage(AppState {
                db_path: Mutex::new(db_path),
                network,
                tor,
                watcher: Mutex::new(watcher),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            search_catalog,
            set_shared_folder,
            rescan_shared_folder,
            save_settings,
            remove_transfer,
            get_transfers,
            open_downloads_folder,
            play_shared_audio,
            play_transfer_audio,
            set_downloads_paused,
            cancel_transfer,
            update_file_metadata,
            start_network,
            network_status,
            publish_catalogue,
            publish_profile,
            network_search,
            request_network_download,
            block_file,
            block_user,
            report_catalogue,
            minimise_window,
            toggle_maximise,
            close_window
        ])
        .run(tauri::generate_context!())
        .expect("error while running Napstr");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("napstr-{name}-{}", std::process::id()))
    }

    #[test]
    fn hashes_file_and_chunks_deterministically() {
        let directory = test_directory("hash-test");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("hello.txt");
        fs::write(&path, b"abc").unwrap();
        let (file_hash, chunks, size) = hash_file(&path).unwrap();
        assert_eq!(
            file_hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(chunks, vec![file_hash.clone()]);
        assert_eq!(size, 3);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recursive_index_keeps_only_valid_audio() {
        let directory = test_directory("audio-index-test");
        let child = directory.join("artist/album");
        fs::create_dir_all(&child).unwrap();
        let audio = child.join("track.wav");
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0\x44\xac\0\0\x88\x58\x01\0\x02\0\x10\0data\x04\0\0\0song");
        fs::write(audio, bytes).unwrap();
        fs::write(directory.join("renamed-video.mp3"), b"not audio").unwrap();
        fs::write(child.join("cover.jpg"), b"image").unwrap();
        let db_path = directory.join("napstr.sqlite3");
        initialise_database(&db_path, &directory).unwrap();
        let mut connection = open_connection(&db_path).unwrap();
        let report = index_path(&mut connection, &directory).unwrap();
        assert_eq!(report.file_count, 1);
        assert!(report.errors.len() >= 2);
        assert_eq!(load_files(&connection, None).unwrap().len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fresh_install_uses_one_download_and_share_folder() {
        let directory = test_directory("default-folder-test");
        let db_path = directory.join("napstr.sqlite3");
        initialise_database(&db_path, &directory).unwrap();
        let connection = open_connection(&db_path).unwrap();
        let settings = load_settings(&connection).unwrap();
        assert_eq!(settings.shared_folder, settings.download_folder);
        assert_eq!(
            Path::new(&settings.shared_folder),
            directory.join("Downloads")
        );
        assert!(Path::new(&settings.shared_folder).is_dir());
        assert_eq!(settings.nostr_relays, DEFAULT_NOSTR_RELAYS);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_default_relays_expand_without_overwriting_custom_relays() {
        let directory = test_directory("default-relay-migration-test");
        let db_path = directory.join("napstr.sqlite3");
        initialise_database(&db_path, &directory).unwrap();
        let connection = open_connection(&db_path).unwrap();
        connection
            .execute(
                "UPDATE settings SET value=?1 WHERE key='nostr_relays'",
                [LEGACY_DEFAULT_NOSTR_RELAYS],
            )
            .unwrap();
        drop(connection);
        initialise_database(&db_path, &directory).unwrap();
        let connection = open_connection(&db_path).unwrap();
        assert_eq!(
            get_setting(&connection, "nostr_relays").unwrap(),
            DEFAULT_NOSTR_RELAYS
        );
        connection
            .execute(
                "UPDATE settings SET value='wss://my-relay.example' WHERE key='nostr_relays'",
                [],
            )
            .unwrap();
        drop(connection);
        initialise_database(&db_path, &directory).unwrap();
        let connection = open_connection(&db_path).unwrap();
        assert_eq!(
            get_setting(&connection, "nostr_relays").unwrap(),
            "wss://my-relay.example"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
