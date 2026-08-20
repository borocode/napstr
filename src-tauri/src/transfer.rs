use crate::{
    protocol::{read_frame, write_frame, ClientFrame, ServerFrame, PROTOCOL_VERSION},
    tor::{is_v3_onion, OnionLease, TorManager},
};
use chrono::Utc;
use rand::RngCore;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    fs::{self, File},
    io::{
        AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt, BufReader, BufWriter,
        SeekFrom,
    },
    net::TcpListener,
    sync::{watch, Mutex, RwLock},
    task::JoinHandle,
    time::timeout,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadOffer {
    pub request_id: String,
    pub file_id: String,
    pub onion: String,
    pub port: u16,
    pub capability: String,
    pub expires_at: i64,
}

struct SessionGrant {
    file_id: String,
    requester: String,
    expires_at: i64,
    _onion_lease: Option<Arc<OnionLease>>,
}

struct ListenerRuntime {
    port: u16,
    _task: JoinHandle<()>,
}

pub struct TransferService {
    db_path: PathBuf,
    tor: Arc<TorManager>,
    grants: Arc<RwLock<HashMap<String, SessionGrant>>>,
    listener: Mutex<Option<ListenerRuntime>>,
    active: Arc<Mutex<HashMap<String, Arc<DownloadCoordinator>>>>,
    globally_paused: AtomicBool,
}

struct DownloadCoordinator {
    cancel: CancellationToken,
    paused: watch::Sender<bool>,
    claims: Mutex<HashSet<usize>>,
    verified: Mutex<HashSet<usize>>,
    manifest: Mutex<Option<PeerManifest>>,
    destination: Mutex<Option<PathBuf>>,
    workers: AtomicUsize,
    finalizing: AtomicBool,
    complete: AtomicBool,
}

#[derive(Clone, PartialEq, Eq)]
struct PeerManifest {
    filename: String,
    size: u64,
    chunk_size: u32,
    chunk_hashes: Vec<String>,
}

impl DownloadCoordinator {
    fn new(initially_paused: bool) -> Arc<Self> {
        let (paused, _) = watch::channel(initially_paused);
        Arc::new(Self {
            cancel: CancellationToken::new(),
            paused,
            claims: Mutex::new(HashSet::new()),
            verified: Mutex::new(HashSet::new()),
            manifest: Mutex::new(None),
            destination: Mutex::new(None),
            workers: AtomicUsize::new(0),
            finalizing: AtomicBool::new(false),
            complete: AtomicBool::new(false),
        })
    }
}

impl TransferService {
    pub fn new(db_path: PathBuf, tor: Arc<TorManager>) -> Self {
        Self {
            db_path,
            tor,
            grants: Arc::new(RwLock::new(HashMap::new())),
            listener: Mutex::new(None),
            active: Arc::new(Mutex::new(HashMap::new())),
            globally_paused: AtomicBool::new(false),
        }
    }

