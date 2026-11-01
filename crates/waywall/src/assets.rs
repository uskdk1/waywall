//! 내장 asset과 사용자 asset 디렉터리 (docs/config.md#assets).
//!
//! 상대 경로 요청은 사용자 `assets.dir` → 바이너리에 내장된 `assets/` 순으로 찾는다.

use std::borrow::Cow;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use include_dir::{Dir, include_dir};

static BUILTIN: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/../../assets");

const DEFAULTS: &str = "defaults.toml";

pub fn defaults_toml() -> &'static str {
    BUILTIN
        .get_file(DEFAULTS)
        .and_then(|f| f.contents_utf8())
        .expect("assets/defaults.toml is embedded at build time")
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // M3(셰이더)부터 사용
pub struct Assets {
    user_dir: PathBuf,
}

#[allow(dead_code)]
impl Assets {
    pub fn new(user_dir: PathBuf) -> Self {
        Self { user_dir }
    }

    pub fn read(&self, rel: &str) -> Result<Cow<'static, [u8]>> {
        let user = self.user_dir.join(rel);
        if user.is_file() {
            return Ok(Cow::Owned(std::fs::read(&user)?));
        }
        BUILTIN
            .get_file(rel)
            .map(|f| Cow::Borrowed(f.contents()))
            .ok_or_else(|| {
                anyhow!(
                    "asset not found: {rel} (looked in {})",
                    self.user_dir.display()
                )
            })
    }
}
