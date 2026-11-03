//! 출력 하나의 웹 배경: gtk4-layer-shell 창 + WebView + `window.waywall` (docs/wallpaper-format.md#웹-배경).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use serde_json::{Map, Value, json};
use waywall_proto::config::LayerKind;
use waywall_proto::web::{Event, Show};
use webkit6::prelude::*;

use crate::send;

/// 페이지에 주입하는 `window.waywall`과 갱신 함수. `__WAYWALL__`은 초기값 JSON으로 바뀐다.
const API_SCRIPT: &str = r#"
(() => {
  const w = Object.assign(__WAYWALL__, {});
  window.waywall = w;
  const media = () => document.querySelectorAll("audio, video");
  // 나중에 재생을 시작하는 요소에도 볼륨·일시정지를 맞춘다 ('play'는 버블링하지 않아 캡처로 받음).
  document.addEventListener("play", (e) => {
    e.target.volume = w.volume;
    if (w.paused) { e.target.dataset.waywallPaused = "1"; e.target.pause(); }
  }, true);
  window.__waywall_apply = (kind, detail) => {
    if (kind === "pause") {
      w.paused = true;
      media().forEach((m) => { if (!m.paused) { m.dataset.waywallPaused = "1"; m.pause(); } });
    } else if (kind === "resume") {
      w.paused = false;
      media().forEach((m) => { if (m.dataset.waywallPaused) { delete m.dataset.waywallPaused; m.play(); } });
    } else if (kind === "volume") {
      w.volume = detail.volume;
      w.muted = detail.muted;
      media().forEach((m) => { m.volume = detail.volume; });
    } else if (kind === "properties") {
      Object.assign(w.properties, detail);
    }
    window.dispatchEvent(new CustomEvent("waywall-" + kind, { detail }));
  };
})();
"#;
const API_PLACEHOLDER: &str = "__WAYWALL__";

pub struct Page {
    show: Show,
    window: Option<gtk::Window>,
    view: Option<webkit6::WebView>,
    watch: Option<glib::SourceId>,
    frames: Rc<RefCell<FrameWatch>>,
}

/// frame 기아 판정: 주기마다 프레임 하나를 요청하고, 다음 주기까지 오지 않으면 가려진 것.
#[derive(Default)]
struct FrameWatch {
    requested: bool,
    ticked: bool,
    hidden: bool,
}

impl Page {
    pub fn new(show: Show) -> Self {
        let mut page = Self {
            show,
            window: None,
            view: None,
            watch: None,
            frames: Rc::default(),
        };
        if !page.try_open() {
            eprintln!(
                "waywall-web: {} not connected yet, waiting",
                page.show.output
            );
        }
        page
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.show.paused = paused;
        let kind = if paused { "pause" } else { "resume" };
        self.apply(kind, &Value::Null);
    }

    pub fn set_volume(&mut self, volume: f64, muted: bool) {
        self.show.volume = volume;
        self.show.muted = muted;
        if let Some(v) = &self.view {
            v.set_is_muted(muted);
        }
        self.apply("volume", &json!({ "volume": volume, "muted": muted }));
    }

    pub fn set_properties(&mut self, properties: Map<String, Value>) {
        self.show.properties.extend(properties.clone());
        self.apply("properties", &Value::Object(properties));
    }

    fn apply(&self, kind: &str, detail: &Value) {
        let Some(view) = &self.view else { return };
        let script =
            format!("window.__waywall_apply && window.__waywall_apply({kind:?}, {detail});");
        view.evaluate_javascript(&script, None, None, None::<&gio::Cancellable>, |r| {
            if let Err(e) = r {
                eprintln!("waywall-web: script failed: {e}");
            }
        });
    }

    /// 창이 없고 출력의 모니터가 있으면 창을 띄운다. 모니터가 아직 없으면 `false` (목록이 바뀔 때 다시 부른다).
    pub fn try_open(&mut self) -> bool {
        if self.window.is_some() {
            return true;
        }
        let Some(display) = gdk::Display::default() else {
            self.fail("no display");
            return false;
        };
        match find_monitor(&display, &self.show.output) {
            Some(monitor) => {
                self.open(&monitor);
                true
            }
            None => false,
        }
    }

    fn open(&mut self, monitor: &gdk::Monitor) {
        let s = &self.show;
        let window = gtk::Window::new();
        window.init_layer_shell();
        window.set_layer(match s.layer {
            LayerKind::Background => Layer::Background,
            LayerKind::Bottom => Layer::Bottom,
            LayerKind::Top => Layer::Top,
            LayerKind::Overlay => Layer::Overlay,
        });
        window.set_namespace(Some(&s.namespace));
        window.set_monitor(Some(monitor));
        for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
            window.set_anchor(edge, true);
        }
        window.set_exclusive_zone(-1);
        window.set_keyboard_mode(KeyboardMode::None);

