//! dts-analyzer 命令行入口。

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

/// 分析多个 OS 的 DTS 文件并生成资源分配 Excel 报告。
#[derive(Parser)]
#[command(name = "dts-analyzer")]
#[command(about = "分析 DTS 文件并生成多 OS 资源分配报告", long_about = None)]
pub struct Cli {
    /// 配置文件路径 (TOML 格式)
    #[arg(short, long, default_value = "config.toml")]
    pub config: PathBuf,

    /// 输出 Excel 文件路径（覆盖配置文件中的设置）
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// 日志级别 (debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    pub log_level: String,

    /// 详细输出（等价于 --log-level debug）
    #[arg(short, long)]
    pub verbose: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let level = if cli.verbose { "debug" } else { &cli.log_level };
    let filter = match level.to_lowercase().as_str() {
        "debug" => log::LevelFilter::Debug,
        "warn" => log::LevelFilter::Warn,
        "error" => log::LevelFilter::Error,
        "trace" => log::LevelFilter::Trace,
        _ => log::LevelFilter::Info,
    };

    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}] {}",
                record.level(),
                message
            ))
        })
        .level(filter)
        .chain(std::io::stderr())
        .apply()?;

    if let Err(e) = dts_analyzer::run(&cli.config, cli.output.as_deref()) {
        log::error!("{:#}", e);
        std::process::exit(1);
    }
    Ok(())
}
