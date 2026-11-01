//! 설정 계층 병합 (docs/config.md#계층과-우선순위).
//!
//! ```text
//! defaults.toml < project.toml < 사용자 전역/[types.*] < [default] < [output."X"] < CLI
//! ```
//!
//! 병합은 `toml::Table` 수준에서 하고, 마지막에 엄격한 스키마로 역직렬화한다.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use toml::{Table, Value};
use waywall_proto::config::{Appearance, Config};
use waywall_proto::project::WallpaperKind;

use crate::assets;

/// `[default]` / `[output."X"]`에서 타입 옵션이 아닌 키.
const OUTPUT_KEYS: [&str; 6] = [
    "wallpaper",
    "volume",
    "mute",
    "properties",
    "cursor",
    "appearance",
];
/// 출력별로 덮어쓸 수 있는 `[cursor]` 키. theme/size는 포인터 전체에 하나만 적용된다.
const OUTPUT_CURSOR_KEYS: [&str; 1] = ["shape"];
const SECTION_DEFAULT: &str = "default";
const SECTION_OUTPUT: &str = "output";

#[derive(Debug, Clone)]
pub struct Layers {
    defaults: Table,
    /// `[default]`, `[output]`을 뺀 사용자 전역 설정.
    user: Table,
    user_default: Table,
    user_outputs: Table,
    /// 사용자 설정 파일 위치. 상대 경로 배경의 기준.
    path: Option<PathBuf>,
}

/// 출력 하나에 적용되는 값.
#[derive(Debug, Clone)]
pub struct OutputConfig {
    pub wallpaper: Option<String>,
    pub volume: u8,
    pub mute: bool,
    pub cursor_shape: String,
    pub appearance: Appearance,
    pub properties: Table,
    /// 타입 옵션 덮어쓰기 (`[default]` < `[output."X"]`).
    options: Table,
}

impl Layers {
    pub fn load(explicit: Option<&Path>) -> Result<Self> {
        let defaults = parse(assets::defaults_toml(), "built-in defaults.toml")?;
        let (path, user) = match explicit {
            Some(p) => (Some(p.to_owned()), Some(read(p)?)),
            None => match default_config_path() {
                Some(p) if p.exists() => (Some(p.clone()), Some(read(&p)?)),
                p => (p, None),
            },
        };
        let layers = Self::from_tables(defaults, user.unwrap_or_default(), path)?;
        layers.validate()?;
        Ok(layers)
    }

    #[cfg(test)]
    pub(crate) fn load_for_test(user: &str) -> Self {
        let defaults = parse(assets::defaults_toml(), "defaults").unwrap();
        let l = Self::from_tables(defaults, user.parse().unwrap(), None).unwrap();
        l.validate().unwrap();
        l
    }

    fn from_tables(defaults: Table, mut user: Table, path: Option<PathBuf>) -> Result<Self> {
        let mut take = |key: &str| -> Result<Table> {
            match user.remove(key) {
                None => Ok(Table::new()),
                Some(Value::Table(t)) => Ok(t),
                Some(_) => bail!("[{key}] must be a table"),
            }
        };
        let user_default = take(SECTION_DEFAULT)?;
        let user_outputs = take(SECTION_OUTPUT)?;
        Ok(Self {
            defaults,
            user,
            user_default,
            user_outputs,
            path,
        })
    }

