//! CLI 계층: `set` / `volume` / `pause`로 바꾼 값 (docs/cli-ipc.md#런타임-덮어쓰기).
//!
//! 출력 이름 기준으로 기억하므로 모니터를 다시 연결해도 복원된다.

use std::collections::HashMap;

use crate::config::OutputConfig;

#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub wallpaper: Option<String>,
    pub volume: Option<u8>,
    pub mute: Option<bool>,
    pub paused: Option<bool>,
}

#[derive(Debug, Default)]
pub struct Runtime {
    /// `-o` 없이 지정한 값.
    all: Overrides,
    outputs: HashMap<String, Overrides>,
}

pub type Field<T> = fn(&mut Overrides) -> &mut Option<T>;

pub fn wallpaper(o: &mut Overrides) -> &mut Option<String> {
    &mut o.wallpaper
}

pub fn volume(o: &mut Overrides) -> &mut Option<u8> {
    &mut o.volume
}

pub fn mute(o: &mut Overrides) -> &mut Option<bool> {
    &mut o.mute
}

pub fn paused(o: &mut Overrides) -> &mut Option<bool> {
    &mut o.paused
}

impl Runtime {
    /// `output`이 `None`이면 모든 출력 값으로 두고, 출력별로 지정했던 같은 키는 지운다.
    pub fn set<T>(&mut self, output: Option<&str>, field: Field<T>, value: T) {
        match output {
            None => {
                *field(&mut self.all) = Some(value);
                for o in self.outputs.values_mut() {
                    *field(o) = None;
                }
            }
            Some(name) => *field(self.outputs.entry(name.to_owned()).or_default()) = Some(value),
        }
    }

    fn get<T: Clone>(&mut self, name: &str, field: Field<T>) -> Option<T> {
        let specific = self.outputs.get_mut(name).and_then(|o| field(o).clone());
        specific.or_else(|| field(&mut self.all).clone())
    }

    /// 설정 파일 값 위에 CLI 값을 덮어쓴다.
    pub fn apply(&mut self, name: &str, cfg: &mut OutputConfig) {
        if let Some(w) = self.get(name, wallpaper) {
            cfg.wallpaper = Some(w);
        }
        if let Some(v) = self.get(name, volume) {
            cfg.volume = v;
        }
        if let Some(m) = self.get(name, mute) {
            cfg.mute = m;
        }
    }

    pub fn is_paused(&mut self, name: &str) -> bool {
        self.get(name, paused).unwrap_or(false)
    }

    /// `reload`: 설정 성격의 값만 지운다. 일시정지는 상태이므로 유지.
    pub fn clear_settings(&mut self) {
        for o in std::iter::once(&mut self.all).chain(self.outputs.values_mut()) {
            o.wallpaper = None;
            o.volume = None;
            o.mute = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Layers;

    fn cfg() -> OutputConfig {
        Layers::load_for_test("[default]\nwallpaper = \"file.png\"")
            .output("X")
            .unwrap()
    }

    #[test]
    fn all_clears_specific() {
        let mut r = Runtime::default();
        r.set(Some("A"), wallpaper, "a.png".to_owned());
        let mut c = cfg();
        r.apply("A", &mut c);
        assert_eq!(c.wallpaper.as_deref(), Some("a.png"));
        let mut c = cfg();
        r.apply("B", &mut c);
        assert_eq!(c.wallpaper.as_deref(), Some("file.png"));

        r.set(None, wallpaper, "all.png".to_owned());
        for name in ["A", "B", "later"] {
            let mut c = cfg();
            r.apply(name, &mut c);
            assert_eq!(c.wallpaper.as_deref(), Some("all.png"), "{name}");
        }
    }

    #[test]
    fn reload_keeps_pause() {
        let mut r = Runtime::default();
        r.set(Some("A"), paused, true);
        r.set(None, volume, 10);
        r.clear_settings();
        assert!(r.is_paused("A"));
        assert!(!r.is_paused("B"));
        let mut c = cfg();
        r.apply("A", &mut c);
        assert_eq!(c.volume, cfg().volume);
    }
}
