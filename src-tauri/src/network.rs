use crate::transfer::{DownloadOffer, TransferService};
use chrono::Utc;
use futures_util::{stream, StreamExt};
use keyring::Entry;
use nostr_sdk::prelude::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::RwLock;
use uuid::Uuid;

pub const CATALOGUE_KIND: u16 = 30421;
pub const AVAILABILITY_KIND: u16 = 30422;

pub fn validate_profile_picture(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Ok(());
    }
    let url = Url::parse(value).map_err(|error| format!("invalid profile picture URL: {error}"))?;
    if url.scheme() != "https" {
        return Err("profile picture URL must use HTTPS".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStatus {
    pub connected: bool,
    pub npub: String,
    pub pubkey: String,
    pub relay_count: usize,
    pub tor_running: bool,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueSource {
    pub pubkey: String,
    pub npub: String,
    pub display_name: String,
    pub relay: String,
    pub about: String,
    pub picture: String,
    pub event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogueResult {
    pub file_id: String,
    pub filename: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub format: String,
    pub mime: String,
    pub size: u64,
    pub license: String,
    pub description: String,
    pub tags: String,
    pub sources: Vec<CatalogueSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CatalogueContent {
    protocol: String,
    file_id: String,
    filename: String,
    title: String,
    artist: String,
    album: String,
    format: String,
    mime: String,
    size: u64,
    license: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    tags: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
enum SignalMessage {
    DownloadRequest {
        protocol: String,
        request_id: String,
        file_id: String,
    },
    DownloadOffer {
        protocol: String,
        offer: DownloadOffer,
    },
    DownloadRefused {
        protocol: String,
        request_id: String,
        file_id: String,
        reason: String,
    },
}

pub struct NetworkService {
    db_path: PathBuf,
    transfers: Arc<TransferService>,
    client: RwLock<Option<Client>>,
    keys: RwLock<Option<Keys>>,
    connected: AtomicBool,
    last_error: RwLock<String>,
}

impl NetworkService {
    pub fn new(db_path: PathBuf, transfers: Arc<TransferService>) -> Arc<Self> {
        Arc::new(Self {
            db_path,
            transfers,
            client: RwLock::new(None),
            keys: RwLock::new(None),
            connected: AtomicBool::new(false),
            last_error: RwLock::new(String::new()),
        })
    }

    pub fn transfers(&self) -> &Arc<TransferService> {
        &self.transfers
    }

    pub async fn start(self: &Arc<Self>) -> Result<NetworkStatus, String> {
        if self.connected.load(Ordering::SeqCst) {
            return self.status().await;
        }
        let keys = load_or_create_identity()?;
        let connection = super::open_connection(&self.db_path)?;
        let relays = relay_urls(&super::get_setting(&connection, "nostr_relays")?);
        if relays.is_empty() {
            return Err("at least one Nostr relay is required".into());
        }
        let display_name = super::get_setting(&connection, "display_name")?;
        let profile_about = super::get_setting(&connection, "profile_about")?;
        let profile_picture = super::get_setting(&connection, "profile_picture")?;
        drop(connection);

        let client = Client::new(keys.clone());
        client.automatic_authentication(true);
        for relay in &relays {
            client
                .add_relay(relay)
                .await
                .map_err(|error| format!("relay {relay}: {error}"))?;
        }
        client.connect().await;

        let mut metadata = Metadata::new()
            .name(display_name.clone())
            .display_name(display_name)
            .about(profile_about);
        if !profile_picture.trim().is_empty() {
            metadata = metadata.picture(
                Url::parse(&profile_picture)
                    .map_err(|error| format!("invalid profile picture URL: {error}"))?,
            );
        }
        client
            .set_metadata(&metadata)
            .await
            .map_err(|error| format!("profile publication failed: {error}"))?;
        let dm_tags = relays
            .iter()
            .map(|relay| Tag::parse(["relay", relay.as_str()]).map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        client
            .send_event_builder(EventBuilder::new(Kind::from(10050), "").tags(dm_tags))
            .await
            .map_err(|error| format!("DM relay publication failed: {error}"))?;

        let inbox_filter = Filter::new()
            .kind(Kind::GiftWrap)
            .pubkey(keys.public_key())
            .limit(0);
        client
            .subscribe(inbox_filter, None)
            .await
            .map_err(|error| format!("NIP-17 inbox subscription failed: {error}"))?;
        *self.client.write().await = Some(client.clone());
        *self.keys.write().await = Some(keys);
        self.connected.store(true, Ordering::SeqCst);
        *self.last_error.write().await = String::new();

        let service = self.clone();
        tokio::spawn(async move {
            let listener_client = client.clone();
            let event_client = listener_client.clone();
            let event_service = service.clone();
            let result = listener_client
                .handle_notifications(move |notification| {
                    let service = event_service.clone();
                    let client = event_client.clone();
                    async move {
                        if let RelayPoolNotification::Event { event, .. } = notification {
                            if event.kind == Kind::GiftWrap {
                                if let Ok(unwrapped) = client.unwrap_gift_wrap(&event).await {
                                    let unexpired = unwrapped
                                        .rumor
                                        .tags
                                        .expiration()
                                        .map(|expires| *expires > Timestamp::now())
                                        .unwrap_or(false);
                                    if unwrapped.rumor.kind == Kind::PrivateDirectMessage
                                        && unexpired
                                    {
                                        let _ = service
                                            .handle_signal(
                                                unwrapped.sender,
                                                &unwrapped.rumor.content,
                                            )
                                            .await;
                                    }
                                }
                            }
                        }
                        Ok(false)
                    }
                })
                .await;
            if let Err(error) = result {
                service.connected.store(false, Ordering::SeqCst);
                *service.last_error.write().await = error.to_string();
            }
        });

        self.publish_catalogue().await?;
        let heartbeat = self.clone();
        tokio::spawn(async move {
            while heartbeat.connected.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_secs(240)).await;
                if heartbeat.connected.load(Ordering::SeqCst) {
                    let _ = heartbeat.publish_availability().await;
                }
            }
        });
        self.status().await
    }

    pub async fn stop(&self) {
        if let Some(client) = self.client.write().await.take() {
            client.disconnect().await;
        }
        self.connected.store(false, Ordering::SeqCst);
    }

    pub async fn status(&self) -> Result<NetworkStatus, String> {
        let keys = self
            .keys
            .read()
            .await
            .clone()
            .or_else(|| load_or_create_identity().ok());
        let (npub, pubkey) = match keys {
            Some(keys) => (
                keys.public_key()
                    .to_bech32()
                    .map_err(|error| error.to_string())?,
                keys.public_key().to_hex(),
            ),
            None => (String::new(), String::new()),
        };
        let relay_count = super::open_connection(&self.db_path)
            .ok()
            .and_then(|connection| super::get_setting(&connection, "nostr_relays").ok())
            .map(|value| relay_urls(&value).len())
            .unwrap_or(0);
        Ok(NetworkStatus {
            connected: self.connected.load(Ordering::SeqCst),
            npub,
            pubkey,
            relay_count,
            tor_running: false,
            error: self.last_error.read().await.clone(),
        })
    }

    pub async fn publish_catalogue(&self) -> Result<usize, String> {
        let client = self
            .client
            .read()
            .await
            .clone()
            .ok_or("Nostr is not connected")?;
        let files = load_publish_files(&self.db_path)?;
        let current_ids: HashSet<String> = files.iter().map(|file| file.0.clone()).collect();
        let stale = load_published_ids(&self.db_path)?
            .into_iter()
            .filter(|id| !current_ids.contains(id))
            .collect::<Vec<_>>();
        for file_id in stale {
            let tags = vec![
                Tag::parse(["d", file_id.as_str()]),
                Tag::parse(["t", "napstr"]),
            ]
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
            client
                .send_event_builder(
                    EventBuilder::new(
                        Kind::from(CATALOGUE_KIND),
                        r#"{"protocol":"napstr/1","deleted":true}"#,
                    )
                    .tags(tags),
                )
                .await
                .map_err(|error| format!("catalogue withdrawal failed: {error}"))?;
            super::open_connection(&self.db_path)?
                .execute(
                    "DELETE FROM published_catalogue WHERE file_id=?1",
                    [&file_id],
                )
                .map_err(|error| error.to_string())?;
        }
        let mut published = 0;
        for (
            file_id,
            filename,
            size,
            format,
            title,
            artist,
            album,
            mime,
            license,
            description,
            content_tags,
        ) in files
        {
            let content = CatalogueContent {
                protocol: "napstr/1".into(),
                file_id: file_id.clone(),
                filename: filename.clone(),
                title: if title.is_empty() {
                    filename.clone()
                } else {
                    title
                },
                artist,
                album,
                format: format.clone(),
                mime: if mime.is_empty() || mime == "application/octet-stream" {
                    mime_for_format(&format)
                } else {
                    mime
                },
                size,
                license: if license.is_empty() {
                    "unspecified".into()
                } else {
                    license
                },
                description,
                tags: content_tags,
            };
            let tags = vec![
                Tag::parse(["d", file_id.as_str()]),
                Tag::parse(["t", "napstr"]),
                Tag::parse(["x", file_id.as_str()]),
                Tag::parse(["name", filename.as_str()]),
                Tag::parse(["size", &size.to_string()]),
                Tag::parse(["m", content.mime.as_str()]),
                Tag::parse(["alt", "Napstr shared file catalogue entry"]),
            ]
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
            client
                .send_event_builder(
                    EventBuilder::new(
                        Kind::from(CATALOGUE_KIND),
                        serde_json::to_string(&content).map_err(|error| error.to_string())?,
                    )
                    .tags(tags),
                )
                .await
                .map_err(|error| format!("catalogue publication failed for {filename}: {error}"))?;
            super::open_connection(&self.db_path)?.execute("INSERT OR REPLACE INTO published_catalogue(file_id,published_at) VALUES (?1,?2)", params![file_id, Utc::now().to_rfc3339()]).map_err(|error| error.to_string())?;
            published += 1;
        }
        self.publish_availability().await?;
        Ok(published)
    }

    pub async fn publish_profile(&self) -> Result<(), String> {
        let client = self
            .client
            .read()
            .await
            .clone()
            .ok_or("Nostr is not connected")?;
        let connection = super::open_connection(&self.db_path)?;
        let display_name = super::get_setting(&connection, "display_name")?;
        let about = super::get_setting(&connection, "profile_about")?;
        let picture = super::get_setting(&connection, "profile_picture")?;
        drop(connection);
        let mut metadata = Metadata::new()
            .name(display_name.clone())
            .display_name(display_name)
            .about(about);
        if !picture.trim().is_empty() {
            metadata = metadata.picture(
                Url::parse(&picture)
                    .map_err(|error| format!("invalid profile picture URL: {error}"))?,
            );
        }
        client
            .set_metadata(&metadata)
            .await
            .map_err(|error| format!("profile publication failed: {error}"))?;
        Ok(())
    }

    async fn publish_availability(&self) -> Result<(), String> {
        let client = self
            .client
            .read()
            .await
            .clone()
            .ok_or("Nostr is not connected")?;
        let ids = load_publish_files(&self.db_path)?
            .into_iter()
            .map(|file| file.0)
            .collect::<Vec<_>>();
        let expiration = (Utc::now().timestamp() + 10 * 60).to_string();
        let batches: Vec<&[String]> = if ids.is_empty() {
            vec![&[]]
        } else {
            ids.chunks(400).collect()
        };
        for (index, batch) in batches.into_iter().enumerate() {
            let batch_id = format!("availability-{index:04}");
            let tags = vec![
                Tag::parse(["d", batch_id.as_str()]),
                Tag::parse(["t", "napstr-availability"]),
                Tag::parse(["expiration", expiration.as_str()]),
            ]
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
            client
                .send_event_builder(
                    EventBuilder::new(
                        Kind::from(AVAILABILITY_KIND),
                        serde_json::to_string(batch).map_err(|error| error.to_string())?,
                    )
                    .tags(tags),
                )
                .await
                .map_err(|error| format!("availability heartbeat failed: {error}"))?;
        }
        Ok(())
    }

    pub async fn search(&self, query: &str) -> Result<Vec<CatalogueResult>, String> {
        let client = self
            .client
            .read()
            .await
            .clone()
            .ok_or("Nostr is not connected")?;
        let catalogue_query = client.fetch_events(
            Filter::new()
                .kind(Kind::from(CATALOGUE_KIND))
                .hashtag("napstr")
                .limit(10_000),
            Duration::from_secs(10),
        );
        let availability_query = client.fetch_events(
            Filter::new()
                .kind(Kind::from(AVAILABILITY_KIND))
                .hashtag("napstr-availability")
                .since(Timestamp::from(
                    Utc::now().timestamp().saturating_sub(12 * 60) as u64,
                ))
                .limit(5_000),
            Duration::from_secs(6),
        );
        let (events, availability) = tokio::join!(catalogue_query, availability_query);
        let events = events.map_err(|error| format!("catalogue search failed: {error}"))?;
        let availability =
            availability.map_err(|error| format!("availability search failed: {error}"))?;
        let mut online: HashSet<(String, String)> = HashSet::new();
        for event in availability.iter() {
            if event
                .tags
                .expiration()
                .map(|expires| *expires <= Timestamp::now())
                .unwrap_or(true)
            {
                continue;
            }
            if let Ok(ids) = serde_json::from_str::<Vec<String>>(&event.content) {
                for id in ids {
                    online.insert((event.pubkey.to_hex(), id));
                }
            }
        }
        let query = query.trim().to_lowercase();
        let mut aggregated: HashMap<String, CatalogueResult> = HashMap::new();
        let connection = super::open_connection(&self.db_path)?;
        for event in events.iter() {
            let Ok(content) = serde_json::from_str::<CatalogueContent>(&event.content) else {
                continue;
            };
            if content.protocol != "napstr/1"
                || !hex::decode(&content.file_id)
                    .map(|bytes| bytes.len() == 32)
                    .unwrap_or(false)
                || !audio_claim_valid(&content.filename, &content.format, &content.mime)
            {
                continue;
            }
            let haystack = format!(
                "{} {} {} {} {} {}",
                content.filename,
                content.title,
                content.artist,
                content.album,
                content.description,
                content.tags
            )
            .to_lowercase();
            if !query.is_empty() && !haystack.contains(&query) {
                continue;
            }
            let pubkey = event.pubkey.to_hex();
            let blocked: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM blocked_files WHERE file_id=?1) OR EXISTS(SELECT 1 FROM blocked_pubkeys WHERE pubkey=?2)",
                params![content.file_id, pubkey], |row| row.get(0),
            ).map_err(|error| error.to_string())?;
            if blocked {
                continue;
            }
            if !online.contains(&(pubkey.clone(), content.file_id.clone())) {
                continue;
            }
            let npub = event.pubkey.to_bech32().unwrap_or_else(|_| pubkey.clone());
            let source = CatalogueSource {
                pubkey: pubkey.clone(),
                npub,
                display_name: short_key(&pubkey),
                relay: String::new(),
                about: String::new(),
                picture: String::new(),
                event_id: event.id.to_hex(),
            };
            connection.execute(
                "INSERT OR REPLACE INTO remote_catalogue (file_id,source_pubkey,filename,title,artist,album,format,mime,size,license,description,tags,event_id,seen_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                params![content.file_id, pubkey, content.filename, content.title, content.artist, content.album, content.format, content.mime, content.size as i64, content.license, content.description, content.tags, event.id.to_hex(), Utc::now().to_rfc3339()],
            ).map_err(|error| error.to_string())?;
            aggregated
                .entry(content.file_id.clone())
                .and_modify(|item| {
                    if !item
                        .sources
                        .iter()
                        .any(|existing| existing.pubkey == source.pubkey)
                    {
                        item.sources.push(source.clone());
                    }
                })
                .or_insert(CatalogueResult {
                    file_id: content.file_id,
                    filename: content.filename,
                    title: content.title,
                    artist: content.artist,
                    album: content.album,
                    format: content.format,
                    mime: content.mime,
                    size: content.size,
                    license: content.license,
                    description: content.description,
                    tags: content.tags,
                    sources: vec![source],
                });
        }
        let mut results: Vec<_> = aggregated.into_values().collect();
        let profile_keys = results
            .iter()
            .flat_map(|result| result.sources.iter())
            .filter_map(|source| PublicKey::from_str(&source.pubkey).ok())
            .collect::<HashSet<_>>()
            .into_iter()
            .take(128);
        let profiles: HashMap<String, Metadata> = stream::iter(profile_keys)
            .map(|public_key| {
                let client = client.clone();
                async move {
                    client
                        .fetch_metadata(public_key, Duration::from_secs(3))
                        .await
                        .ok()
                        .flatten()
                        .map(|metadata| (public_key.to_hex(), metadata))
                }
            })
            .buffer_unordered(16)
            .filter_map(|profile| async move { profile })
            .collect()
            .await;
        for result in &mut results {
            for source in &mut result.sources {
                if let Some(metadata) = profiles.get(&source.pubkey) {
                    if let Some(name) = metadata
                        .display_name
                        .clone()
                        .or_else(|| metadata.name.clone())
                    {
                        source.display_name = name;
                    }
                    source.about = metadata.about.clone().unwrap_or_default();
                    source.picture = metadata.picture.clone().unwrap_or_default();
                }
            }
        }
        results.sort_by(|left, right| {
            right
                .sources
                .len()
                .cmp(&left.sources.len())
                .then_with(|| left.filename.cmp(&right.filename))
        });
        Ok(results)
    }

    pub async fn request_download(
        &self,
        file_id: String,
        source_pubkeys: Vec<String>,
    ) -> Result<String, String> {
        let client = self
            .client
            .read()
            .await
            .clone()
            .ok_or("Nostr is not connected")?;
        let mut unique = source_pubkeys
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        unique.sort();
        unique.truncate(32);
        if unique.is_empty() {
            return Err("at least one seeder is required".into());
        }
        let connection = super::open_connection(&self.db_path)?;
        let already_local: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE file_id=?1)",
                [&file_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if already_local {
            return Err("this audio is already on this computer; play it locally".into());
        }
        let file_blocked: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM blocked_files WHERE file_id=?1)",
                [&file_id],
                |row| row.get(0),
            )
            .map_err(|error| error.to_string())?;
        if file_blocked {
            return Err("this file hash is blocked".into());
        }
        let mut receivers = Vec::new();
        let mut file_record = None;
        for source in &unique {
            let source_blocked: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM blocked_pubkeys WHERE pubkey=?1)",
                    [source],
                    |row| row.get(0),
                )
                .map_err(|error| error.to_string())?;
            if source_blocked {
                return Err(format!("seeder {source} is blocked"));
            }
            let record: Option<(String, i64)> = connection.query_row(
                "SELECT filename,size FROM remote_catalogue WHERE file_id=?1 AND source_pubkey=?2", params![file_id, source],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional().map_err(|error| error.to_string())?;
            let record = record
                .ok_or_else(|| format!("seeder {source} is not in the current catalogue cache"))?;
            file_record.get_or_insert(record);
            receivers.push((
                source.clone(),
                PublicKey::from_str(source)
                    .map_err(|error| format!("invalid seeder public key: {error}"))?,
            ));
        }
        let (filename, size) = file_record.ok_or("catalogue record disappeared")?;
        let request_id = Uuid::new_v4().to_string();
        connection.execute(
            "INSERT INTO network_downloads (request_id,file_id,source_pubkey,filename,size,progress,status,speed,destination,onion,updated_at) VALUES (?1,?2,?3,?4,?5,0,'Waiting for encrypted multi-source offers','—','','',?6)",
            params![request_id, file_id, unique[0], filename, size, Utc::now().to_rfc3339()],
        ).map_err(|error| error.to_string())?;
        for (source, _) in &receivers {
            connection.execute("INSERT INTO download_sources(request_id,source_pubkey,status,updated_at) VALUES(?1,?2,'Requested',?3)", params![request_id, source, Utc::now().to_rfc3339()]).map_err(|error| error.to_string())?;
        }
        drop(connection);
        let message = SignalMessage::DownloadRequest {
            protocol: "napstr/1".into(),
            request_id: request_id.clone(),
            file_id,
        };
        let content = serde_json::to_string(&message).map_err(|error| error.to_string())?;
        let mut sent = 0;
        for (source, receiver) in receivers {
            match client
                .send_private_msg(receiver, content.clone(), signal_tags())
                .await
            {
                Ok(_) => sent += 1,
                Err(error) => {
                    if let Ok(connection) = super::open_connection(&self.db_path) {
                        let _ = connection.execute("UPDATE download_sources SET status=?1,updated_at=?2 WHERE request_id=?3 AND source_pubkey=?4", params![format!("Failed: {error}"), Utc::now().to_rfc3339(), request_id, source]);
                    }
                }
            }
        }
        if sent == 0 {
            return Err("NIP-17 request could not be delivered to any seeder".into());
        }
        Ok(request_id)
    }

    pub async fn report_catalogue(
        &self,
        file_id: String,
        source_pubkey: String,
        event_id: String,
        report_type: String,
        reason: String,
    ) -> Result<(), String> {
        let report_type = report_type.trim().to_ascii_lowercase();
        if !matches!(
            report_type.as_str(),
            "illegal" | "malware" | "spam" | "nudity" | "profanity" | "impersonation" | "other"
        ) {
            return Err("unsupported NIP-56 report type".into());
        }
        if reason.trim().is_empty() || reason.len() > 500 {
            return Err("a report reason between 1 and 500 characters is required".into());
        }
        PublicKey::from_str(&source_pubkey).map_err(|_| "invalid seeder public key")?;
        if !hex::decode(&file_id)
            .map(|bytes| bytes.len() == 32)
            .unwrap_or(false)
            || !hex::decode(&event_id)
                .map(|bytes| bytes.len() == 32)
                .unwrap_or(false)
        {
            return Err("invalid event or file hash".into());
        }
        let connection = super::open_connection(&self.db_path)?;
        let known: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM remote_catalogue WHERE file_id=?1 AND source_pubkey=?2 AND event_id=?3)",
            params![file_id, source_pubkey, event_id], |row| row.get(0),
        ).map_err(|error| error.to_string())?;
        if !known {
            return Err("the catalogue event is no longer in the local search cache".into());
        }
        drop(connection);
        let tags = vec![
            Tag::parse(["p", source_pubkey.as_str(), report_type.as_str()]),
            Tag::parse(["e", event_id.as_str(), report_type.as_str()]),
            Tag::parse(["x", file_id.as_str(), report_type.as_str()]),
            Tag::parse(["client", "Napstr"]),
        ]
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
        self.client
            .read()
            .await
            .clone()
            .ok_or("Nostr is not connected")?
            .send_event_builder(EventBuilder::new(Kind::from(1984), reason.trim()).tags(tags))
            .await
            .map_err(|error| format!("NIP-56 report publication failed: {error}"))?;
        Ok(())
    }

    async fn handle_signal(&self, sender: PublicKey, content: &str) -> Result<(), String> {
        let message: SignalMessage =
            serde_json::from_str(content).map_err(|error| error.to_string())?;
        let sender_hex = sender.to_hex();
        match message {
            SignalMessage::DownloadRequest {
                protocol,
                request_id,
                file_id,
            } if protocol == "napstr/1" => {
                let response = match self
                    .transfers
                    .create_offer(request_id.clone(), file_id.clone(), sender_hex)
                    .await
                {
                    Ok(offer) => SignalMessage::DownloadOffer {
                        protocol: "napstr/1".into(),
                        offer,
                    },
                    Err(reason) => SignalMessage::DownloadRefused {
                        protocol: "napstr/1".into(),
                        request_id,
                        file_id,
                        reason,
                    },
                };
                self.client
                    .read()
                    .await
                    .clone()
                    .ok_or("Nostr disconnected")?
                    .send_private_msg(
                        sender,
                        serde_json::to_string(&response).map_err(|error| error.to_string())?,
                        signal_tags(),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
            }
            SignalMessage::DownloadOffer { protocol, offer } if protocol == "napstr/1" => {
                let connection = super::open_connection(&self.db_path)?;
                let blocked: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM blocked_files WHERE file_id=?1) OR EXISTS(SELECT 1 FROM blocked_pubkeys WHERE pubkey=?2)",
                    params![offer.file_id, sender.to_hex()], |row| row.get(0),
                ).map_err(|error| error.to_string())?;
                if blocked {
                    return Err("offer rejected by the local blocklist".into());
                }
                let expected: bool = connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM download_sources s JOIN network_downloads d ON d.request_id=s.request_id WHERE s.request_id=?1 AND d.file_id=?2 AND s.source_pubkey=?3)",
                    params![offer.request_id, offer.file_id, sender.to_hex()], |row| row.get(0),
                ).map_err(|error| error.to_string())?;
                if !expected {
                    return Err("offer sender did not match a requested seeder".into());
                }
                connection.execute("UPDATE download_sources SET status='Connected',updated_at=?1 WHERE request_id=?2 AND source_pubkey=?3", params![Utc::now().to_rfc3339(), offer.request_id, sender.to_hex()]).map_err(|error| error.to_string())?;
                drop(connection);
                self.transfers.accept_offer(offer, sender.to_hex()).await?;
            }
            SignalMessage::DownloadRefused {
                protocol,
                request_id,
                reason,
                ..
            } if protocol == "napstr/1" => {
                let connection = super::open_connection(&self.db_path)?;
                connection.execute("UPDATE download_sources SET status=?1,updated_at=?2 WHERE request_id=?3 AND source_pubkey=?4", params![format!("Refused: {reason}"), Utc::now().to_rfc3339(), request_id, sender.to_hex()]).map_err(|error| error.to_string())?;
                let pending: i64 = connection.query_row("SELECT count(*) FROM download_sources WHERE request_id=?1 AND status IN ('Requested','Connected')", [&request_id], |row| row.get(0)).map_err(|error| error.to_string())?;
                if pending == 0 {
                    connection.execute("UPDATE network_downloads SET status='All seeders refused',updated_at=?1 WHERE request_id=?2", params![Utc::now().to_rfc3339(), request_id]).map_err(|error| error.to_string())?;
                }
            }
            _ => return Err("unsupported Napstr signal".into()),
        }
        Ok(())
    }
}

