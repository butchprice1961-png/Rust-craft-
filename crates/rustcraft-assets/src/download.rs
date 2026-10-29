//! Verified, resumable, cancellable parallel downloads.
//!
//! Every file lands at `<path>.part` first and is renamed into place only
//! after its size and SHA-1 (when the metadata provides them) match, so a
//! crash or cancel never leaves a corrupt file at the final path.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::{StreamExt, TryStreamExt};
use sha1::{Digest, Sha1};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

pub const USER_AGENT: &str = concat!("RustCraft/", env!("CARGO_PKG_VERSION"));

/// HTTPS-only client shared by all metadata and file requests. TLS uses
/// rustls with the OS trust store.
pub fn http_client() -> reqwest::Client {
    http_client_builder()
        .https_only(true)
        .build()
        .expect("HTTP client configuration is static")
}

/// Base client settings; tests use it directly to reach plain-HTTP mocks.
pub fn http_client_builder() -> reqwest::ClientBuilder {
    // Ignored if another crate already installed a provider.
    let _ = rustls::crypto::ring::default_provider().install_default();
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDownload {
    /// Empty when the file is produced by a loader installer rather than
    /// downloaded; it must then already exist.
    pub url: String,
    pub path: PathBuf,
    pub sha1: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("download cancelled")]
    Cancelled,
    #[error("HTTP {status} from {url}")]
    Status { url: String, status: u16 },
    #[error("network error fetching {url}: {source}")]
    Network {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("{}: expected SHA-1 {expected}, got {actual}", path.display())]
    HashMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("{}: expected {expected} bytes, got {actual}", path.display())]
    SizeMismatch {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    #[error("{} is missing and has no download URL (install it with its loader installer)", path.display())]
    NoSource { path: PathBuf },
    #[error("I/O error on {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl DownloadError {
    fn retryable(&self) -> bool {
        match self {
            Self::Network { .. } | Self::HashMismatch { .. } | Self::SizeMismatch { .. } => true,
            Self::Status { status, .. } => *status == 429 || *status >= 500,
            Self::Cancelled | Self::NoSource { .. } | Self::Io { .. } => false,
        }
    }
}

/// Live counters a UI can poll while downloads run.
#[derive(Debug, Default)]
pub struct Progress {
    pub files_total: AtomicUsize,
    pub files_done: AtomicUsize,
    pub bytes_total: AtomicU64,
    pub bytes_done: AtomicU64,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        let total = self.bytes_total.load(Ordering::Relaxed);
        if total == 0 {
            let files = self.files_total.load(Ordering::Relaxed).max(1);
            return self.files_done.load(Ordering::Relaxed) as f32 / files as f32;
        }
        (self.bytes_done.load(Ordering::Relaxed) as f64 / total as f64).min(1.0) as f32
    }
}

#[derive(Debug, Clone)]
pub struct Downloader {
    client: reqwest::Client,
    concurrency: usize,
    retries: u32,
    backoff: Duration,
}