    pub fn config_path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// 상대 경로를 해석할 기준 디렉터리.
    pub fn base_dir(&self) -> PathBuf {
        self.path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_owned)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default()
    }

    pub fn global(&self) -> Result<Config> {
        let mut merged = self.defaults.clone();
        merge(&mut merged, &self.user);
        Table::try_into(merged).context("invalid configuration")
    }

    pub fn output(&self, name: &str) -> Result<OutputConfig> {
        let global = self.global()?;
        let mut section = self.user_default.clone();
        if let Some(Value::Table(t)) = self.user_outputs.get(name) {
            merge(&mut section, t);
        }

        let mut appearance = Table::try_from(&global.appearance)?;
        let mut cursor_shape = global.cursor.shape.clone();
        let mut out = OutputConfig {
            wallpaper: None,
            volume: global.volume,
            mute: global.mute,
            cursor_shape: String::new(),
            appearance: global.appearance,
            properties: Table::new(),
            options: Table::new(),
        };
        for (key, value) in section {
            let ctx = || format!("[output.\"{name}\"].{key}");
            match key.as_str() {
                "wallpaper" => out.wallpaper = Some(value.try_into().with_context(ctx)?),
                "volume" => out.volume = value.try_into().with_context(ctx)?,
                "mute" => out.mute = value.try_into().with_context(ctx)?,
                "properties" => out.properties = value.try_into().with_context(ctx)?,
                "cursor" => {
                    let t: Table = value.try_into().with_context(ctx)?;
                    if let Some(shape) = t.get("shape") {
                        cursor_shape = shape.clone().try_into().with_context(ctx)?;
                    }
                }
                "appearance" => {
                    let t: Table = value.try_into().with_context(ctx)?;
                    merge(&mut appearance, &t);
                }
                _ => {
                    out.options.insert(key, value);
                }
            }
        }
        out.cursor_shape = cursor_shape;
        out.appearance = appearance
            .try_into()
            .with_context(|| format!("[output.\"{name}\"].appearance"))?;
        Ok(out)
    }

    /// 배경 타입 옵션: defaults < project < 사용자 [types.K] < 출력 덮어쓰기.
    pub fn type_options<T: DeserializeOwned>(
        &self,
        kind: WallpaperKind,
        project: Option<&Table>,
        output: &OutputConfig,
    ) -> Result<T> {
        let key = kind.key();
        let mut merged = types_table(&self.defaults, key)
            .cloned()
            .unwrap_or_default();
        if let Some(p) = project {
            merge(&mut merged, p);
        }
        if let Some(t) = types_table(&self.user, key) {
            merge(&mut merged, t);
        }
        let known: BTreeSet<String> = merged.keys().cloned().collect();
        let overrides: Table = output
            .options
            .iter()
            .filter(|(k, _)| known.contains(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        merge(&mut merged, &overrides);
        merged
            .try_into()
            .with_context(|| format!("invalid options for {key} wallpaper"))
    }

    /// 전역 스키마와 모든 출력 섹션을 미리 검사해서, 오타를 시작 시점에 알린다.
    fn validate(&self) -> Result<()> {
        let global = self.global()?;
        crate::cursor::validate_shape(&global.cursor.shape).context("[cursor].shape")?;

        let option_keys: BTreeSet<&str> = WallpaperKind::ALL
            .iter()
            .filter_map(|k| types_table(&self.defaults, k.key()))
            .flat_map(|t| t.keys().map(String::as_str))
            .collect();

        let mut sections = vec![(SECTION_DEFAULT.to_owned(), &self.user_default)];
        for (name, value) in &self.user_outputs {
            let Value::Table(t) = value else {
                bail!("[output.\"{name}\"] must be a table")
            };
            sections.push((format!("output.\"{name}\""), t));
        }
        for (label, table) in sections {
            for key in table.keys() {
                if !OUTPUT_KEYS.contains(&key.as_str()) && !option_keys.contains(key.as_str()) {
                    bail!("[{label}]: unknown key `{key}`");
                }
            }
            if let Some(Value::Table(c)) = table.get("cursor")
                && let Some(k) = c.keys().find(|k| !OUTPUT_CURSOR_KEYS.contains(&k.as_str()))
            {
                bail!("[{label}.cursor]: `{k}` can only be set globally in [cursor]");
            }
        }
        for name in self.user_outputs.keys() {
            let out = self.output(name)?;
            crate::cursor::validate_shape(&out.cursor_shape)
                .with_context(|| format!("[output.\"{name}\"].cursor.shape"))?;
        }
        Ok(())
    }

    /// `waywall config show -o <출력>`용: 그 출력에 실제로 적용되는 값.
    pub fn describe_output(&self, name: &str) -> Result<String> {
        let out = self.output(name)?;
        let mut t = Table::new();
        if let Some(w) = &out.wallpaper {
            t.insert("wallpaper".into(), w.clone().into());
        }
        t.insert("volume".into(), (out.volume as i64).into());
        t.insert("mute".into(), out.mute.into());
        t.insert(
            "cursor".into(),
            Value::Table(Table::from_iter([(
                "shape".into(),
                out.cursor_shape.clone().into(),
            )])),
        );
        t.insert(
            "appearance".into(),
            Value::Table(Table::try_from(&out.appearance)?),
        );
        if !out.properties.is_empty() {
            t.insert("properties".into(), Value::Table(out.properties.clone()));
        }
        if let Some(spec) = &out.wallpaper {
            let detect = self.global()?.detect;
            match crate::wallpaper::resolve(spec, &detect, &self.base_dir()) {
                Ok(wp) => {
                    let opts: Table = self.type_options(wp.kind, wp.options.as_ref(), &out)?;
                    t.insert("type".into(), wp.kind.key().into());
                    t.insert("options".into(), Value::Table(opts));
                }
                Err(e) => {
                    t.insert("error".into(), format!("{e:#}").into());
                }
            }
        }
        Ok(toml::to_string_pretty(&t)?)
    }

    /// `waywall config show`용: 모든 계층을 합친 전역 설정 + 출력 섹션.
    pub fn merged_table(&self) -> Result<Table> {
        let mut t = Table::try_from(self.global()?)?;
        if !self.user_default.is_empty() {
            t.insert(
                SECTION_DEFAULT.into(),
                Value::Table(self.user_default.clone()),
            );
        }
        if !self.user_outputs.is_empty() {
            t.insert(
                SECTION_OUTPUT.into(),
                Value::Table(self.user_outputs.clone()),
            );
        }
        Ok(t)
    }
}

fn types_table<'a>(root: &'a Table, key: &str) -> Option<&'a Table> {
    root.get("types")?.as_table()?.get(key)?.as_table()
}

