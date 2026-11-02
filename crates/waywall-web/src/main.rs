//! waywall 웹 배경 헬퍼: 데몬이 stdin으로 보내는 명령에 따라 출력마다 layer 창 + WebView를 띄운다
//! (docs/architecture.md#웹-헬퍼, 프로토콜은 docs/cli-ipc.md#웹-헬퍼-프로토콜).
//!
//! 데몬은 GTK를 링크하지 않으므로 GTK/WebKit은 이 프로세스에만 있다. 로그는 stderr.

mod page;

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use waywall_proto::web::{Command, Event};

use page::Page;

fn main() -> glib::ExitCode {
    if let Err(e) = gtk::init() {
        eprintln!("waywall-web: cannot initialize GTK: {e}");
        return glib::ExitCode::FAILURE;
    }
    let main_loop = glib::MainLoop::new(None, false);
    let (tx, rx) = async_channel::unbounded::<Option<Command>>();
    // stdin은 막히는 읽기라 스레드에서 읽어 메인 루프로 넘긴다. EOF(데몬 종료)는 None.
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            match serde_json::from_str::<Command>(&line) {
                Ok(cmd) => {
                    if tx.send_blocking(Some(cmd)).is_err() {
                        return;
                    }
                }
                Err(e) => eprintln!("waywall-web: bad command `{line}`: {e}"),
            }
        }
        let _ = tx.send_blocking(None);
    });

    let pages: Rc<RefCell<HashMap<String, Page>>> = Rc::default();
    // 연결 직후라 아직 모니터가 없던 출력의 페이지를 모니터가 나타나면 띄운다.
    if let Some(display) = gtk::gdk::Display::default() {
        let pages = pages.clone();
        display.monitors().connect_items_changed(move |_, _, _, _| {
            for page in pages.borrow_mut().values_mut() {
                page.try_open();
            }
        });
    }
    let ml = main_loop.clone();
    glib::spawn_future_local(async move {
        while let Ok(Some(cmd)) = rx.recv().await {
            if !handle(&pages, cmd) {
                break;
            }
        }
        pages.borrow_mut().clear();
        ml.quit();
    });
    send(&Event::Ready);
    main_loop.run();
    glib::ExitCode::SUCCESS
}

/// 명령 하나를 처리한다. 끝내야 하면 `false`.
fn handle(pages: &Rc<RefCell<HashMap<String, Page>>>, cmd: Command) -> bool {
    match cmd {
        Command::Show(show) => {
            let output = show.output.clone();
            // 같은 출력의 이전 페이지는 바꾼다.
            pages.borrow_mut().remove(&output);
            let page = Page::new(*show);
            pages.borrow_mut().insert(output, page);
        }
        Command::Hide { output } => {
            pages.borrow_mut().remove(&output);
            send(&Event::Hidden { output });
        }
        Command::Pause { output } => with(pages, &output, |p| p.set_paused(true)),
        Command::Resume { output } => with(pages, &output, |p| p.set_paused(false)),
        Command::Volume {
            output,
            volume,
            muted,
        } => with(pages, &output, |p| p.set_volume(volume, muted)),
        Command::Properties { output, properties } => {
            with(pages, &output, |p| p.set_properties(properties));
        }
        Command::Quit => return false,
    }
    true
}

fn with(pages: &Rc<RefCell<HashMap<String, Page>>>, output: &str, f: impl FnOnce(&mut Page)) {
    match pages.borrow_mut().get_mut(output) {
        Some(p) => f(p),
        None => eprintln!("waywall-web: no page on {output}"),
    }
}

/// 데몬에 이벤트 한 줄. stdout이 닫혔으면(데몬 종료) 무시한다 — 곧 stdin EOF로 끝난다.
pub fn send(event: &Event) {
    let Ok(line) = serde_json::to_string(event) else {
        return;
    };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}").and_then(|()| out.flush());
}
