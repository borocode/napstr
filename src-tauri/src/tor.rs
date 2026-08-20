use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    process::{Child, Command},
    sync::Mutex,
    time::{sleep, timeout},
};
use tokio_socks::tcp::Socks5Stream;
use tokio_util::sync::CancellationToken;

struct RunningTor {
    child: Child,
    socks_port: u16,
    control_port: u16,
    cookie: Vec<u8>,
}

pub struct OnionLease {
    pub onion: String,
    _control: TcpStream,
}

pub struct TorManager {
    app_data: PathBuf,
    resource_dir: PathBuf,
    runtime: Mutex<Option<RunningTor>>,
}

pub fn is_v3_onion(host: &str) -> bool {
    host.strip_suffix(".onion")
        .map(|service| {
            service.len() == 56
                && service
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || (b'2'..=b'7').contains(&byte))
        })
        .unwrap_or(false)
}

impl TorManager {
    pub fn new(app_data: PathBuf, resource_dir: PathBuf) -> Self {
        Self {
            app_data,
            resource_dir,
            runtime: Mutex::new(None),
        }
    }

    pub async fn start(&self) -> Result<u16, String> {
        let mut guard = self.runtime.lock().await;
        if let Some(runtime) = guard.as_mut() {
            if runtime
                .child
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_none()
            {
                return Ok(runtime.socks_port);
            }
        }

        let tor = self.find_tor_binary();
        let socks_port = free_port()?;
        let control_port = free_port()?;
        let tor_data = self.app_data.join("tor");
        fs::create_dir_all(&tor_data)
            .await
            .map_err(|error| error.to_string())?;
        let cookie_path = tor_data.join("control_auth_cookie");
        let _ = fs::remove_file(&cookie_path).await;

        let mut command = Command::new(&tor);
        command
            .arg("--DataDirectory")
            .arg(&tor_data)
            .arg("--SocksPort")
            .arg(format!("127.0.0.1:{socks_port}"))
            .arg("--ControlPort")
            .arg(format!("127.0.0.1:{control_port}"))
            .arg("--CookieAuthentication")
            .arg("1")
            .arg("--CookieAuthFile")
            .arg(&cookie_path)
            .arg("--ClientOnly")
            .arg("1")
            .arg("--AvoidDiskWrites")
            .arg("1")
            .arg("--Log")
            .arg("notice stdout")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        if let Some(parent) = tor.parent() {
            prepend_library_path(&mut command, parent);
        }
        command.kill_on_drop(true);

        let mut child = command
            .spawn()
            .map_err(|error| format!("could not start Tor at {}: {error}", tor.display()))?;
        let mut cookie = None;
        for _ in 0..480 {
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                return Err(format!("Tor exited before bootstrap completed: {status}"));
            }
            if let Ok(bytes) = fs::read(&cookie_path).await {
                if TcpStream::connect(("127.0.0.1", control_port))
                    .await
                    .is_ok()
                {
                    let mut control = TcpStream::connect(("127.0.0.1", control_port))
                        .await
                        .map_err(|error| error.to_string())?;
                    if control_command(
                        &mut control,
                        &format!("AUTHENTICATE {}", hex::encode(&bytes)),
                    )
                    .await
                    .is_ok()
                    {
                        if let Ok(lines) =
                            control_command(&mut control, "GETINFO status/bootstrap-phase").await
                        {
                            if lines.iter().any(|line| line.contains("PROGRESS=100")) {
                                cookie = Some(bytes);
                                break;
                            }
                        }
                    }
                }
            }
            sleep(Duration::from_millis(250)).await;
        }
        let cookie = cookie
            .ok_or_else(|| "Tor did not complete bootstrap within 120 seconds".to_string())?;
        *guard = Some(RunningTor {
            child,
            socks_port,
            control_port,
            cookie,
        });
        Ok(socks_port)
    }

    pub async fn stop(&self) {
        if let Some(mut runtime) = self.runtime.lock().await.take() {
            let _ = runtime.child.start_kill();
            let _ = runtime.child.wait().await;
        }
    }

    pub async fn status(&self) -> bool {
        let mut guard = self.runtime.lock().await;
        match guard.as_mut() {
            Some(runtime) => runtime.child.try_wait().ok().flatten().is_none(),
            None => false,
        }
    }

    pub async fn create_onion(&self, target_port: u16) -> Result<Arc<OnionLease>, String> {
        self.start().await?;
        let (control_port, cookie) = {
            let guard = self.runtime.lock().await;
            let runtime = guard.as_ref().ok_or("Tor runtime disappeared")?;
            (runtime.control_port, runtime.cookie.clone())
        };
        let mut stream = TcpStream::connect(("127.0.0.1", control_port))
            .await
            .map_err(|error| error.to_string())?;
        control_command(
            &mut stream,
            &format!("AUTHENTICATE {}", hex::encode(cookie)),
        )
        .await?;
        let response = control_command(
            &mut stream,
            &format!("ADD_ONION NEW:BEST Flags=DiscardPK Port=80,127.0.0.1:{target_port}"),
        )
        .await?;
        let service_id = response
            .iter()
            .find_map(|line| line.strip_prefix("250-ServiceID="))
            .ok_or("Tor did not return a ServiceID")?
            .to_string();
        Ok(Arc::new(OnionLease {
            onion: format!("{service_id}.onion"),
            _control: stream,
        }))
    }

    pub async fn connect_onion(
        &self,
        onion: &str,
        port: u16,
    ) -> Result<Socks5Stream<TcpStream>, String> {
        if !is_v3_onion(onion) {
            return Err("refusing a destination that is not a valid Tor v3 onion".into());
        }
        let socks_port = self.start().await?;
        timeout(
            Duration::from_secs(20),
            Socks5Stream::connect(("127.0.0.1", socks_port), (onion, port)),
        )
        .await
        .map_err(|_| "Tor connection attempt timed out".to_string())?
        .map_err(|error| format!("Tor connection failed: {error}"))
    }

    pub async fn connect_onion_with_retry(
        &self,
        onion: &str,
        port: u16,
        cancel: &CancellationToken,
    ) -> Result<Socks5Stream<TcpStream>, String> {
        let mut last_error = String::new();
        for attempt in 0..8 {
            if cancel.is_cancelled() {
                return Err("cancelled".into());
            }
            match self.connect_onion(onion, port).await {
                Ok(stream) => return Ok(stream),
                Err(error) => last_error = error,
            }
            let delay = Duration::from_secs((attempt + 1).min(5));
            tokio::select! {
                _ = cancel.cancelled() => return Err("cancelled".into()),
                _ = sleep(delay) => {}
            }
        }
        Err(format!(
            "temporary onion service was not reachable: {last_error}"
        ))
    }

    fn find_tor_binary(&self) -> PathBuf {
        if let Ok(value) = std::env::var("NAPSTR_TOR_PATH") {
            return PathBuf::from(value);
        }
        let executable = if cfg!(windows) { "tor.exe" } else { "tor" };
        let platform = if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        };
        for candidate in [
            self.resource_dir
                .join("resources")
                .join("tor")
                .join(platform)
                .join("tor")
                .join(executable),
            self.resource_dir
                .join("resources")
                .join("tor")
                .join(platform)
                .join(executable),
            self.resource_dir
                .join("tor")
                .join(platform)
                .join("tor")
                .join(executable),
            self.resource_dir
                .join("tor")
                .join(platform)
                .join(executable),
            self.resource_dir.join("tor").join(executable),
        ] {
            if candidate.is_file() {
                return candidate;
            }
        }
        PathBuf::from(executable)
    }
}

