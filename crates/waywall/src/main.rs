mod assets;
mod backend;
mod config;
mod config_edit;
mod cursor;
mod daemon;
mod fit;
mod gpu;
mod ipc;
mod live2d;
mod properties;
mod runtime;
mod visibility;
mod wallpaper;
mod wayland;
mod web;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use clap::{ArgGroup, Parser, Subcommand};
use tracing_subscriber::EnvFilter;
use waywall_proto::ipc::{ListData, OutputStatus, Request};

use crate::config::Layers;
use crate::ipc::ClientError;

/// 데몬이 없을 때의 종료 코드 (docs/cli-ipc.md).
const EXIT_NOT_RUNNING: u8 = 3;

#[derive(Parser)]
#[command(version, about = "Live wallpapers for Wayland")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 배경 데몬 실행
    Daemon {
        /// 설정 파일 (기본: $XDG_CONFIG_HOME/waywall/config.toml)
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// 배경 바꾸기
    Set {
        /// 이 출력만 (생략 시 모든 출력)
        #[arg(short, long)]
        output: Option<String>,
        /// 설정 파일에도 기록
        #[arg(long)]
        save: bool,
        /// 파일/디렉터리 경로 또는 URL
        path: String,
    },
    /// 일시정지
    Pause {
        #[arg(short, long)]
        output: Option<String>,
    },
    /// pause로 멈춘 것 재개
    Resume {
        #[arg(short, long)]
        output: Option<String>,
    },
    /// 볼륨/음소거
    #[command(group(ArgGroup::new("change").required(true).multiple(true).args(["volume", "mute", "unmute"])))]
    Volume {
        #[arg(short, long)]
        output: Option<String>,
        #[arg(long)]
        save: bool,
        #[arg(value_parser = clap::value_parser!(u8).range(0..=100))]
        volume: Option<u8>,
        #[arg(long, conflicts_with = "unmute")]
        mute: bool,
        #[arg(long)]
        unmute: bool,
    },
    /// 출력과 배경 상태
    List {
        #[arg(long)]
        json: bool,
    },
    /// 설정 파일 다시 읽기
    Reload,
    /// 데몬 종료
    Quit,
    /// 설정 확인
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// 내장 defaults.toml 출력
    Defaults,
    /// 모든 계층을 합친 최종 설정 출력
    Show {
        #[arg(long)]
        config: Option<PathBuf>,
        /// 이 출력에 적용되는 값만 보기
        #[arg(short, long)]
        output: Option<String>,
    },
}

fn main() -> Result<ExitCode> {
    let request = match Cli::parse().command {
        Command::Daemon { config } => {
            let layers = Layers::load(config.as_deref())?;
            init_logging(&layers.global()?.log.level);
            daemon::run(config, layers)?;
            return Ok(ExitCode::SUCCESS);
        }
        Command::Config {
            command: ConfigCommand::Defaults,
        } => {
            print!("{}", assets::defaults_toml());
            return Ok(ExitCode::SUCCESS);
        }
        Command::Config {
            command: ConfigCommand::Show { config, output },
        } => {
            let layers = Layers::load(config.as_deref())?;
            match output {
                None => print!("{}", toml::to_string_pretty(&layers.merged_table()?)?),
                Some(name) => print!("{}", layers.describe_output(&name)?),
            }
            return Ok(ExitCode::SUCCESS);
        }
        Command::List { json } => return list(json),
        Command::Set { output, save, path } => Request::Set {
            output,
            path: absolute(&path),
            save,
        },
        Command::Pause { output } => Request::Pause { output },
        Command::Resume { output } => Request::Resume { output },
        Command::Volume {
            output,
            save,
            volume,
            mute,
            unmute,
        } => Request::Volume {
            output,
            volume,
            mute: (mute || unmute).then_some(mute),
            save,
        },
        Command::Reload => Request::Reload,
        Command::Quit => Request::Quit,
    };
    send(&request).map(|_| ExitCode::SUCCESS).or_else(exit_code)
}

/// 데몬의 작업 디렉터리와 무관하도록, 지금 위치에서 존재하는 경로는 절대 경로로 보낸다.
fn absolute(spec: &str) -> String {
    let p = Path::new(spec);
    match p.exists().then(|| std::path::absolute(p)) {
        Some(Ok(abs)) => abs.display().to_string(),
        _ => spec.to_owned(),
    }
}

/// 성공하면 응답의 `data`.
fn send(req: &Request) -> Result<Option<serde_json::Value>, ClientError> {
    let resp = ipc::request(req)?;
    if resp.ok {
        Ok(resp.data)
    } else {
        Err(ClientError::Other(anyhow::anyhow!(
            "{}",
            resp.error.unwrap_or_default()
        )))
    }
}

fn exit_code(e: ClientError) -> Result<ExitCode> {
    match e {
        ClientError::NotRunning(path, err) => {
            eprintln!("waywall: daemon is not running ({}: {err})", path.display());
            Ok(ExitCode::from(EXIT_NOT_RUNNING))
        }
        ClientError::Other(err) => Err(err),
    }
}

fn list(json: bool) -> Result<ExitCode> {
    let data = match send(&Request::List) {
        Ok(d) => d.unwrap_or_default(),
        Err(e) => return exit_code(e),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&data)?);
        return Ok(ExitCode::SUCCESS);
    }
    let data: ListData = serde_json::from_value(data)?;
    let rows: Vec<Vec<String>> = data.outputs.iter().map(row).collect();
    let widths: Vec<usize> = (0..rows.first().map_or(0, Vec::len))
        .map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0))
        .collect();
    for (row, out) in rows.iter().zip(&data.outputs) {
        let line: Vec<String> = row
            .iter()
            .zip(&widths)
            .map(|(cell, w)| format!("{cell:<w$}"))
            .collect();
        println!("{}", line.join("  ").trim_end());
        if let Some(err) = &out.error {
            println!("  error: {err}");
        }
    }
    Ok(ExitCode::SUCCESS)
}

const NONE: &str = "-";

fn row(o: &OutputStatus) -> Vec<String> {
    let reasons: Vec<String> = o
        .pause_reasons
        .iter()
        .map(|r| format!("{}:{}", r.reason, r.source))
        .collect();
    vec![
        o.name.clone(),
        format!("{}x{}@{}", o.width, o.height, o.scale),
        o.wallpaper.as_ref().map_or(NONE.into(), |w| w.name.clone()),
        o.wallpaper
            .as_ref()
            .and_then(|w| w.kind)
            .map_or(NONE.into(), |k| k.key().into()),
        o.state.to_string(),
        format!("vol {}{}", o.volume, if o.mute { " muted" } else { "" }),
        match (o.render_path, o.fps) {
            (Some(r), Some(fps)) => format!("{r} {fps:.1}fps"),
            (Some(r), None) => r.to_string(),
            (None, _) => NONE.into(),
        },
        if reasons.is_empty() {
            String::new()
        } else {
            format!("({})", reasons.join(", "))
        },
    ]
}

fn init_logging(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