/// `overlay`를 `base` 위에 재귀적으로 덮어쓴다. 테이블은 병합, 나머지는 교체.
pub fn merge(base: &mut Table, overlay: &Table) {
    for (key, value) in overlay {
        match (base.get_mut(key), value) {
            (Some(Value::Table(b)), Value::Table(o)) => merge(b, o),
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
}

fn parse(text: &str, what: &str) -> Result<Table> {
    text.parse::<Table>()
        .with_context(|| format!("failed to parse {what}"))
}

fn read(path: &Path) -> Result<Table> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    parse(&text, &path.display().to_string())
}

/// XDG Base Directory 규약의 설정 파일 위치.
pub fn default_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("waywall").join("config.toml"))
}

/// `~`, `$VAR`, `${VAR}`, `${VAR:-대체값}`을 확장하고, 상대 경로는 `base` 기준으로 만든다.
pub fn expand_path(s: &str, base: &Path) -> PathBuf {
    let expanded = expand_vars(s);
    let path = match expanded.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => match std::env::var_os("HOME") {
            Some(home) => PathBuf::from(format!("{}{rest}", home.to_string_lossy())),
            None => PathBuf::from(&expanded),
        },
        _ => PathBuf::from(&expanded),
    };
    if path.is_relative() {
        base.join(path)
    } else {
        path
    }
}