impl Downloader {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            concurrency: 16,
            retries: 4,
            backoff: Duration::from_millis(500),
        }
    }

    pub fn with_backoff(mut self, backoff: Duration) -> Self {
        self.backoff = backoff;
        self
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// GETs a small document into memory, retrying transient failures.
    pub async fn get_bytes(&self, url: &str) -> Result<Vec<u8>, DownloadError> {
        let network = |source| DownloadError::Network {
            url: url.into(),
            source,
        };
        let mut attempt = 0;
        loop {
            let result = async {
                let response = self.client.get(url).send().await.map_err(network)?;
                let status = response.status().as_u16();
                if status != 200 {
                    return Err(DownloadError::Status {
                        url: url.into(),
                        status,
                    });
                }
                Ok(response.bytes().await.map_err(network)?.to_vec())
            }
            .await;
            match result {
                Err(e) if e.retryable() && attempt < self.retries => {
                    tokio::time::sleep(self.backoff * 2u32.pow(attempt)).await;
                    attempt += 1;
                }
                other => return other,
            }
        }
    }

    /// Downloads every file, skipping ones already present and valid.
    /// Stops at the first file that still fails after retries.
    pub async fn fetch_all(
        &self,
        files: Vec<FileDownload>,
        progress: &Arc<Progress>,
        cancel: &CancellationToken,
    ) -> Result<(), DownloadError> {
        progress
            .files_total
            .fetch_add(files.len(), Ordering::Relaxed);
        let known: u64 = files.iter().filter_map(|f| f.size).sum();
        progress.bytes_total.fetch_add(known, Ordering::Relaxed);
        futures_util::stream::iter(files.into_iter().map(Ok))
            .try_for_each_concurrent(self.concurrency, |file| async move {
                self.fetch_one(&file, progress, cancel).await?;
                progress.files_done.fetch_add(1, Ordering::Relaxed);
                Ok(())
            })
            .await
    }

    pub async fn fetch_one(
        &self,
        file: &FileDownload,
        progress: &Progress,
        cancel: &CancellationToken,
    ) -> Result<(), DownloadError> {
        if is_valid(file).await? {
            progress
                .bytes_done
                .fetch_add(file.size.unwrap_or(0), Ordering::Relaxed);
            return Ok(());
        }
        if file.url.is_empty() {
            return Err(DownloadError::NoSource {
                path: file.path.clone(),
            });
        }
        let mut attempt = 0;
        loop {
            let counted = AtomicU64::new(0);
            match self.try_fetch(file, progress, &counted, cancel).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    progress
                        .bytes_done
                        .fetch_sub(counted.load(Ordering::Relaxed), Ordering::Relaxed);
                    if !e.retryable() || attempt >= self.retries {
                        return Err(e);
                    }
                    tracing::warn!("retrying {} after: {e}", file.url);
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
                        _ = tokio::time::sleep(self.backoff * 2u32.pow(attempt)) => {}
                    }
                    attempt += 1;
                }
            }
        }
    }

    async fn try_fetch(
        &self,
        file: &FileDownload,
        progress: &Progress,
        counted: &AtomicU64,
        cancel: &CancellationToken,
    ) -> Result<(), DownloadError> {
        let io_err = |source| DownloadError::Io {
            path: file.path.clone(),
            source,
        };
        let part = part_path(&file.path);
        if let Some(dir) = part.parent() {
            tokio::fs::create_dir_all(dir).await.map_err(io_err)?;
        }

        // Resume from an earlier partial download if one exists.
        let mut hasher = Sha1::new();
        let mut offset = 0;
        if let Ok(existing) = tokio::fs::read(&part).await {
            if file.size.is_some_and(|s| existing.len() as u64 > s) {
                let _ = tokio::fs::remove_file(&part).await;
            } else {
                hasher.update(&existing);
                offset = existing.len() as u64;
            }
        }

        let mut request = self.client.get(&file.url);
        if offset > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let network = |source| DownloadError::Network {
            url: file.url.clone(),
            source,
        };
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
            r = request.send() => r.map_err(network)?,
        };

        let status = response.status().as_u16();
        let append = match status {
            206 if offset > 0 => true,
            200 => false,
            416 if offset > 0 => {
                // The server says the range is past the end: the partial
                // file may already be whole. Verify it; restart if not.
                return match finish(file, &part, offset, hasher).await {
                    Err(e) => {
                        let _ = tokio::fs::remove_file(&part).await;
                        Err(e)
                    }
                    ok => ok,
                };
            }
            _ => {
                return Err(DownloadError::Status {
                    url: file.url.clone(),
                    status,
                });
            }
        };
        if !append {
            hasher = Sha1::new();
            offset = 0;
        }
        progress.bytes_done.fetch_add(offset, Ordering::Relaxed);
        counted.fetch_add(offset, Ordering::Relaxed);

        let mut out = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(&part)
            .await
            .map_err(io_err)?;
        let mut body = response.bytes_stream();
        let mut written = offset;
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
                c = body.next() => c,
            };
            let Some(chunk) = chunk else { break };
            let chunk = chunk.map_err(network)?;
            out.write_all(&chunk).await.map_err(io_err)?;
            hasher.update(&chunk);
            written += chunk.len() as u64;
            progress
                .bytes_done
                .fetch_add(chunk.len() as u64, Ordering::Relaxed);
            counted.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        out.flush().await.map_err(io_err)?;
        out.sync_all().await.map_err(io_err)?;
        drop(out);

        let result = finish(file, &part, written, hasher).await;
        if matches!(
            result,
            Err(DownloadError::HashMismatch { .. } | DownloadError::SizeMismatch { .. })
        ) {
            // Corrupt data must not be resumed from.
            let _ = tokio::fs::remove_file(&part).await;
        }
        result
    }
}

async fn finish(
    file: &FileDownload,
    part: &Path,
    len: u64,
    hasher: Sha1,
) -> Result<(), DownloadError> {
    if let Some(expected) = file.size
        && expected != len
    {
        return Err(DownloadError::SizeMismatch {
            path: file.path.clone(),
            expected,
            actual: len,
        });
    }
    if let Some(expected) = &file.sha1 {
        let actual = hex::encode(hasher.finalize());
        if !actual.eq_ignore_ascii_case(expected) {
            return Err(DownloadError::HashMismatch {
                path: file.path.clone(),
                expected: expected.clone(),
                actual,
            });
        }
    }
    tokio::fs::rename(part, &file.path)
        .await
        .map_err(|source| DownloadError::Io {
            path: file.path.clone(),
            source,
        })
}

fn part_path(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(".part");
    PathBuf::from(p)
}

