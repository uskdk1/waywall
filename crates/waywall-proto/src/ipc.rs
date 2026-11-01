//! CLI ↔ 데몬 IPC 메시지 (docs/cli-ipc.md#ipc).
//!
//! JSON-lines: 요청 한 줄, 응답 한 줄.

use serde::{Deserialize, Serialize};

use crate::project::WallpaperKind;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Request {
    Set {
        output: Option<String>,
        path: String,
        save: bool,
    },
    Pause {
        output: Option<String>,
    },
    Resume {
        output: Option<String>,
    },
    Volume {
        output: Option<String>,
        volume: Option<u8>,
        mute: Option<bool>,
        save: bool,
    },
    List,
    Reload,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok() -> Self {
        Self {
            ok: true,
            data: None,
            error: None,
        }
    }

    pub fn data(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

/// `list` 응답의 `data`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListData {
    pub outputs: Vec<OutputStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputStatus {
    pub name: String,
    pub description: Option<String>,
    /// 물리 픽셀.
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub wallpaper: Option<WallpaperInfo>,
    pub render_path: Option<RenderPath>,
    pub volume: u8,
    pub mute: bool,
    pub state: PlayState,
    pub pause_reasons: Vec<PauseReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WallpaperInfo {
    pub path: String,
    /// 판별 전에 실패했으면 `None`.
    pub kind: Option<WallpaperKind>,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RenderPath {
    Solid,
    Shm,
    Dmabuf,
    ShmFallback,
    Gpu,
    Web,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlayState {
    Playing,
    Paused,
    Loading,
    Idle,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PauseReason {
    pub reason: Reason,
    pub source: ReasonSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    User,
    Hidden,
    Covered,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonSource {
    Cli,
    Frame,
    ForeignToplevel,
    Niri,
}

macro_rules! display_as_serde {
    ($($t:ty),*) => {$(
        impl std::fmt::Display for $t {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                match serde_json::to_value(self) {
                    Ok(serde_json::Value::String(s)) => f.write_str(&s),
                    _ => Err(std::fmt::Error),
                }
            }
        }
    )*};
}
display_as_serde!(RenderPath, PlayState, Reason, ReasonSource);

#[cfg(test)]
mod tests {
    use super::*;

    /// docs/cli-ipc.md의 예시 요청이 그대로 파싱되어야 한다.
    #[test]
    fn documented_requests() {
        let cases = [
            (
                r#"{"cmd":"set","output":"DP-1","path":"/w/rain.mp4","save":false}"#,
                Request::Set {
                    output: Some("DP-1".into()),
                    path: "/w/rain.mp4".into(),
                    save: false,
                },
            ),
            (
                r#"{"cmd":"pause","output":null}"#,
                Request::Pause { output: None },
            ),
            (
                r#"{"cmd":"volume","output":null,"volume":30,"mute":null,"save":false}"#,
                Request::Volume {
                    output: None,
                    volume: Some(30),
                    mute: None,
                    save: false,
                },
            ),
            (r#"{"cmd":"list"}"#, Request::List),
            (r#"{"cmd":"quit"}"#, Request::Quit),
        ];
        for (json, want) in cases {
            let got: Request = serde_json::from_str(json).unwrap();
            assert_eq!(got, want, "{json}");
            let back: Request =
                serde_json::from_str(&serde_json::to_string(&got).unwrap()).unwrap();
            assert_eq!(back, want);
        }
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"nope"}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"pause","outptu":"A"}"#).is_err());
    }

    #[test]
    fn response_shape() {
        assert_eq!(
            serde_json::to_string(&Response::error("bad")).unwrap(),
            r#"{"ok":false,"error":"bad"}"#
        );
        assert_eq!(RenderPath::ShmFallback.to_string(), "shm-fallback");
        assert_eq!(
            ReasonSource::ForeignToplevel.to_string(),
            "foreign_toplevel"
        );
    }
}