fn expand_vars(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if let Some(inner) = rest.strip_prefix('{') {
            let Some(end) = inner.find('}') else {
                out.push('$');
                continue;
            };
            let expr = &inner[..end];
            rest = &inner[end + 1..];
            let (name, fallback) = match expr.split_once(":-") {
                Some((n, f)) => (n, Some(f)),
                None => (expr, None),
            };
            match std::env::var(name).ok().filter(|v| !v.is_empty()) {
                Some(v) => out.push_str(&v),
                None => out.push_str(&fallback.map(expand_vars).unwrap_or_default()),
            }
        } else {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            if end == 0 {
                out.push('$');
                continue;
            }
            out.push_str(&std::env::var(&rest[..end]).unwrap_or_default());
            rest = &rest[end..];
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use waywall_proto::config::{Fill, FillOrPrevious, Fit, ImageOptions};

    fn layers(user: &str) -> Layers {
        Layers::load_for_test(user)
    }

    /// defaults.toml이 스키마의 모든 키를 갖고, 모르는 키가 없어야 한다.
    #[test]
    fn defaults_are_complete() {
        let l = layers("");
        let global = l.global().unwrap();
        let out = l.output("X").unwrap();
        let _: ImageOptions = l.type_options(WallpaperKind::Image, None, &out).unwrap();
        let _: waywall_proto::config::VideoOptions =
            l.type_options(WallpaperKind::Video, None, &out).unwrap();
        let _: waywall_proto::config::ShaderOptions =
            l.type_options(WallpaperKind::Shader, None, &out).unwrap();
        let _: waywall_proto::config::SceneOptions =
            l.type_options(WallpaperKind::Scene, None, &out).unwrap();
        let _: waywall_proto::config::WebOptions =
            l.type_options(WallpaperKind::Web, None, &out).unwrap();
        // 왕복 직렬화 시 defaults와 같은 키 집합이어야 한다 (types/default/output 제외 비교).
        let round: Table = Table::try_from(&global).unwrap();
        let defaults = parse(assets::defaults_toml(), "defaults").unwrap();
        assert_eq!(
            round.keys().collect::<Vec<_>>(),
            defaults.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn unknown_keys_rejected() {
        let defaults = parse(assets::defaults_toml(), "defaults").unwrap();
        for bad in [
            "volum = 3",
            "[render]\nfoo = 1",
            "[default]\nfitt = \"cover\"",
            "[output.A.cursor]\ntheme = \"x\"",
        ] {
            let l = Layers::from_tables(defaults.clone(), bad.parse().unwrap(), None).unwrap();
            assert!(l.validate().is_err(), "{bad} should be rejected");
        }
    }

    /// 사용자는 항상 배경 작성자를 이긴다.
    #[test]
    fn precedence() {
        let project: Table = "fit = \"fill\"\ndownscale = false".parse().unwrap();

        let l = layers("");
        let out = l.output("DP-1").unwrap();
        let o: ImageOptions = l
            .type_options(WallpaperKind::Image, Some(&project), &out)
            .unwrap();
        assert_eq!(o.fit, Fit::Fill, "project beats defaults");
        assert!(!o.downscale);

        let l = layers("[types.image]\nfit = \"contain\"");
        let out = l.output("DP-1").unwrap();
        let o: ImageOptions = l
            .type_options(WallpaperKind::Image, Some(&project), &out)
            .unwrap();
        assert_eq!(o.fit, Fit::Contain, "user [types] beats project");

        let l = layers(
            "[types.image]\nfit = \"contain\"\n[default]\nfit = \"fill\"\n[output.\"DP-1\"]\nfit = \"cover\"",
        );
        let o: ImageOptions = l
            .type_options(
                WallpaperKind::Image,
                Some(&project),
                &l.output("DP-1").unwrap(),
            )
            .unwrap();
        assert_eq!(o.fit, Fit::Cover, "[output] beats [default]");
        let o: ImageOptions = l
            .type_options(
                WallpaperKind::Image,
                Some(&project),
                &l.output("HDMI-A-1").unwrap(),
            )
            .unwrap();
        assert_eq!(o.fit, Fit::Fill, "[default] beats [types]");
    }

    #[test]
    fn output_sections() {
        let l = layers(
            "volume = 40\n[appearance]\nempty = \"#112233\"\n[default]\nwallpaper = \"a.png\"\n\
             [output.\"DP-1\"]\nwallpaper = \"b.png\"\nmute = true\ncursor = { shape = \"crosshair\" }\n\
             appearance = { error = \"#ff0000\" }",
        );
        let a = l.output("eDP-1").unwrap();
        assert_eq!(a.wallpaper.as_deref(), Some("a.png"));
        assert_eq!(a.volume, 40);
        assert!(!a.mute);
        let b = l.output("DP-1").unwrap();
        assert_eq!(b.wallpaper.as_deref(), Some("b.png"));
        assert!(b.mute);
        assert_eq!(b.cursor_shape, "crosshair");
        assert_eq!(b.appearance.empty, Fill::Color("#112233".parse().unwrap()));
        assert_eq!(
            b.appearance.error,
            FillOrPrevious::Fill(Fill::Color("#ff0000".parse().unwrap()))
        );
    }

    #[test]
    fn path_expansion() {
        // SAFETY: 이 테스트만 이 변수를 쓴다.
        unsafe { std::env::set_var("WAYWALL_TEST_DIR", "/tmp/x") };
        let base = Path::new("/base");
        assert_eq!(
            expand_path("${WAYWALL_TEST_DIR}/a", base),
            PathBuf::from("/tmp/x/a")
        );
        assert_eq!(
            expand_path("$WAYWALL_TEST_DIR/a", base),
            PathBuf::from("/tmp/x/a")
        );
        assert_eq!(
            expand_path("${WAYWALL_TEST_UNSET:-/opt}/a", base),
            PathBuf::from("/opt/a")
        );
        assert_eq!(
            expand_path("rel/a.png", base),
            PathBuf::from("/base/rel/a.png")
        );
    }
}