pub fn initialise_network_schema(connection: &Connection) -> Result<(), String> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS remote_catalogue (
           file_id TEXT NOT NULL, source_pubkey TEXT NOT NULL, filename TEXT NOT NULL, title TEXT NOT NULL,
           artist TEXT NOT NULL, album TEXT NOT NULL, format TEXT NOT NULL, mime TEXT NOT NULL, size INTEGER NOT NULL,
           license TEXT NOT NULL, event_id TEXT NOT NULL, seen_at TEXT NOT NULL,
           PRIMARY KEY(file_id, source_pubkey)
         );
         CREATE TABLE IF NOT EXISTS network_downloads (
           request_id TEXT PRIMARY KEY, file_id TEXT NOT NULL, source_pubkey TEXT NOT NULL, filename TEXT NOT NULL,
           size INTEGER NOT NULL, progress REAL NOT NULL, status TEXT NOT NULL, speed TEXT NOT NULL,
           destination TEXT NOT NULL, onion TEXT NOT NULL, updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS download_chunks (
           request_id TEXT NOT NULL, chunk_index INTEGER NOT NULL, sha256 TEXT NOT NULL,
           path TEXT NOT NULL, source_pubkey TEXT NOT NULL DEFAULT '', verified_at TEXT NOT NULL,
           PRIMARY KEY(request_id, chunk_index),
           FOREIGN KEY(request_id) REFERENCES network_downloads(request_id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS download_sources (
           request_id TEXT NOT NULL, source_pubkey TEXT NOT NULL, status TEXT NOT NULL, updated_at TEXT NOT NULL,
           PRIMARY KEY(request_id, source_pubkey),
           FOREIGN KEY(request_id) REFERENCES network_downloads(request_id) ON DELETE CASCADE
         );
         CREATE TABLE IF NOT EXISTS published_catalogue (file_id TEXT PRIMARY KEY, published_at TEXT NOT NULL);"
    ).map_err(|error| error.to_string())
    .and_then(|_| super::ensure_column(connection, "remote_catalogue", "description", "TEXT NOT NULL DEFAULT ''"))
    .and_then(|_| super::ensure_column(connection, "remote_catalogue", "tags", "TEXT NOT NULL DEFAULT ''"))
    .and_then(|_| super::ensure_column(connection, "download_chunks", "source_pubkey", "TEXT NOT NULL DEFAULT ''"))
}

pub fn load_network_transfers(connection: &Connection) -> Result<Vec<super::Transfer>, String> {
    let mut statement = connection.prepare("SELECT rowid,file_id,filename,size,progress,status,speed,destination FROM network_downloads ORDER BY updated_at DESC").map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(super::Transfer {
                id: -row.get::<_, i64>(0)?,
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
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

fn load_or_create_identity() -> Result<Keys, String> {
    if let Ok(nsec) = std::env::var("NAPSTR_NSEC") {
        return Keys::parse(&nsec).map_err(|error| error.to_string());
    }
    let entry =
        Entry::new("social.napstr.desktop", "nostr-identity").map_err(|error| error.to_string())?;
    if let Ok(secret) = entry.get_password() {
        return Keys::parse(&secret).map_err(|error| error.to_string());
    }
    let keys = Keys::generate();
    let nsec = keys
        .secret_key()
        .to_bech32()
        .map_err(|error| error.to_string())?;
    entry.set_password(&nsec).map_err(|error| {
        format!("could not store Nostr identity in the operating-system keyring: {error}")
    })?;
    Ok(keys)
}

type PublishFile = (
    String,
    String,
    u64,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
);

fn load_publish_files(db_path: &PathBuf) -> Result<Vec<PublishFile>, String> {
    let connection = super::open_connection(db_path)?;
    let mut statement = connection.prepare(
        "SELECT file_id, filename, size, format, title, artist, album, mime, license, description, tags, path
         FROM files WHERE NOT EXISTS(SELECT 1 FROM blocked_files WHERE blocked_files.file_id=files.file_id)
         ORDER BY filename"
    ).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    let mut files = Vec::new();
    for row in rows {
        let (
            file_id,
            filename,
            size,
            format,
            title,
            artist,
            album,
            mime,
            license,
            description,
            tags,
            path,
        ) = row.map_err(|error| error.to_string())?;
        let Ok(validated) = crate::audio::validate_audio(std::path::Path::new(&path)) else {
            continue;
        };
        if validated.format != format || validated.mime != mime {
            continue;
        }
        files.push((
            file_id,
            filename,
            size,
            format,
            title,
            artist,
            album,
            mime,
            license,
            description,
            tags,
        ));
    }
    Ok(files)
}

fn load_published_ids(db_path: &PathBuf) -> Result<Vec<String>, String> {
    let connection = super::open_connection(db_path)?;
    let mut statement = connection
        .prepare("SELECT file_id FROM published_catalogue")
        .map_err(|error| error.to_string())?;
    let ids = statement
        .query_map([], |row| row.get(0))
        .map_err(|error| error.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    Ok(ids)
}

fn relay_urls(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|relay| relay.starts_with("wss://") || relay.starts_with("ws://"))
        .map(str::to_owned)
        .collect()
}

fn short_key(value: &str) -> String {
    format!(
        "{}…{}",
        &value[..8.min(value.len())],
        &value[value.len().saturating_sub(4)..]
    )
}

fn audio_claim_valid(filename: &str, format: &str, mime: &str) -> bool {
    let extension = std::path::Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(
        (
            extension.as_str(),
            format.to_ascii_uppercase().as_str(),
            mime
        ),
        ("mp3", "MP3", "audio/mpeg")
            | ("flac", "FLAC", "audio/flac")
            | ("wav", "WAV", "audio/wav")
            | ("ogg", "OGG", "audio/ogg")
            | ("opus", "OPUS", "audio/ogg")
    )
}

fn signal_tags() -> Vec<Tag> {
    vec![
        Tag::expiration(Timestamp::from((Utc::now().timestamp() + 20 * 60) as u64)),
        Tag::client("Napstr"),
    ]
}

fn mime_for_format(format: &str) -> String {
    match format.to_ascii_lowercase().as_str() {
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn nip17_signal_is_gift_wrapped_and_unwraps_for_only_the_receiver() {
        let sender = Keys::generate();
        let receiver = Keys::generate();
        let stranger = Keys::generate();
        let content = serde_json::to_string(&SignalMessage::DownloadRequest {
            protocol: "napstr/1".into(),
            request_id: "request-1".into(),
            file_id: "a".repeat(64),
        })
        .unwrap();
        let gift = EventBuilder::private_msg(
            &sender,
            receiver.public_key(),
            content.clone(),
            signal_tags(),
        )
        .await
        .unwrap();
        assert_eq!(gift.kind, Kind::GiftWrap);
        assert!(!gift.content.contains("request-1"));
        let unwrapped = Client::new(receiver).unwrap_gift_wrap(&gift).await.unwrap();
        assert_eq!(unwrapped.sender, sender.public_key());
        assert_eq!(unwrapped.rumor.kind, Kind::PrivateDirectMessage);
        assert_eq!(unwrapped.rumor.content, content);
        assert!(unwrapped.rumor.tags.expiration().is_some());
        assert!(Client::new(stranger).unwrap_gift_wrap(&gift).await.is_err());
    }
}
