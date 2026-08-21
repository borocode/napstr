use crate::{audio, hash_file, open_db, playable_audio_path, AppState};
use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, Source};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use tauri::State;

#[derive(Default)]
pub struct NativePlayer {
    inner: Mutex<NativePlayerState>,
}

#[derive(Default)]
struct NativePlayerState {
    output: Option<MixerDeviceSink>,
    player: Option<Player>,
    file_id: Option<String>,
    duration: f64,
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackStatus {
    file_id: String,
    current_time: f64,
    duration: f64,
    playing: bool,
    ended: bool,
}

impl NativePlayerState {
    fn status(&self) -> PlaybackStatus {
        let ended = self
            .player
            .as_ref()
            .map(|player| player.empty())
            .unwrap_or(false);
        PlaybackStatus {
            file_id: self.file_id.clone().unwrap_or_default(),
            current_time: self
                .player
                .as_ref()
                .map(|player| player.get_pos().as_secs_f64())
                .unwrap_or(0.0)
                .min(self.duration),
            duration: self.duration,
            playing: self
                .player
                .as_ref()
                .map(|player| !player.is_paused() && !player.empty())
                .unwrap_or(false),
            ended,
        }
    }
}

#[tauri::command]
pub fn play_audio(
    file_id: String,
    volume: f32,
    state: State<'_, AppState>,
) -> Result<PlaybackStatus, String> {
    validate_file_id(&file_id)?;
    let connection = open_db(&state)?;
    let path = playable_audio_path(&connection, &file_id)?;
    let path = validated_path(&file_id, &path)?;
    let file = File::open(&path)
        .map_err(|error| format!("could not open this track for playback: {error}"))?;
    let decoder = Decoder::try_from(file)
        .map_err(|error| format!("could not decode this validated audio file: {error}"))?;
    let duration = decoder
        .total_duration()
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0);

    let mut native = state
        .player
        .inner
        .lock()
        .map_err(|_| "audio player lock poisoned")?;

    // Keep exactly one device stream for the lifetime of the app. Opening the
    // next stream before dropping the previous one can leave the new stream
    // silent on exclusive or compatibility audio devices.
    if native.output.is_none() {
        let mut output = DeviceSinkBuilder::open_default_sink()
            .map_err(|error| format!("could not open the system audio output: {error}"))?;
        output.log_on_drop(false);
        native.output = Some(output);
    }
    if let Some(previous) = native.player.take() {
        previous.stop();
    }
    let output = native
        .output
        .as_ref()
        .ok_or_else(|| "the audio output was not initialised".to_string())?;
    let player = Player::connect_new(output.mixer());
    player.set_volume(volume.clamp(0.0, 1.0));
    player.append(decoder);
    player.play();

    native.player = Some(player);
    native.file_id = Some(file_id);
    native.duration = duration;
    Ok(native.status())
}

#[tauri::command]
pub fn toggle_audio(state: State<'_, AppState>) -> Result<PlaybackStatus, String> {
    let native = state
        .player
        .inner
        .lock()
        .map_err(|_| "audio player lock poisoned")?;
    let player = native
        .player
        .as_ref()
        .ok_or_else(|| "no track is loaded".to_string())?;
    if player.is_paused() {
        player.play();
    } else {
        player.pause();
    }
    Ok(native.status())
}

#[tauri::command]
pub fn stop_audio(state: State<'_, AppState>) -> Result<PlaybackStatus, String> {
    let native = state
        .player
        .inner
        .lock()
        .map_err(|_| "audio player lock poisoned")?;
    if let Some(player) = native.player.as_ref() {
        player.pause();
        player
            .try_seek(Duration::ZERO)
            .map_err(|error| format!("could not rewind this track: {error}"))?;
    }
    Ok(native.status())
}

#[tauri::command]
pub fn seek_audio(seconds: f64, state: State<'_, AppState>) -> Result<PlaybackStatus, String> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err("invalid playback position".into());
    }
    let native = state
        .player
        .inner
        .lock()
        .map_err(|_| "audio player lock poisoned")?;
    let player = native
        .player
        .as_ref()
        .ok_or_else(|| "no track is loaded".to_string())?;
    player
        .try_seek(Duration::from_secs_f64(seconds.min(native.duration)))
        .map_err(|error| format!("could not seek in this track: {error}"))?;
    Ok(native.status())
}

#[tauri::command]
pub fn set_audio_volume(volume: f32, state: State<'_, AppState>) -> Result<PlaybackStatus, String> {
    if !volume.is_finite() {
        return Err("invalid playback volume".into());
    }
    let native = state
        .player
        .inner
        .lock()
        .map_err(|_| "audio player lock poisoned")?;
    if let Some(player) = native.player.as_ref() {
        player.set_volume(volume.clamp(0.0, 1.0));
    }
    Ok(native.status())
}

#[tauri::command]
pub fn audio_status(state: State<'_, AppState>) -> Result<PlaybackStatus, String> {
    let native = state
        .player
        .inner
        .lock()
        .map_err(|_| "audio player lock poisoned")?;
    Ok(native.status())
}

fn validate_file_id(file_id: &str) -> Result<(), String> {
    if hex::decode(file_id)
        .map(|bytes| bytes.len() == 32)
        .unwrap_or(false)
    {
        Ok(())
    } else {
        Err("invalid SHA-256 file ID".into())
    }
}

fn validated_path(file_id: &str, path: &Path) -> Result<PathBuf, String> {
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    audio::validate_audio(&canonical)
        .map_err(|error| format!("playback rejected by the audio-only policy: {error}"))?;
    let (actual_file_id, _) = hash_file(&canonical)?;
    if actual_file_id != file_id {
        return Err("playback rejected because the file changed after verification".into());
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::validated_path;
    use crate::hash_file;
    use rodio::{Decoder, Source};
    use std::{fs, fs::File};

    #[test]
    fn validates_and_decodes_unchanged_audio() {
        let directory = std::env::temp_dir().join(format!(
            "napstr-player-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("track.wav");
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&40u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0\x44\xac\0\0\x88\x58\x01\0\x02\0\x10\0data\x04\0\0\0song");
        fs::write(&path, &bytes).unwrap();
        let file_id = hash_file(&path).unwrap().0;
        let validated = validated_path(&file_id, &path).unwrap();
        let decoder = Decoder::try_from(File::open(validated).unwrap()).unwrap();
        assert!(decoder.total_duration().is_some());
        assert!(validated_path(&"0".repeat(64), &path).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
