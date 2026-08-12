//! dts-analyzer：多 OS Device Tree 资源分配分析工具。
//!
//! 解析多个 OS 的 DTS 文件，提取 CPU / 内存 / 外设 / 中断等资源，
//! 识别 OS 之间的共享与冲突，并生成 Excel 报告。

pub mod analyzer;
pub mod config;
pub mod dts;
pub mod export;
pub mod utils;

use std::path::Path;

use anyhow::Result;

use config::Config;

/// 业务核心入口：加载配置 -> 分析 -> 导出 Excel。
pub fn run(config_path: &Path, output_override: Option<&Path>) -> Result<()> {
    log::info!("加载配置文件: {}", config_path.display());
    let mut config = Config::load(config_path)?;
    if let Some(out) = output_override {
        config.output.excel_file = out.to_path_buf();
    }

    log::info!(
        "共 {} 个 OS 待分析: {}",
        config.os.len(),
        config.os.iter().map(|o| o.name.as_str()).collect::<Vec<_>>().join(", ")
    );

    let report = analyzer::analyze(&config)?;
    export::export_report(&report, &config.output.excel_file)?;

    println!();
    println!("分析完成:");
    println!("  OS 数量:       {}", report.os_names.len());
    println!("  CPU 核心总数:  {}", report.cpu_matrix.len());
    println!("  共享资源条目:  {}", report.shared_resources.len());
    println!("  共享内存条目:  {}", report.shared_memory_rows.len());
    println!("  冲突条目:      {}", report.conflicts.len());
    println!("  Excel 报告:    {}", config.output.excel_file.display());
    Ok(())
}
