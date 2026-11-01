mod assets;
mod backend;
mod config;
mod cursor;
mod daemon;
mod fit;
mod wallpaper;
mod wayland;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use crate::config::Layers;

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

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Daemon { config } => {
            let layers = Layers::load(config.as_deref())?;
            init_logging(&layers.global()?.log.level);
            daemon::run(layers)
        }
        Command::Config {
            command: ConfigCommand::Defaults,
        } => {
            print!("{}", assets::defaults_toml());
            Ok(())
        }
        Command::Config {
            command: ConfigCommand::Show { config, output },
        } => {
            let layers = Layers::load(config.as_deref())?;
            match output {
                None => print!("{}", toml::to_string_pretty(&layers.merged_table()?)?),
                Some(name) => print!("{}", layers.describe_output(&name)?),
            }
            Ok(())
        }
    }
}

fn init_logging(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