async fn control_command(stream: &mut TcpStream, command: &str) -> Result<Vec<String>, String> {
    stream
        .write_all(format!("{command}\r\n").as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    stream.flush().await.map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut lines = Vec::new();
    loop {
        let mut line = String::new();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Err("Tor control connection closed unexpectedly".into());
        }
        let line = line.trim_end().to_string();
        if line.starts_with("250") {
            let complete =
                line == "250 OK" || (line.starts_with("250 ") && !line.starts_with("250-"));
            lines.push(line);
            if complete {
                return Ok(lines);
            }
        } else if line.len() >= 3 {
            return Err(format!("Tor control error: {line}"));
        }
    }
}

fn free_port() -> Result<u16, String> {
    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|error| error.to_string())?;
    listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| error.to_string())
}

fn prepend_library_path(command: &mut Command, directory: &Path) {
    #[cfg(target_os = "linux")]
    {
        let value = std::env::var_os("LD_LIBRARY_PATH")
            .map(|existing| format!("{}:{}", directory.display(), existing.to_string_lossy()))
            .unwrap_or_else(|| directory.display().to_string());
        command.env("LD_LIBRARY_PATH", value);
    }
    #[cfg(target_os = "macos")]
    {
        let value = std::env::var_os("DYLD_LIBRARY_PATH")
            .map(|existing| format!("{}:{}", directory.display(), existing.to_string_lossy()))
            .unwrap_or_else(|| directory.display().to_string());
        command.env("DYLD_LIBRARY_PATH", value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    #[test]
    fn accepts_only_v3_onion_hostnames() {
        assert!(is_v3_onion(&format!("{}.onion", "a".repeat(56))));
        assert!(!is_v3_onion("example.com"));
        assert!(!is_v3_onion("short.onion"));
        assert!(!is_v3_onion(&format!("{}.onion.example", "a".repeat(56))));
    }

    #[tokio::test]
    #[ignore = "requires a Tor binary and external Tor network access"]
    async fn ephemeral_onion_round_trip() {
        let directory =
            std::env::temp_dir().join(format!("napstr-tor-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).await.unwrap();
        let manager = TorManager::new(directory.clone(), directory.clone());
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let lease = manager.create_onion(port).await.unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 4];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"ping");
            stream.write_all(b"pong").await.unwrap();
        });
        let cancel = CancellationToken::new();
        let mut client = manager
            .connect_onion_with_retry(&lease.onion, 80, &cancel)
            .await
            .unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut response = [0u8; 4];
        client.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"pong");
        server.await.unwrap();
        drop(lease);
        manager.stop().await;
        let _ = fs::remove_dir_all(directory).await;
    }
}
