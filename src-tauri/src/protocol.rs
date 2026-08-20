use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_CONTROL_FRAME: usize = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClientFrame {
    Hello {
        version: u16,
        capability: String,
        file_id: String,
    },
    RequestChunk {
        index: u32,
    },
    TransferComplete,
    Cancel,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ServerFrame {
    Welcome {
        version: u16,
        file_id: String,
        filename: String,
        size: u64,
        chunk_size: u32,
        chunk_hashes: Vec<String>,
    },
    ChunkData {
        index: u32,
        size: u32,
        sha256: String,
    },
    TransferComplete,
    Error {
        code: String,
        message: String,
    },
}

pub async fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    if payload.len() > MAX_CONTROL_FRAME {
        return Err("control frame exceeds protocol limit".into());
    }
    writer
        .write_u32(payload.len() as u32)
        .await
        .map_err(|error| error.to_string())?;
    writer
        .write_all(&payload)
        .await
        .map_err(|error| error.to_string())?;
    writer.flush().await.map_err(|error| error.to_string())
}

pub async fn read_frame<R, T>(reader: &mut R) -> Result<T, String>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let size = reader.read_u32().await.map_err(|error| error.to_string())? as usize;
    if size == 0 || size > MAX_CONTROL_FRAME {
        return Err("invalid control frame size".into());
    }
    let mut payload = vec![0u8; size];
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|error| error.to_string())?;
    serde_json::from_slice(&payload).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn round_trips_a_protocol_frame() {
        let (mut left, mut right) = duplex(2048);
        let sender = tokio::spawn(async move {
            write_frame(&mut left, &ClientFrame::RequestChunk { index: 42 })
                .await
                .unwrap();
        });
        let frame: ClientFrame = read_frame(&mut right).await.unwrap();
        sender.await.unwrap();
        assert!(matches!(frame, ClientFrame::RequestChunk { index: 42 }));
    }
}