        // 웹 프로세스 샌드박스에는 배경 디렉터리가 없어서, 웹 프로세스가 직접 여는 로컬 `<audio>`/`<video>`가
        // 실패한다(페이지·이미지·fetch는 네트워크 프로세스가 읽어서 됨). 웹 프로세스가 뜨기 전에 넣어야 하므로 페이지마다 컨텍스트.
        let context = webkit6::WebContext::new();
        if let Some(dir) = gio::File::for_uri(&s.url)
            .path()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        {
            context.add_path_to_sandbox(dir, true);
        }
        let view = webkit6::WebView::builder().web_context(&context).build();
        if let Some(settings) = webkit6::prelude::WebViewExt::settings(&view) {
            settings.set_allow_file_access_from_file_urls(true);
            settings.set_media_playback_requires_user_gesture(false);
            settings.set_enable_webaudio(true);
        }
        let c = s.background;
        view.set_background_color(&gdk::RGBA::new(
            f32::from(c.r) / 255.0,
            f32::from(c.g) / 255.0,
            f32::from(c.b) / 255.0,
            f32::from(c.a) / 255.0,
        ));
        view.set_is_muted(s.muted);
        if let Some(ucm) = view.user_content_manager() {
            let initial = json!({
                "version": env!("CARGO_PKG_VERSION"),
                "output": s.output,
                "properties": s.properties,
                "paused": s.paused,
                "volume": s.volume,
                "muted": s.muted,
            });
            let source = API_SCRIPT.replace(API_PLACEHOLDER, &initial.to_string());
            ucm.add_script(&webkit6::UserScript::new(
                &source,
                webkit6::UserContentInjectedFrames::TopFrame,
                webkit6::UserScriptInjectionTime::Start,
                &[],
                &[],
            ));
        }
        let output = s.output.clone();
        view.connect_load_changed(move |_, event| {
            if event == webkit6::LoadEvent::Finished {
                send(&Event::Shown {
                    output: output.clone(),
                });
            }
        });
        let output = s.output.clone();
        view.connect_load_failed(move |_, _, uri, err| {
            send(&Event::Error {
                output: output.clone(),
                message: format!("{uri}: {err}"),
            });
            false
        });
        if !s.interactive {
            // 입력 영역을 비워 컴포지터가 포인터 이벤트를 아예 보내지 않게 한다.
            view.set_can_target(false);
            window.connect_map(|w| {
                if let Some(surface) = w.surface() {
                    surface.set_input_region(Some(&gtk::cairo::Region::create()));
                }
            });
        }
        window.set_child(Some(&view));
        view.load_uri(&s.url);
        window.present();

        self.watch = Some(self.start_watch(&window));
        self.window = Some(window);
        self.view = Some(view);
    }

    fn start_watch(&self, window: &gtk::Window) -> glib::SourceId {
        let frames = self.frames.clone();
        let output = self.show.output.clone();
        let window = window.downgrade();
        let period = Duration::from_millis(self.show.frame_timeout_ms.max(1));
        glib::timeout_add_local(period, move || {
            let Some(window) = window.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let mut f = frames.borrow_mut();
            if f.requested {
                let hidden = !f.ticked;
                if hidden != f.hidden {
                    f.hidden = hidden;
                    send(&Event::Visibility {
                        output: output.clone(),
                        hidden,
                    });
                }
            }
            f.requested = true;
            f.ticked = false;
            drop(f);
            let frames = frames.clone();
            window.add_tick_callback(move |_, _| {
                frames.borrow_mut().ticked = true;
                glib::ControlFlow::Break
            });
            glib::ControlFlow::Continue
        })
    }

    fn fail(&self, message: &str) {
        send(&Event::Error {
            output: self.show.output.clone(),
            message: message.into(),
        });
    }
}

impl Drop for Page {
    fn drop(&mut self) {
        if let Some(id) = self.watch.take() {
            id.remove();
        }
        if let Some(w) = self.window.take() {
            w.destroy();
        }
    }
}

fn find_monitor(display: &gdk::Display, connector: &str) -> Option<gdk::Monitor> {
    let monitors = display.monitors();
    (0..monitors.n_items())
        .filter_map(|i| monitors.item(i)?.downcast::<gdk::Monitor>().ok())
        .find(|m| m.connector().is_some_and(|c| c == connector))
}
