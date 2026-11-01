//! IPC 소켓 (docs/cli-ipc.md#ipc): 데몬 쪽 연결 관리와 CLI 쪽 요청 전송.
//!
//! 데몬은 단일 스레드 calloop에서 소켓을 다룬다. 클라이언트 소켓은 blocking으로 두고, 읽을 수 있다는
//! 알림을 받았을 때만 `read`를 한 번 하므로 루프가 막히지 않는다. 응답은 한 줄짜리라 그대로 쓴다.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use waywall_proto::ipc::{Request, Response};

const SOCKET_ENV: &str = "WAYWALL_SOCKET";
const SOCKET_NAME: &str = "waywall.sock";
/// 소켓은 사용자만 접근할 수 있어야 한다.
const SOCKET_MODE: u32 = 0o600;
/// 한 번의 `read` 크기 (I/O 단위일 뿐, 요청 길이 제한은 아님).
pub const READ_CHUNK: usize = 4096;

pub fn socket_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os(SOCKET_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .with_context(|| format!("XDG_RUNTIME_DIR is not set (or set {SOCKET_ENV})"))?;
    Ok(PathBuf::from(dir).join(SOCKET_NAME))
}

/// 종료 시 소켓 파일을 지운다.
pub struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 소켓을 연다. 다른 데몬이 응답하면 실패, 응답이 없는 낡은 소켓은 지운다.
pub fn bind() -> Result<(UnixListener, SocketGuard)> {
    let path = socket_path()?;
    if path.symlink_metadata().is_ok() {
        if UnixStream::connect(&path).is_ok() {
            bail!("waywall daemon is already running ({})", path.display());
        }
        tracing::debug!(path = %path.display(), "removing stale socket");
        std::fs::remove_file(&path)
            .with_context(|| format!("failed to remove stale socket {}", path.display()))?;
    }
    let listener =
        UnixListener::bind(&path).with_context(|| format!("failed to bind {}", path.display()))?;
    let guard = SocketGuard(path.clone());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(SOCKET_MODE))?;
    listener.set_nonblocking(true)?;
    tracing::info!(path = %path.display(), "IPC socket ready");
    Ok((listener, guard))
}

struct Client {
    writer: UnixStream,
    buf: Vec<u8>,
    /// 지연 응답을 기다리는 중. 그동안 다음 요청은 버퍼에 둔다(응답 순서 유지).
    busy: bool,
    /// 상대가 쓰기를 닫았다. 남은 요청을 처리하고 나면 연결을 버린다.
    eof: bool,
}

#[derive(Default)]
pub struct Clients {
    next: u64,
    map: HashMap<u64, Client>,
}

impl Clients {
    pub fn add(&mut self, writer: UnixStream) -> u64 {
        let id = self.next;
        self.next += 1;
        self.map.insert(
            id,
            Client {
                writer,
                buf: Vec::new(),
                busy: false,
                eof: false,
            },
        );
        id
    }

    pub fn feed(&mut self, id: u64, data: &[u8]) {
        if let Some(c) = self.map.get_mut(&id) {
            c.buf.extend_from_slice(data);
        }
    }

    pub fn eof(&mut self, id: u64) {
        if let Some(c) = self.map.get_mut(&id) {
            c.eof = true;
        }
    }

    /// 처리할 다음 요청 줄. 응답 대기 중이면 `None`.
    pub fn next_line(&mut self, id: u64) -> Option<String> {
        let c = self.map.get_mut(&id).filter(|c| !c.busy)?;
        loop {
            let end = c.buf.iter().position(|&b| b == b'\n')?;
            let line: Vec<u8> = c.buf.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line).trim().to_owned();
            if !line.is_empty() {
                return Some(line);
            }
        }
    }

    pub fn set_busy(&mut self, id: u64, busy: bool) {
        if let Some(c) = self.map.get_mut(&id) {
            c.busy = busy;
        }
    }

    pub fn reply(&mut self, id: u64, resp: &Response) {
        let Some(c) = self.map.get_mut(&id) else {
            return;
        };
        let mut line = match serde_json::to_string(resp) {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("failed to serialize IPC response: {e}");
                return;
            }
        };
        line.push('\n');
        if let Err(e) = c.writer.write_all(line.as_bytes()) {
            tracing::debug!(client = id, "IPC client went away: {e}");
            self.map.remove(&id);
        }
    }

    /// 끝난 연결 정리. 더 처리할 요청도, 기다리는 응답도 없을 때만.
    pub fn reap(&mut self, id: u64) {
        if self.map.get(&id).is_some_and(|c| c.eof && !c.busy) {
            self.map.remove(&id);
        }
    }
}

/// CLI 쪽 실패. 데몬이 없을 때는 종료 코드를 따로 쓴다.
pub enum ClientError {
    NotRunning(PathBuf, std::io::Error),
    Other(anyhow::Error),
}

impl From<anyhow::Error> for ClientError {
    fn from(e: anyhow::Error) -> Self {
        ClientError::Other(e)
    }
}

pub fn request(req: &Request) -> Result<Response, ClientError> {
    let path = socket_path()?;
    let stream =
        UnixStream::connect(&path).map_err(|e| ClientError::NotRunning(path.clone(), e))?;
    send(stream, req, &path).map_err(ClientError::Other)
}

fn send(mut stream: UnixStream, req: &Request, path: &Path) -> Result<Response> {
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .with_context(|| format!("failed to write to {}", path.display()))?;
    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .with_context(|| format!("failed to read from {}", path.display()))?;
    if reply.is_empty() {
        bail!("daemon closed the connection without replying");
    }
    serde_json::from_str(&reply).with_context(|| format!("invalid reply from daemon: {reply}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_and_busy() {
        let (a, _b) = UnixStream::pair().unwrap();
        let mut c = Clients::default();
        let id = c.add(a);
        c.feed(id, b"{\"cmd\":\"list\"}\n\n{\"cmd\":");
        assert_eq!(c.next_line(id).as_deref(), Some("{\"cmd\":\"list\"}"));
        assert_eq!(c.next_line(id), None, "partial line waits");
        c.feed(id, b"\"quit\"}\n");
        c.set_busy(id, true);
        assert_eq!(c.next_line(id), None, "busy holds later requests");
        c.set_busy(id, false);
        assert_eq!(c.next_line(id).as_deref(), Some("{\"cmd\":\"quit\"}"));
        c.eof(id);
        c.reap(id);
        assert!(c.map.is_empty());
    }
}
