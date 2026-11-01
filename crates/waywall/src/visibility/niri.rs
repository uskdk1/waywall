//! niri IPC 신호원 (컴포지터 전용, docs/protocols.md#최후-수단-niri-ipc-컴포지터-전용).
//!
//! `$NIRI_SOCKET`에 `"EventStream"`을 보내고 이벤트를 받는다. 필요한 필드만 `serde_json::Value`로
//! 느슨하게 읽어서 niri 버전 차이를 견딘다.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;

use anyhow::{Context, Result, bail};
use calloop::channel::Sender;
use serde_json::Value;

const SOCKET_ENV: &str = "NIRI_SOCKET";
const EVENT_STREAM_REQUEST: &str = "\"EventStream\"\n";

pub enum NiriMsg {
    /// 출력 이름 → 활성 워크스페이스에 창이 있는지.
    Outputs(HashMap<String, bool>),
    /// 연결이 끊겼다. 이 신호원은 꺼진다.
    Gone(String),
}

#[derive(Debug, Default)]
struct Workspace {
    output: Option<String>,
    active: bool,
}

/// 이벤트를 반영한 niri 상태.
#[derive(Debug, Default)]
pub struct NiriState {
    workspaces: HashMap<u64, Workspace>,
    /// 창 id → 워크스페이스 id.
    windows: HashMap<u64, Option<u64>>,
}

impl NiriState {
    pub fn apply(&mut self, event: &Value) {
        let Some((name, body)) = event.as_object().and_then(|o| o.iter().next()) else {
            return;
        };
        match name.as_str() {
            "WorkspacesChanged" => {
                self.workspaces = body["workspaces"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|w| {
                        Some((
                            w["id"].as_u64()?,
                            Workspace {
                                output: w["output"].as_str().map(str::to_owned),
                                active: w["is_active"].as_bool().unwrap_or(false),
                            },
                        ))
                    })
                    .collect();
            }
            "WorkspaceActivated" => {
                let Some(id) = body["id"].as_u64() else {
                    return;
                };
                let output = self.workspaces.get(&id).and_then(|w| w.output.clone());
                for (wid, w) in &mut self.workspaces {
                    if w.output == output {
                        w.active = *wid == id;
                    }
                }
            }
            "WindowsChanged" => {
                self.windows = body["windows"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|w| Some((w["id"].as_u64()?, w["workspace_id"].as_u64())))
                    .collect();
            }
            "WindowOpenedOrChanged" => {
                let w = &body["window"];
                if let Some(id) = w["id"].as_u64() {
                    self.windows.insert(id, w["workspace_id"].as_u64());
                }
            }
            "WindowClosed" => {
                if let Some(id) = body["id"].as_u64() {
                    self.windows.remove(&id);
                }
            }
            _ => {}
        }
    }

    pub fn outputs(&self) -> HashMap<String, bool> {
        let mut out = HashMap::new();
        for (id, w) in &self.workspaces {
            let Some(output) = &w.output else { continue };
            let busy = w.active && self.windows.values().any(|ws| *ws == Some(*id));
            let e = out.entry(output.clone()).or_insert(false);
            *e |= busy;
        }
        out
    }
}

/// 이벤트 스트림 스레드를 시작한다. 바뀔 때만 보낸다.
pub fn spawn(tx: Sender<NiriMsg>) -> Result<()> {
    let path = std::env::var_os(SOCKET_ENV)
        .with_context(|| format!("{SOCKET_ENV} is not set (not running under niri?)"))?;
    let mut stream = UnixStream::connect(&path)
        .with_context(|| format!("failed to connect to {}", path.to_string_lossy()))?;
    stream.write_all(EVENT_STREAM_REQUEST.as_bytes())?;
    std::thread::Builder::new()
        .name("waywall-niri".into())
        .spawn(move || {
            let err = run(stream, &tx).map_or_else(|e| format!("{e:#}"), |()| "closed".into());
            let _ = tx.send(NiriMsg::Gone(err));
        })?;
    tracing::info!("niri IPC event stream connected");
    Ok(())
}

fn run(stream: UnixStream, tx: &Sender<NiriMsg>) -> Result<()> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let reply: Value = serde_json::from_str(&line).context("invalid niri reply")?;
    if reply.get("Ok").is_none() {
        bail!("niri refused the event stream: {line}");
    }
    let mut state = NiriState::default();
    let mut last = HashMap::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        state.apply(&event);
        let now = state.outputs();
        if now != last {
            tx.send(NiriMsg::Outputs(now.clone()))?;
            last = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    /// `niri msg -j event-stream`에서 본 형태.
    #[test]
    fn active_workspace_windows() {
        let mut s = NiriState::default();
        s.apply(&ev(r#"{"WorkspacesChanged":{"workspaces":[
            {"id":1,"idx":1,"output":"eDP-1","is_active":true},
            {"id":2,"idx":1,"output":"HDMI-A-1","is_active":true},
            {"id":3,"idx":2,"output":"HDMI-A-1","is_active":false}]}}"#));
        s.apply(&ev(r#"{"WindowsChanged":{"windows":[{"id":10,"workspace_id":2},{"id":11,"workspace_id":3}]}}"#));
        let o = s.outputs();
        assert!(!o["eDP-1"]);
        assert!(o["HDMI-A-1"]);

        s.apply(&ev(r#"{"WorkspaceActivated":{"id":3,"focused":true}}"#));
        assert!(s.outputs()["HDMI-A-1"], "workspace 3 also has a window");
        s.apply(&ev(r#"{"WindowClosed":{"id":11}}"#));
        assert!(!s.outputs()["HDMI-A-1"]);
        s.apply(&ev(
            r#"{"WindowOpenedOrChanged":{"window":{"id":12,"workspace_id":1}}}"#,
        ));
        assert!(s.outputs()["eDP-1"]);
        // 모르는 이벤트는 무시.
        s.apply(&ev(r#"{"KeyboardLayoutsChanged":{"keyboard_layouts":{}}}"#));
    }
}