    pub async fn create_offer(
        &self,
        request_id: String,
        file_id: String,
        requester: String,
    ) -> Result<DownloadOffer, String> {
        let connection = crate::open_connection(&self.db_path)?;
        let path: Option<String> = connection
            .query_row(
                "SELECT path FROM files WHERE file_id = ?1 AND NOT EXISTS(SELECT 1 FROM blocked_files WHERE file_id=?1)",
                [&file_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let requester_blocked: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM blocked_pubkeys WHERE pubkey=?1)",
                [&requester],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if requester_blocked {
            return Err("requester is blocked".into());
        }
        let Some(path) = path else {
            return Err("requested file ID is not currently shared".into());
        };
        crate::audio::validate_audio(Path::new(&path))
            .map_err(|error| format!("shared file failed the audio-only policy: {error}"))?;
        {
            let now = Utc::now().timestamp();
            let mut grants = self.grants.write().await;
            grants.retain(|_, grant| grant.expires_at > now);
            if grants.len() >= 64 {
                return Err(
                    "this peer is already serving the maximum number of private offers".into(),
                );
            }
            if grants
                .values()
                .any(|grant| grant.file_id == file_id && grant.requester == requester)
            {
                return Err("an offer for this requester and file is already active".into());
            }
        }

        let port = self.ensure_listener().await?;
        let onion_lease = self.tor.create_onion(port).await?;
        let mut random = [0u8; 32];
        rand::rng().fill_bytes(&mut random);
        let capability = hex::encode(random);
        let key = capability_key(&capability);
        let expires_at = Utc::now().timestamp() + 15 * 60;
        self.grants.write().await.insert(
            key,
            SessionGrant {
                file_id: file_id.clone(),
                requester,
                expires_at,
                _onion_lease: Some(onion_lease.clone()),
            },
        );
        let grants = self.grants.clone();
        let expiring_key = capability_key(&capability);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(15 * 60 + 1)).await;
            grants.write().await.remove(&expiring_key);
        });
        Ok(DownloadOffer {
            request_id,
            file_id,
            onion: onion_lease.onion.clone(),
            port: 80,
            capability,
            expires_at,
        })
    }

    async fn ensure_listener(&self) -> Result<u16, String> {
        let mut guard = self.listener.lock().await;
        if let Some(runtime) = guard.as_ref() {
            return Ok(runtime.port);
        }
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| error.to_string())?;
        let port = listener
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let db_path = self.db_path.clone();
        let grants = self.grants.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let db_path = db_path.clone();
                let grants = grants.clone();
                tokio::spawn(async move {
                    let _ = serve_connection(stream, &db_path, grants).await;
                });
            }
        });
        *guard = Some(ListenerRuntime { port, _task: task });
        Ok(port)
    }

    pub async fn accept_offer(
        &self,
        offer: DownloadOffer,
        source_pubkey: String,
    ) -> Result<(), String> {
        if offer.expires_at <= Utc::now().timestamp() {
            return Err("download offer has expired".into());
        }
        if !is_v3_onion(&offer.onion) {
            return Err("refusing a download offer without a valid Tor v3 onion".into());
        }
        let status: Option<String> = crate::open_connection(&self.db_path)?
            .query_row(
                "SELECT status FROM network_downloads WHERE request_id=?1",
                [&offer.request_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if matches!(
            status.as_deref(),
            None | Some("Verified · Complete") | Some("Cancelled")
        ) {
            return Ok(());
        }
        let db_path = self.db_path.clone();
        let tor = self.tor.clone();
        let active = self.active.clone();
        let request_id = offer.request_id.clone();
        let coordinator = {
            let mut active = self.active.lock().await;
            let initially_paused = self.globally_paused.load(Ordering::SeqCst);
            active
                .entry(request_id.clone())
                .or_insert_with(|| DownloadCoordinator::new(initially_paused))
                .clone()
        };
        coordinator.workers.fetch_add(1, Ordering::SeqCst);
        let pause_rx = coordinator.paused.subscribe();
        tokio::spawn(async move {
            let result = download_offer(
                &db_path,
                tor,
                &offer,
                &source_pubkey,
                coordinator.clone(),
                pause_rx,
            )
            .await;
            let remaining = coordinator.workers.fetch_sub(1, Ordering::SeqCst) - 1;
            let source_status = match &result {
                Ok(_) => "Complete".to_string(),
                Err(error) => format!("Failed: {error}"),
            };
            if let Ok(connection) = crate::open_connection(&db_path) {
                let _ = connection.execute("UPDATE download_sources SET status=?1,updated_at=?2 WHERE request_id=?3 AND source_pubkey=?4", params![source_status, Utc::now().to_rfc3339(), offer.request_id, source_pubkey]);
            }
            if let Err(error) = result {
                if remaining == 0 && !coordinator.complete.load(Ordering::SeqCst) {
                    let status = if coordinator.cancel.is_cancelled() {
                        "Cancelled".to_string()
                    } else {
                        format!("Failed: {error}")
                    };
                    let current = current_progress(&db_path, &offer.request_id).unwrap_or(0.0);
                    let _ =
                        update_download(&db_path, &offer.request_id, current, &status, "—", None);
                }
            }
            if remaining == 0 {
                active.lock().await.remove(&request_id);
            }
        });
        Ok(())
    }

    pub async fn set_paused(&self, paused: bool) {
        self.globally_paused.store(paused, Ordering::SeqCst);
        for transfer in self.active.lock().await.values() {
            let _ = transfer.paused.send(paused);
        }
    }

    pub async fn cancel_by_rowid(&self, rowid: i64) -> Result<(), String> {
        let connection = crate::open_connection(&self.db_path)?;
        let request_id: Option<String> = connection
            .query_row(
                "SELECT request_id FROM network_downloads WHERE rowid=?1",
                [rowid],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some(request_id) = request_id {
            if let Some(transfer) = self.active.lock().await.get(&request_id) {
                transfer.cancel.cancel();
            }
            let progress = current_progress(&self.db_path, &request_id).unwrap_or(0.0);
            update_download(&self.db_path, &request_id, progress, "Cancelled", "—", None)?;
        }
        Ok(())
    }
}

fn capability_key(capability: &str) -> String {
    hex::encode(Sha256::digest(capability.as_bytes()))
}

async fn serve_connection<S>(
    mut stream: S,
    db_path: &Path,
    grants: Arc<RwLock<HashMap<String, SessionGrant>>>,
) -> Result<(), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let hello: ClientFrame = timeout(Duration::from_secs(30), read_frame(&mut stream))
        .await
        .map_err(|_| "HELLO timed out".to_string())??;
    let (capability, file_id) = match hello {
        ClientFrame::Hello {
            version,
            capability,
            file_id,
        } if version == PROTOCOL_VERSION => (capability, file_id),
        _ => {
            write_frame(
                &mut stream,
                &ServerFrame::Error {
                    code: "BAD_HELLO".into(),
                    message: "protocol negotiation failed".into(),
                },
            )
            .await?;
            return Err("invalid HELLO".into());
        }
    };
    let key = capability_key(&capability);
    let authorized = {
        let guard = grants.read().await;
        guard
            .get(&key)
            .map(|grant| grant.file_id == file_id && grant.expires_at > Utc::now().timestamp())
            .unwrap_or(false)
    };
    if !authorized {
        write_frame(
            &mut stream,
            &ServerFrame::Error {
                code: "UNAUTHORIZED".into(),
                message: "capability is invalid or expired".into(),
            },
        )
        .await?;
        return Err("invalid capability".into());
    }

    let connection = crate::open_connection(db_path)?;
    let record: Option<(String, String, i64, i64, String)> = connection
        .query_row(
            "SELECT path, filename, size, chunk_size, chunk_hashes FROM files WHERE file_id = ?1 AND NOT EXISTS(SELECT 1 FROM blocked_files WHERE file_id=?1)",
            [&file_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let (path, filename, size, chunk_size, chunk_hashes) =
        record.ok_or("shared file disappeared")?;
    crate::audio::validate_audio(Path::new(&path))
        .map_err(|error| format!("shared file failed the audio-only policy: {error}"))?;
    let chunk_hashes: Vec<String> = chunk_hashes
        .split(',')
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    write_frame(
        &mut stream,
        &ServerFrame::Welcome {
            version: PROTOCOL_VERSION,
            file_id: file_id.clone(),
            filename,
            size: size as u64,
            chunk_size: chunk_size as u32,
            chunk_hashes: chunk_hashes.clone(),
        },
    )
    .await?;

    let mut file = File::open(path).await.map_err(|error| error.to_string())?;
    loop {
        match timeout(
            Duration::from_secs(120),
            read_frame::<_, ClientFrame>(&mut stream),
        )
        .await
        .map_err(|_| "peer was idle for too long".to_string())??
        {
            ClientFrame::RequestChunk { index } => {
                let Some(expected_hash) = chunk_hashes.get(index as usize) else {
                    write_frame(
                        &mut stream,
                        &ServerFrame::Error {
                            code: "BAD_CHUNK".into(),
                            message: "chunk index is out of range".into(),
                        },
                    )
                    .await?;
                    continue;
                };
                let offset = index as u64 * chunk_size as u64;
                let remaining = (size as u64).saturating_sub(offset);
                let count = remaining.min(chunk_size as u64) as usize;
                let mut bytes = vec![0u8; count];
                file.seek(SeekFrom::Start(offset))
                    .await
                    .map_err(|error| error.to_string())?;
                file.read_exact(&mut bytes)
                    .await
                    .map_err(|error| error.to_string())?;
                if hex::encode(Sha256::digest(&bytes)) != *expected_hash {
                    return Err("local shared file changed after indexing".into());
                }
                write_frame(
                    &mut stream,
                    &ServerFrame::ChunkData {
                        index,
                        size: count as u32,
                        sha256: expected_hash.clone(),
                    },
                )
                .await?;
                stream
                    .write_all(&bytes)
                    .await
                    .map_err(|error| error.to_string())?;
                stream.flush().await.map_err(|error| error.to_string())?;
            }
            ClientFrame::TransferComplete => {
                write_frame(&mut stream, &ServerFrame::TransferComplete).await?;
                grants.write().await.remove(&key);
                return Ok(());
            }
            ClientFrame::Cancel => {
                grants.write().await.remove(&key);
                return Ok(());
            }
            ClientFrame::Hello { .. } => return Err("duplicate HELLO".into()),
        }
    }
}

async fn download_offer(
    db_path: &Path,
    tor: Arc<TorManager>,
    offer: &DownloadOffer,
    source_pubkey: &str,
    coordinator: Arc<DownloadCoordinator>,
    mut pause: watch::Receiver<bool>,
) -> Result<(), String> {
    let existing_progress = current_progress(db_path, &offer.request_id).unwrap_or(0.0);
    update_download(
        db_path,
        &offer.request_id,
        existing_progress,
        "Connecting another Tor seeder",
        "Connecting…",
        Some(&offer.onion),
    )?;
    let mut stream = tor
        .connect_onion_with_retry(&offer.onion, offer.port, &coordinator.cancel)
        .await?;
    write_frame(
        &mut stream,
        &ClientFrame::Hello {
            version: PROTOCOL_VERSION,
            capability: offer.capability.clone(),
            file_id: offer.file_id.clone(),
        },
    )
    .await?;
    let welcome: ServerFrame = timeout(Duration::from_secs(60), read_frame(&mut stream))
        .await
        .map_err(|_| "peer manifest timed out".to_string())??;
    let (filename, size, chunk_size, chunk_hashes) = match welcome {
        ServerFrame::Welcome {
            version,
            file_id,
            filename,
            size,
            chunk_size,
            chunk_hashes,
        } if version == PROTOCOL_VERSION && file_id == offer.file_id => {
            (filename, size, chunk_size, chunk_hashes)
        }
        ServerFrame::Error { message, .. } => return Err(message),
        _ => return Err("peer returned an invalid manifest".into()),
    };
    if chunk_hashes.is_empty() && size > 0 {
        return Err("peer returned an empty chunk manifest".into());
    }
    if chunk_size as usize != crate::CHUNK_SIZE
        || chunk_hashes.len() != ((size + chunk_size as u64 - 1) / chunk_size as u64) as usize
    {
        return Err("peer returned an incompatible chunk manifest".into());
    }
    let peer_manifest = PeerManifest {
        filename: filename.clone(),
        size,
        chunk_size,
        chunk_hashes: chunk_hashes.clone(),
    };
    {
        let mut manifest = coordinator.manifest.lock().await;
        match manifest.as_ref() {
            Some(existing) if existing != &peer_manifest => {
                return Err("seeder manifest did not match the other exact-file seeders".into())
            }
            None => *manifest = Some(peer_manifest),
            _ => {}
        }
    }

    let connection = crate::open_connection(db_path)?;
    let download_folder = PathBuf::from(super::get_setting(&connection, "download_folder")?);
    fs::create_dir_all(&download_folder)
        .await
        .map_err(|error| error.to_string())?;
    let parts = download_folder.join(".napstr-parts").join(&offer.file_id);
    fs::create_dir_all(&parts)
        .await
        .map_err(|error| error.to_string())?;
    let destination = {
        let mut selected = coordinator.destination.lock().await;
        selected
            .get_or_insert_with(|| super::unique_destination(&download_folder, &filename))
            .clone()
    };
    drop(connection);

    let started = std::time::Instant::now();
    loop {
        if coordinator.complete.load(Ordering::SeqCst) {
            let _ = write_frame(&mut stream, &ClientFrame::Cancel).await;
            return Ok(());
        }
        let complete = coordinator.verified.lock().await.len();
        while *pause.borrow() {
            update_download(
                db_path,
                &offer.request_id,
                complete as f64 / chunk_hashes.len().max(1) as f64 * 100.0,
                "Paused",
                "—",
                Some(&offer.onion),
            )?;
            tokio::select! {
                _ = coordinator.cancel.cancelled() => {
                    let _ = write_frame(&mut stream, &ClientFrame::Cancel).await;
                    return Err("cancelled".into());
                }
                changed = pause.changed() => { if changed.is_err() { break; } }
            }
        }
        if coordinator.cancel.is_cancelled() {
            let _ = write_frame(&mut stream, &ClientFrame::Cancel).await;
            return Err("cancelled".into());
        }
        if complete == chunk_hashes.len() {
            break;
        }

        let claimed = {
            let verified = coordinator.verified.lock().await;
            let mut claims = coordinator.claims.lock().await;
            (0..chunk_hashes.len()).find(|index| !verified.contains(index) && claims.insert(*index))
        };
        let Some(index) = claimed else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let expected = &chunk_hashes[index];
        let part = parts.join(format!("{index:08}.chunk"));
        let existing_valid = fs::read(&part)
            .await
            .ok()
            .map(|bytes| hex::encode(Sha256::digest(&bytes)) == *expected)
            .unwrap_or(false);
        let chunk_result: Result<(), String> = async {
            if !existing_valid {
                write_frame(&mut stream, &ClientFrame::RequestChunk { index: index as u32 }).await?;
                let header: ServerFrame = timeout(Duration::from_secs(60), read_frame(&mut stream)).await.map_err(|_| "chunk header timed out".to_string())??;
                let (received_index, received_size, received_hash) = match header {
                    ServerFrame::ChunkData { index, size, sha256 } => (index, size, sha256),
                    ServerFrame::Error { message, .. } => return Err(message),
                    _ => return Err("peer returned an invalid chunk header".into()),
                };
                let expected_size = (size.saturating_sub(index as u64 * chunk_size as u64)).min(chunk_size as u64) as u32;
                if received_index != index as u32 || received_hash != *expected || received_size != expected_size { return Err("chunk header did not match the signed manifest".into()); }
                let mut bytes = vec![0u8; received_size as usize];
                timeout(Duration::from_secs(120), stream.read_exact(&mut bytes)).await.map_err(|_| "chunk body timed out".to_string())?.map_err(|error| error.to_string())?;
                if hex::encode(Sha256::digest(&bytes)) != *expected { return Err(format!("chunk {index} failed SHA-256 verification")); }
                let temporary = parts.join(format!("{index:08}.{}.tmp", offer.request_id));
                fs::write(&temporary, &bytes).await.map_err(|error| error.to_string())?;
                fs::rename(temporary, &part).await.map_err(|error| error.to_string())?;
            }
            crate::open_connection(db_path)?.execute(
                "INSERT OR REPLACE INTO download_chunks(request_id,chunk_index,sha256,path,source_pubkey,verified_at) VALUES (?1,?2,?3,?4,?5,?6)",
                params![offer.request_id, index as i64, expected, part.to_string_lossy(), source_pubkey, Utc::now().to_rfc3339()],
            ).map_err(|error| error.to_string())?;
            coordinator.verified.lock().await.insert(index);
            Ok(())
        }.await;
        coordinator.claims.lock().await.remove(&index);
        chunk_result?;

        let complete = coordinator.verified.lock().await.len();
        let progress = complete as f64 / chunk_hashes.len().max(1) as f64 * 100.0;
        let received = (complete as u64 * chunk_size as u64).min(size);
        let speed = if started.elapsed() > Duration::from_millis(250) {
            format_speed(received as f64 / started.elapsed().as_secs_f64())
        } else {
            "—".into()
        };
        update_download(
            db_path,
            &offer.request_id,
            progress,
            "Downloading verified chunks from multiple Tor seeders",
            &speed,
            Some(&offer.onion),
        )?;
    }

    loop {
        if coordinator
            .finalizing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            break;
        }
        while !coordinator.complete.load(Ordering::SeqCst)
            && coordinator.finalizing.load(Ordering::SeqCst)
        {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if coordinator.complete.load(Ordering::SeqCst) {
            let _ = write_frame(&mut stream, &ClientFrame::Cancel).await;
            return Ok(());
        }
    }

    let final_result: Result<(), String> = async {
        let partial = destination.with_extension(format!(
            "{}part",
            destination
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| format!("{value}."))
                .unwrap_or_default()
        ));
        let mut writer = BufWriter::new(
            File::create(&partial)
                .await
                .map_err(|error| error.to_string())?,
        );
        let mut full_hasher = Sha256::new();
        for index in 0..chunk_hashes.len() {
            let mut reader = BufReader::new(
                File::open(parts.join(format!("{index:08}.chunk")))
                    .await
                    .map_err(|error| error.to_string())?,
            );
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .await
                .map_err(|error| error.to_string())?;
            full_hasher.update(&bytes);
            writer
                .write_all(&bytes)
                .await
                .map_err(|error| error.to_string())?;
        }
        writer.flush().await.map_err(|error| error.to_string())?;
        drop(writer);
        if hex::encode(full_hasher.finalize()) != offer.file_id {
            let _ = fs::remove_file(&partial).await;
            return Err("final file SHA-256 verification failed".into());
        }
        let blocked: bool = crate::open_connection(db_path)?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM blocked_files WHERE file_id=?1)",
                [&offer.file_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if blocked {
            let _ = fs::remove_file(&partial).await;
            return Err("download rejected because this file hash is blocked".into());
        }
        if let Err(error) = crate::audio::validate_audio(&partial) {
            let _ = fs::remove_file(&partial).await;
            return Err(format!(
                "download rejected by the audio-only policy: {error}"
            ));
        }
        let mut final_destination = destination.clone();
        loop {
            match fs::hard_link(&partial, &final_destination).await {
                Ok(()) => break,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    final_destination = super::unique_destination(&download_folder, &filename);
                }
                Err(error) => return Err(error.to_string()),
            }
        }
        fs::remove_file(&partial)
            .await
            .map_err(|error| error.to_string())?;
        *coordinator.destination.lock().await = Some(final_destination.clone());
        if write_frame(&mut stream, &ClientFrame::TransferComplete)
            .await
            .is_ok()
        {
            let _ = timeout(
                Duration::from_secs(10),
                read_frame::<_, ServerFrame>(&mut stream),
            )
            .await;
        }
        let _ = fs::remove_dir_all(&parts).await;
        update_download(
            db_path,
            &offer.request_id,
            100.0,
            "Verified · Complete",
            "—",
            Some(&offer.onion),
        )?;
        let connection = crate::open_connection(db_path)?;
        connection
            .execute(
                "UPDATE network_downloads SET destination = ?1 WHERE request_id = ?2",
                params![final_destination.to_string_lossy(), offer.request_id],
            )
            .map_err(|error| error.to_string())?;
        connection
            .execute(
                "DELETE FROM download_chunks WHERE request_id = ?1",
                [&offer.request_id],
            )
            .map_err(|error| error.to_string())?;
        coordinator.complete.store(true, Ordering::SeqCst);
        Ok(())
    }
    .await;
    if final_result.is_err() {
        coordinator.finalizing.store(false, Ordering::SeqCst);
    }
    final_result
}

fn current_progress(db_path: &Path, request_id: &str) -> Result<f64, String> {
    crate::open_connection(db_path)?
        .query_row(
            "SELECT progress FROM network_downloads WHERE request_id=?1",
            [request_id],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())
}

fn update_download(
    db_path: &Path,
    request_id: &str,
    progress: f64,
    status: &str,
    speed: &str,
    onion: Option<&str>,
) -> Result<(), String> {
    crate::open_connection(db_path)?.execute(
        "UPDATE network_downloads SET progress=?1, status=?2, speed=?3, onion=COALESCE(?4,onion), updated_at=?5 WHERE request_id=?6",
        params![progress, status, speed, onion, Utc::now().to_rfc3339(), request_id],
    ).map_err(|error| error.to_string())?;
    Ok(())
}

fn format_speed(bytes_per_second: f64) -> String {
    if bytes_per_second >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", bytes_per_second / 1024.0 / 1024.0)
    } else {
        format!("{:.0} KB/s", bytes_per_second / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tokio::io::{duplex, DuplexStream};

    fn test_peer(
        db_path: PathBuf,
        grants: Arc<RwLock<HashMap<String, SessionGrant>>>,
    ) -> DuplexStream {
        let (client, server) = duplex(2 * 1024 * 1024);
        tokio::spawn(async move {
            let _ = serve_connection(server, &db_path, grants).await;
        });
        client
    }

    fn shared_fixture() -> (PathBuf, PathBuf, String, Vec<u8>) {
        let directory =
            std::env::temp_dir().join(format!("napstr-transfer-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let db_path = directory.join("napstr.sqlite3");
        crate::initialise_database(&db_path, &directory).unwrap();
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&44u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0\x44\xac\0\0\x88\x58\x01\0\x02\0\x10\0data\x08\0\0\0audio123");
        let file_path = directory.join("payload.wav");
        std::fs::write(&file_path, &bytes).unwrap();
        let (file_id, chunks, size) = crate::hash_file(&file_path).unwrap();
        Connection::open(&db_path).unwrap().execute(
            "INSERT INTO files(file_id,filename,path,size,format,chunk_size,chunk_hashes,indexed_at,mime) VALUES(?1,'payload.wav',?2,?3,'WAV',?4,?5,?6,'audio/wav')",
            params![file_id, file_path.to_string_lossy(), size as i64, crate::CHUNK_SIZE as i64, chunks.join(","), Utc::now().to_rfc3339()],
        ).unwrap();
        (directory, db_path, file_id, bytes)
    }

    #[tokio::test]
    async fn capability_authorizes_only_the_negotiated_file() {
        let (directory, db_path, file_id, bytes) = shared_fixture();
        let capability = "private-capability".to_string();
        let grants = Arc::new(RwLock::new(HashMap::from([(
            capability_key(&capability),
            SessionGrant {
                file_id: file_id.clone(),
                requester: "requester".into(),
                expires_at: Utc::now().timestamp() + 60,
                _onion_lease: None,
            },
        )])));

        let mut bad = test_peer(db_path.clone(), grants.clone());
        write_frame(
            &mut bad,
            &ClientFrame::Hello {
                version: PROTOCOL_VERSION,
                capability: "wrong".into(),
                file_id: file_id.clone(),
            },
        )
        .await
        .unwrap();
        assert!(
            matches!(read_frame::<_, ServerFrame>(&mut bad).await.unwrap(), ServerFrame::Error { code, .. } if code == "UNAUTHORIZED")
        );

        let mut stream = test_peer(db_path, grants);
        write_frame(
            &mut stream,
            &ClientFrame::Hello {
                version: PROTOCOL_VERSION,
                capability,
                file_id,
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            read_frame::<_, ServerFrame>(&mut stream).await.unwrap(),
            ServerFrame::Welcome { .. }
        ));
        write_frame(&mut stream, &ClientFrame::RequestChunk { index: 0 })
            .await
            .unwrap();
        let size = match read_frame::<_, ServerFrame>(&mut stream).await.unwrap() {
            ServerFrame::ChunkData { index: 0, size, .. } => size,
            other => panic!("unexpected response: {other:?}"),
        };
        let mut received = vec![0; size as usize];
        stream.read_exact(&mut received).await.unwrap();
        assert_eq!(received, bytes);
        write_frame(&mut stream, &ClientFrame::TransferComplete)
            .await
            .unwrap();
        assert!(matches!(
            read_frame::<_, ServerFrame>(&mut stream).await.unwrap(),
            ServerFrame::TransferComplete
        ));
        std::fs::remove_dir_all(directory).unwrap();
    }
}
