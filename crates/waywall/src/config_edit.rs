//! `--save`: 사용자 config.toml을 주석·서식을 보존하며 고친다 (docs/cli-ipc.md#--save).

use std::path::Path;

use anyhow::{Context, Result};
use toml_edit::{DocumentMut, Item, Table, TableLike, Value};

/// 값을 기록할 곳. 기록한 키는 그보다 구체적인 섹션에서 지워서, 기록한 값이 실제로 적용되게 한다.
#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    /// 최상위 키. `[default]`, `[output.*]`의 같은 키를 지운다.
    TopLevel,
    /// `[default]`. `[output.*]`의 같은 키를 지운다.
    Default,
    /// `[output."X"]`.
    Output(&'a str),
}

const SECTION_DEFAULT: &str = "default";
const SECTION_OUTPUT: &str = "output";

pub fn save(path: &Path, target: Target<'_>, values: &[(&str, Value)]) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).with_context(|| format!("failed to read {}", path.display())),
    };
    let mut doc: DocumentMut = text
        .parse()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    edit(&mut doc, target, values)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    std::fs::write(path, doc.to_string())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn edit(doc: &mut DocumentMut, target: Target<'_>, values: &[(&str, Value)]) -> Result<()> {
    let root = doc.as_table_mut();
    let keys: Vec<&str> = values.iter().map(|(k, _)| *k).collect();
    let section: &mut dyn TableLike = match target {
        Target::TopLevel => {
            if let Some(t) = root
                .get_mut(SECTION_DEFAULT)
                .and_then(Item::as_table_like_mut)
            {
                remove(t, &keys);
            }
            remove_from_outputs(root, &keys);
            root
        }
        Target::Default => {
            remove_from_outputs(root, &keys);
            subtable(root, SECTION_DEFAULT, false)?
        }
        Target::Output(name) => {
            let outputs = subtable(root, SECTION_OUTPUT, true)?;
            subtable(outputs, name, false)?
        }
    };
    for (key, value) in values {
        // 이미 있는 키는 값만 바꿔서 키 앞의 주석과 줄 끝 주석을 보존한다.
        if let Some(old) = section.get_mut(key).and_then(Item::as_value_mut) {
            let decor = old.decor().clone();
            *old = value.clone();
            *old.decor_mut() = decor;
        } else {
            section.insert(key, Item::Value(value.clone()));
        }
    }
    Ok(())
}

fn subtable<'a>(
    parent: &'a mut dyn TableLike,
    key: &str,
    implicit: bool,
) -> Result<&'a mut dyn TableLike> {
    parent
        .entry(key)
        .or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(implicit);
            Item::Table(t)
        })
        .as_table_like_mut()
        .with_context(|| format!("`{key}` in the config file is not a table"))
}

fn remove_from_outputs(root: &mut Table, keys: &[&str]) {
    let Some(outputs) = root
        .get_mut(SECTION_OUTPUT)
        .and_then(Item::as_table_like_mut)
    else {
        return;
    };
    for (_, item) in outputs.iter_mut() {
        if let Some(t) = item.as_table_like_mut() {
            remove(t, keys);
        }
    }
}

fn remove(t: &mut dyn TableLike, keys: &[&str]) {
    for k in keys {
        t.remove(k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &str, target: Target<'_>, values: &[(&str, Value)]) -> String {
        let mut doc: DocumentMut = input.parse().unwrap();
        edit(&mut doc, target, values).unwrap();
        doc.to_string()
    }

    #[test]
    fn output_section_created() {
        let got = run(
            "# my config\nvolume = 40\n",
            Target::Output("DP-1"),
            &[("wallpaper", "/w/a.png".into())],
        );
        assert_eq!(
            got,
            "# my config\nvolume = 40\n\n[output.DP-1]\nwallpaper = \"/w/a.png\"\n"
        );
    }

    #[test]
    fn existing_output_keeps_comments() {
        let got = run(
            "[output.\"eDP-1\"]\n# keep me\nwallpaper = \"old.png\" # old\nfit = \"contain\"\n",
            Target::Output("eDP-1"),
            &[("wallpaper", "new.png".into())],
        );
        assert!(got.contains("# keep me"), "{got}");
        assert!(got.contains("wallpaper = \"new.png\""), "{got}");
        assert!(got.contains("fit = \"contain\""), "{got}");
    }

    #[test]
    fn default_clears_outputs() {
        let got = run(
            "[default]\nwallpaper = \"a.png\"\n[output.A]\nwallpaper = \"b.png\"\nfit = \"fill\"\n",
            Target::Default,
            &[("wallpaper", "c.png".into())],
        );
        let t: toml::Table = got.parse().unwrap();
        assert_eq!(t["default"]["wallpaper"].as_str(), Some("c.png"));
        assert!(t["output"]["A"].get("wallpaper").is_none());
        assert_eq!(t["output"]["A"]["fit"].as_str(), Some("fill"));
    }

    #[test]
    fn top_level_clears_sections() {
        let got = run(
            "volume = 10\n[default]\nvolume = 20\n[output.A]\nvolume = 30\nmute = true\n",
            Target::TopLevel,
            &[("volume", 70.into()), ("mute", false.into())],
        );
        let t: toml::Table = got.parse().unwrap();
        assert_eq!(t["volume"].as_integer(), Some(70));
        assert_eq!(t["mute"].as_bool(), Some(false));
        assert!(t["default"].get("volume").is_none());
        assert!(t["output"]["A"].get("volume").is_none());
        assert!(t["output"]["A"].get("mute").is_none());
    }
}
