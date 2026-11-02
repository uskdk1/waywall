//! 웹 헬퍼(`waywall-web`) 프로세스 (docs/architecture.md#웹-헬퍼).
//!
//! 데몬 → 헬퍼는 stdin에 JSON 한 줄씩 쓰고, 헬퍼 → 데몬은 stdout을 읽는 스레드가 calloop 채널로 넘긴다.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command as Process, Stdio};

use anyhow::{Context, Result};
use calloop::channel::Sender;
use serde_json::{Map, Value};
use waywall_proto::web::{Command, Event};

use crate::properties::{PropValue, Property};

/// 헬퍼 경로를 덮어쓰는 환경변수 (docs/development.md#런타임-경로).
const HELPER_ENV: &str = "WAYWALL_WEB_HELPER";
const HELPER_NAME: &str = "waywall-web";

/// 헬퍼 → 메인 루프.
pub enum WebMsg {
    Event(Event),
    /// stdout이 닫혔다 (헬퍼 종료). `pid`로 이미 바꾼 헬퍼의 소식은 무시한다.
    Exited {
        pid: u32,
    },
}

pub struct WebHelper {
    /// `Drop`에서 꺼내 기다린다.
    child: Option<Child>,
    pid: u32,
    stdin: ChildStdin,
}

impl WebHelper {
    pub fn spawn(tx: Sender<WebMsg>) -> Result<Self> {
        let path = helper_path()?;
        let mut child = Process::new(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("failed to start the web helper {}", path.display()))?;
        let stdin = child.stdin.take().context("web helper stdin")?;
        let stdout = child.stdout.take().context("web helper stdout")?;
        let pid = child.id();
        std::thread::Builder::new()
            .name("waywall-web stdout".into())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    match serde_json::from_str::<Event>(&line) {
                        Ok(ev) => {
                            if tx.send(WebMsg::Event(ev)).is_err() {
                                return;
                            }
                        }
                        Err(e) => tracing::warn!("web helper sent `{line}`: {e}"),
                    }
                }
                let _ = tx.send(WebMsg::Exited { pid });
            })?;
        tracing::info!(helper = %path.display(), pid, "web helper started");
        Ok(Self {
            child: Some(child),
            pid,
            stdin,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn send(&mut self, cmd: &Command) {
        let line = match serde_json::to_string(cmd) {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("cannot encode web helper command: {e}");
                return;
            }
        };
        tracing::debug!(command = %line, "to web helper");
        if let Err(e) = writeln!(self.stdin, "{line}").and_then(|()| self.stdin.flush()) {
            // 헬퍼가 죽었으면 stdout 스레드가 곧 Exited를 보낸다.
            tracing::warn!("cannot write to the web helper: {e}");
        }
    }
}

impl Drop for WebHelper {
    fn drop(&mut self) {
        self.send(&Command::Quit);
        // 좀비가 남지 않게 기다리되, 메인 루프는 막지 않는다 (stdin이 닫히면 헬퍼도 끝난다).
        if let Some(mut child) = self.child.take() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

fn helper_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os(HELPER_ENV).filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let exe = std::env::current_exe().context("cannot locate the waywall executable")?;
    let dir = exe.parent().unwrap_or(Path::new("."));
    Ok(dir.join(HELPER_NAME))
}

/// 배경 속성 → `window.waywall.properties` (색은 `"#rrggbb"`, 불투명하지 않으면 `"#rrggbbaa"`).
pub fn properties_json(props: &[Property]) -> Map<String, Value> {
    props
        .iter()
        .map(|p| {
            let v = match p.value {
                PropValue::Float(f) => serde_json::json!(f),
                PropValue::Int(i) => serde_json::json!(i),
                PropValue::Bool(b) => Value::Bool(b),
                PropValue::Color(c) => Value::String(c.to_string()),
            };
            (p.name.clone(), v)
        })
        .collect()
}

/// 로컬 파일 → `file://` URL. 경로 구분자와 비예약 문자 밖은 퍼센트 인코딩한다.
pub fn file_url(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut url = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            url.push(char::from(b));
        } else {
            url.push_str(&format!("%{b:02X}"));
        }
    }
    url
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_are_percent_encoded() {
        assert_eq!(
            file_url(Path::new("/home/m/Wall papers/별.html")),
            "file:///home/m/Wall%20papers/%EB%B3%84.html"
        );
    }

    #[test]
    fn properties_become_json() {
        let props = [
            Property {
                name: "hue".into(),
                value: PropValue::Float(2.5),
            },
            Property {
                name: "tint".into(),
                value: PropValue::Color("#ff0000".parse().unwrap()),
            },
        ];
        let m = properties_json(&props);
        assert_eq!(m["hue"], serde_json::json!(2.5));
        assert_eq!(m["tint"], serde_json::json!("#ff0000"));
    }
}
