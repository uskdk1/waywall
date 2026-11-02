//! 경로/URL → 배경 타입 판별 (docs/wallpaper-format.md).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use toml::Table;
use waywall_proto::config::Detect;
use waywall_proto::project::{Project, WallpaperKind};

use crate::config::expand_path;

#[derive(Debug, Clone)]
pub struct Wallpaper {
    pub kind: WallpaperKind,
    pub source: Source,
    /// 표시 이름 (project.toml의 name 또는 파일 이름).
    pub name: String,
    /// 배경 작성자의 옵션 (project.toml `[options]`).
    pub options: Option<Table>,
    pub properties: Option<Table>,
    /// shader 배경의 `[inputs]`.
    pub inputs: Option<Table>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    File(PathBuf),
    Url(String),
}

pub fn resolve(spec: &str, detect: &Detect, base: &Path) -> Result<Wallpaper> {
    if detect
        .url_schemes
        .iter()
        .any(|s| spec.starts_with(s.as_str()))
    {
        return Ok(Wallpaper {
            kind: WallpaperKind::Web,
            source: Source::Url(spec.to_owned()),
            name: spec.to_owned(),
            options: None,
            properties: None,
            inputs: None,
        });
    }

    let path = expand_path(spec, base);
    if path.is_dir() {
        return resolve_project(&path, detect);
    }
    if !path.is_file() {
        bail!("wallpaper not found: {}", path.display());
    }
    let kind = kind_by_extension(&path, detect)?;
    Ok(Wallpaper {
        kind,
        name: file_name(&path),
        source: Source::File(path),
        options: None,
        properties: None,
        inputs: None,
    })
}

fn resolve_project(dir: &Path, detect: &Detect) -> Result<Wallpaper> {
    let file = dir.join(&detect.project_file);
    let text = std::fs::read_to_string(&file).with_context(|| {
        format!(
            "{} is a directory without {}",
            dir.display(),
            detect.project_file
        )
    })?;
    let project: Project =
        toml::from_str(&text).with_context(|| format!("invalid {}", file.display()))?;
    if project.inputs.is_some() && project.wallpaper.kind != WallpaperKind::Shader {
        bail!("{}: [inputs] is only for type = \"shader\"", file.display());
    }
    let entry = dir.join(&project.wallpaper.entry);
    if !entry.is_file() {
        bail!("{}: entry {} not found", file.display(), entry.display());
    }
    Ok(Wallpaper {
        kind: project.wallpaper.kind,
        source: Source::File(entry),
        name: project.wallpaper.name,
        options: project.options,
        properties: project.properties,
        inputs: project.inputs,
    })
}

fn kind_by_extension(path: &Path, detect: &Detect) -> Result<WallpaperKind> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let table = [
        (WallpaperKind::Image, &detect.image),
        (WallpaperKind::Video, &detect.video),
        (WallpaperKind::Shader, &detect.shader),
        (WallpaperKind::Web, &detect.web),
    ];
    table
        .into_iter()
        .find(|(_, exts)| exts.iter().any(|e| e.eq_ignore_ascii_case(&ext)))
        .map(|(k, _)| k)
        .with_context(|| {
            format!(
                "unknown wallpaper type for {} (see [detect])",
                path.display()
            )
        })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect() -> Detect {
        let l = crate::config::Layers::load_for_test("");
        l.global().unwrap().detect
    }

    #[test]
    fn detection() {
        let dir = std::env::temp_dir().join(format!("waywall-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("proj")).unwrap();
        for f in [
            "a.PNG",
            "b.mp4",
            "c.glsl",
            "d.html",
            "e.xyz",
            "proj/main.html",
        ] {
            std::fs::write(dir.join(f), b"").unwrap();
        }
        std::fs::write(
            dir.join("proj/project.toml"),
            "[wallpaper]\nname = \"P\"\ntype = \"web\"\nentry = \"main.html\"\n[options]\naudio = false\n",
        )
        .unwrap();
        let d = detect();
        let kind = |s: &str| resolve(s, &d, &dir).map(|w| w.kind);
        assert_eq!(kind("a.PNG").unwrap(), WallpaperKind::Image);
        assert_eq!(kind("b.mp4").unwrap(), WallpaperKind::Video);
        assert_eq!(kind("c.glsl").unwrap(), WallpaperKind::Shader);
        assert_eq!(kind("d.html").unwrap(), WallpaperKind::Web);
        assert_eq!(kind("https://example.org").unwrap(), WallpaperKind::Web);
        assert!(kind("e.xyz").is_err());
        assert!(kind("missing.png").is_err());
        let p = resolve("proj", &d, &dir).unwrap();
        assert_eq!(p.kind, WallpaperKind::Web);
        assert_eq!(p.name, "P");
        assert_eq!(p.source, Source::File(dir.join("proj/main.html")));
        assert!(p.options.unwrap().contains_key("audio"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(test)]
mod example_tests {
    use super::*;

    /// `examples/<분류>/<배경>`은 전부 열리고, 옵션·속성이 스키마에 맞아야 한다 (docs/development.md#테스트-방침).
    /// 셰이더 컴파일·씬 로드는 각 백엔드의 테스트가 한다.
    #[test]
    fn examples_resolve() {
        let l = crate::config::Layers::load_for_test("");
        let detect = l.global().unwrap().detect;
        let cfg = l.output("X").unwrap();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples");
        let mut n = 0;
        for group in std::fs::read_dir(&root).unwrap() {
            let group = group.unwrap().path();
            if !group.is_dir() {
                continue;
            }
            for entry in std::fs::read_dir(&group).unwrap() {
                let path = entry.unwrap().path();
                let wp = resolve(path.to_str().unwrap(), &detect, Path::new("/"))
                    .unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
                crate::properties::resolve(wp.properties.as_ref(), &cfg.properties)
                    .unwrap_or_else(|e| panic!("{}: properties: {e:#}", path.display()));
                use waywall_proto::config::{
                    ImageOptions, SceneOptions, ShaderOptions, VideoOptions, WebOptions,
                };
                let o = wp.options.as_ref();
                let checked = match wp.kind {
                    WallpaperKind::Image => {
                        l.type_options::<ImageOptions>(wp.kind, o, &cfg).map(drop)
                    }
                    WallpaperKind::Video => {
                        l.type_options::<VideoOptions>(wp.kind, o, &cfg).map(drop)
                    }
                    WallpaperKind::Shader => {
                        l.type_options::<ShaderOptions>(wp.kind, o, &cfg).map(drop)
                    }
                    WallpaperKind::Scene => {
                        l.type_options::<SceneOptions>(wp.kind, o, &cfg).map(drop)
                    }
                    WallpaperKind::Web => l.type_options::<WebOptions>(wp.kind, o, &cfg).map(drop),
                };
                checked.unwrap_or_else(|e| panic!("{}: options: {e:#}", path.display()));
                n += 1;
            }
        }
        assert!(n >= 9, "found {n} examples");
    }
}