/// True if `file.path` exists and matches the expected size and hash.
pub async fn is_valid(file: &FileDownload) -> Result<bool, DownloadError> {
    let meta = match tokio::fs::metadata(&file.path).await {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(DownloadError::Io {
                path: file.path.clone(),
                source,
            });
        }
    };
    if file.size.is_some_and(|s| s != meta.len()) {
        return Ok(false);
    }
    let Some(expected) = file.sha1.clone() else {
        return Ok(true);
    };
    let path = file.path.clone();
    let actual = tokio::task::spawn_blocking(move || sha1_file(&path))
        .await
        .expect("hashing task panicked")
        .map_err(|source| DownloadError::Io {
            path: file.path.clone(),
            source,
        })?;
    Ok(actual.eq_ignore_ascii_case(&expected))
}

pub fn sha1_file(path: &Path) -> io::Result<String> {
    let mut hasher = Sha1::new();
    io::copy(&mut std::fs::File::open(path)?, &mut hasher)?;
    Ok(hex::encode(hasher.finalize()))
}

pub fn sha1_hex(bytes: &[u8]) -> String {
    hex::encode(Sha1::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const BODY: &[u8] = b"the quick brown fox jumps over the lazy dog";

    fn downloader() -> Downloader {
        // The mock server is plain HTTP, so skip the https-only client.
        Downloader::new(http_client_builder().build().unwrap())
            .with_backoff(Duration::from_millis(1))
    }

    fn target(dir: &Path, server: &MockServer) -> FileDownload {
        FileDownload {
            url: format!("{}/f", server.uri()),
            path: dir.join("sub/f.bin"),
            sha1: Some(sha1_hex(BODY)),
            size: Some(BODY.len() as u64),
        }
    }

    #[tokio::test]
    async fn retries_server_errors_then_verifies() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/f"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/f"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let file = target(dir.path(), &server);
        let progress = Arc::new(Progress::default());
        downloader()
            .fetch_all(vec![file.clone()], &progress, &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&file.path).unwrap(), BODY);
        assert_eq!(
            progress.bytes_done.load(Ordering::Relaxed),
            BODY.len() as u64
        );
        assert_eq!(progress.files_done.load(Ordering::Relaxed), 1);
        assert!(!part_path(&file.path).exists());
    }

    #[tokio::test]
    async fn corrupt_body_is_rejected_and_not_kept() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(b"the quick brown fox jumps over the lazy cat".as_slice()),
            )
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let file = target(dir.path(), &server);
        let err = downloader()
            .fetch_one(&file, &Progress::default(), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DownloadError::HashMismatch { .. }), "{err}");
        assert!(!file.path.exists());
        assert!(!part_path(&file.path).exists());
        // 1 try + 4 retries.
        assert_eq!(server.received_requests().await.unwrap().len(), 5);
    }

    #[tokio::test]
    async fn resumes_partial_download_with_range() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=10-"))
            .respond_with(ResponseTemplate::new(206).set_body_bytes(&BODY[10..]))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let file = target(dir.path(), &server);
        std::fs::create_dir_all(file.path.parent().unwrap()).unwrap();
        std::fs::write(part_path(&file.path), &BODY[..10]).unwrap();
        downloader()
            .fetch_one(&file, &Progress::default(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&file.path).unwrap(), BODY);
    }

    #[tokio::test]
    async fn server_ignoring_range_restarts_cleanly() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let file = target(dir.path(), &server);
        std::fs::create_dir_all(file.path.parent().unwrap()).unwrap();
        std::fs::write(part_path(&file.path), b"garbage!!").unwrap();
        downloader()
            .fetch_one(&file, &Progress::default(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(std::fs::read(&file.path).unwrap(), BODY);
    }

    #[tokio::test]
    async fn valid_existing_file_is_not_downloaded() {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let file = target(dir.path(), &server);
        std::fs::create_dir_all(file.path.parent().unwrap()).unwrap();
        std::fs::write(&file.path, BODY).unwrap();
        downloader()
            .fetch_one(&file, &Progress::default(), &CancellationToken::new())
            .await
            .unwrap();
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn not_found_fails_fast_and_cancel_stops() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/f"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/slow"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let file = target(dir.path(), &server);
        let err = downloader()
            .fetch_one(&file, &Progress::default(), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DownloadError::Status { status: 404, .. }));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);

        let cancel = CancellationToken::new();
        let slow = FileDownload {
            url: format!("{}/slow", server.uri()),
            ..file
        };
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            canceller.cancel();
        });
        let started = std::time::Instant::now();
        let err = downloader()
            .fetch_one(&slow, &Progress::default(), &cancel)
            .await
            .unwrap_err();
        assert!(matches!(err, DownloadError::Cancelled));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn missing_installer_file_reports_no_source() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileDownload {
            url: String::new(),
            path: dir.path().join("x.jar"),
            sha1: None,
            size: None,
        };
        let err = downloader()
            .fetch_one(&file, &Progress::default(), &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(matches!(err, DownloadError::NoSource { .. }));
    }
}
