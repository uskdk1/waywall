//! 데몬 ↔ 웹 헬퍼(`waywall-web`) 프로토콜 (docs/cli-ipc.md#웹-헬퍼-프로토콜).
//!
//! 헬퍼의 stdin(데몬 → 헬퍼)과 stdout(헬퍼 → 데몬)에 한 줄에 하나씩 JSON으로 주고받는다.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::color::Color;
use crate::config::LayerKind;

/// 데몬 → 헬퍼.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Command {
    /// 그 출력에 페이지를 띄운다 (이전 페이지는 바꾼다).
    Show(Box<Show>),
    Hide {
        output: String,
    },
    Pause {
        output: String,
    },
    Resume {
        output: String,
    },
    Volume {
        output: String,
        volume: f64,
        muted: bool,
    },
    Properties {
        output: String,
        properties: Map<String, Value>,
    },
    Quit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Show {
    /// 커넥터 이름 (`DP-1`).
    pub output: String,
    pub url: String,
    pub properties: Map<String, Value>,
    /// 0–1.
    pub volume: f64,
    pub muted: bool,
    pub interactive: bool,
    pub paused: bool,
    /// 페이지가 투명한 부분.
    pub background: Color,
    pub layer: LayerKind,
    pub namespace: String,
    /// 가림 판정: 이 간격마다 프레임 하나를 요청하고, 다음 간격까지 오지 않으면 가려졌다고 본다.
    pub frame_timeout_ms: u64,
}

/// 헬퍼 → 데몬.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ev", rename_all = "kebab-case")]
pub enum Event {
    Ready,
    /// 페이지 로드가 끝났다.
    Shown {
        output: String,
    },
    /// `hide`를 처리했다.
    Hidden {
        output: String,
    },
    /// frame 기아로 판정한 가림 (pause는 데몬이 결정).
    Visibility {
        output: String,
        hidden: bool,
    },
    Error {
        output: String,
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_matches_docs() {
        let c: Command = serde_json::from_str(
            r##"{"op":"show","output":"DP-1","url":"file:///x/index.html","properties":{"density":1.0},
                "volume":0.5,"muted":false,"interactive":true,"paused":false,"background":"#000000",
                "layer":"background","namespace":"waywall","frame_timeout_ms":1500}"##,
        )
        .unwrap();
        let Command::Show(s) = &c else {
            panic!("{c:?}")
        };
        assert_eq!(s.output, "DP-1");
        assert_eq!(s.layer, LayerKind::Background);
        let line = serde_json::to_string(&Command::Hide {
            output: "DP-1".into(),
        })
        .unwrap();
        assert_eq!(line, r#"{"op":"hide","output":"DP-1"}"#);
        assert_eq!(
            serde_json::to_string(&Command::Quit).unwrap(),
            r#"{"op":"quit"}"#
        );
        let e: Event =
            serde_json::from_str(r#"{"ev":"visibility","output":"DP-1","hidden":true}"#).unwrap();
        assert_eq!(
            e,
            Event::Visibility {
                output: "DP-1".into(),
                hidden: true
            }
        );
        assert_eq!(
            serde_json::to_string(&Event::Ready).unwrap(),
            r#"{"ev":"ready"}"#
        );
    }
}
