//! 배경 속성: `project.toml`의 `[properties.<이름>]` 정의 + 사용자 출력별 값
//! (docs/wallpaper-format.md#디렉터리형-배경-projecttoml).

use anyhow::{Context, Result, bail};
use toml::{Table, Value};
use waywall_proto::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropValue {
    Float(f32),
    Int(i32),
    Bool(bool),
    Color(Color),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub value: PropValue,
}

const TYPE_KEY: &str = "type";
const DEFAULT_KEY: &str = "default";
const MIN_KEY: &str = "min";
const MAX_KEY: &str = "max";

/// 정의 순서대로 최종 값을 만든다. 사용자 값이 기본값을 이기고, 정의되지 않은 사용자 키는 무시한다
/// (출력별 설정은 그 출력의 어떤 배경에도 적용되므로).
pub fn resolve(defs: Option<&Table>, user: &Table) -> Result<Vec<Property>> {
    let Some(defs) = defs else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(defs.len());
    for (name, def) in defs {
        let ctx = || format!("[properties.{name}]");
        if !is_identifier(name) {
            bail!("property name `{name}` must be [A-Za-z_][A-Za-z0-9_]*");
        }
        let def = def
            .as_table()
            .with_context(|| format!("{} must be a table", ctx()))?;
        for key in def.keys() {
            if ![TYPE_KEY, DEFAULT_KEY, MIN_KEY, MAX_KEY].contains(&key.as_str()) {
                bail!("{}: unknown key `{key}`", ctx());
            }
        }
        let ty = def
            .get(TYPE_KEY)
            .and_then(Value::as_str)
            .with_context(|| format!("{}: missing `type`", ctx()))?;
        let default = def
            .get(DEFAULT_KEY)
            .with_context(|| format!("{}: missing `default`", ctx()))?;
        let raw = user.get(name).unwrap_or(default);
        let value = parse(ty, raw).with_context(|| format!("property `{name}`"))?;
        let value = clamp(value, def.get(MIN_KEY), def.get(MAX_KEY));
        out.push(Property {
            name: name.clone(),
            value,
        });
    }
    for key in user.keys().filter(|k| !defs.contains_key(*k)) {
        tracing::debug!(property = %key, "ignoring property not defined by this wallpaper");
    }
    Ok(out)
}

fn parse(ty: &str, v: &Value) -> Result<PropValue> {
    Ok(match ty {
        "float" => PropValue::Float(match v {
            Value::Float(f) => *f as f32,
            Value::Integer(i) => *i as f32,
            _ => bail!("expected a number, got {v}"),
        }),
        "int" => PropValue::Int(
            v.as_integer()
                .with_context(|| format!("expected an integer, got {v}"))?
                .try_into()?,
        ),
        "bool" => PropValue::Bool(
            v.as_bool()
                .with_context(|| format!("expected a bool, got {v}"))?,
        ),
        "color" => PropValue::Color(
            v.as_str()
                .with_context(|| format!("expected a color string, got {v}"))?
                .parse()?,
        ),
        other => bail!("unknown property type `{other}` (float | int | bool | color)"),
    })
}

fn clamp(value: PropValue, min: Option<&Value>, max: Option<&Value>) -> PropValue {
    let num = |v: Option<&Value>| match v {
        Some(Value::Float(f)) => Some(*f),
        Some(Value::Integer(i)) => Some(*i as f64),
        _ => None,
    };
    let (lo, hi) = (num(min), num(max));
    match value {
        PropValue::Float(f) => {
            let f = lo.map_or(f, |lo| f.max(lo as f32));
            PropValue::Float(hi.map_or(f, |hi| f.min(hi as f32)))
        }
        PropValue::Int(i) => {
            let i = lo.map_or(i, |lo| i.max(lo as i32));
            PropValue::Int(hi.map_or(i, |hi| i.min(hi as i32)))
        }
        other => other,
    }
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_beats_default_and_clamps() {
        let defs: Table = "[density]\ntype = \"float\"\ndefault = 1.0\nmin = 0.0\nmax = 4.0\n\
                           [tint]\ntype = \"color\"\ndefault = \"#88aaff\"\n\
                           [on]\ntype = \"bool\"\ndefault = true"
            .parse()
            .unwrap();
        let user: Table = "density = 9\nunknown = 1".parse().unwrap();
        let props = resolve(Some(&defs), &user).unwrap();
        assert_eq!(props[0].value, PropValue::Float(4.0));
        assert_eq!(props[1].value, PropValue::Bool(true));
        assert_eq!(props[2].value, PropValue::Color("#88aaff".parse().unwrap()));
    }

    #[test]
    fn bad_definitions() {
        for bad in [
            "[\"a-b\"]\ntype = \"float\"\ndefault = 1.0",
            "[a]\ntype = \"vec9\"\ndefault = 1.0",
            "[a]\ntype = \"float\"",
            "[a]\ntype = \"float\"\ndefault = 1.0\nstep = 2",
        ] {
            let defs: Table = bad.parse().unwrap();
            assert!(resolve(Some(&defs), &Table::new()).is_err(), "{bad}");
        }
    }
}
