//! 资源分析模块。

pub mod conflict;
pub mod memory;
pub mod peripheral;
pub mod resource;

use anyhow::Result;

use crate::config::Config;
use resource::AnalysisReport;

/// 分析入口：解析全部 OS 的 DTS 并执行跨 OS 资源分析。
pub fn analyze(config: &Config) -> Result<AnalysisReport> {
    let mut report = AnalysisReport::new(config);

    // Phase 1: 逐个 OS 提取资源（文件间相互独立，可并行；此处顺序执行已足够快）
    for os_cfg in &config.os {
        log::info!("解析 OS [{}] 的 DTS: {}", os_cfg.name, os_cfg.dts_file.display());
        let dts = crate::dts::parse_dts_file(&os_cfg.dts_file)?;
        let resources = resource::extract_os_resources(os_cfg, &dts, &config.rules);
        log::info!(
            "  -> {} 个 CPU, {} 段系统内存, {} 段保留内存, {} 个外设",
            resources.cpus.len(),
            resources.memory_regions.len(),
            resources.reserved_regions.len(),
            resources.peripherals.len()
        );
        report.os_resources.push(resources);
        report.dts_files.push(dts);
    }

    // Phase 2: 跨 OS 分析
    memory::analyze_memory(&mut report);
    peripheral::analyze_peripherals(&mut report);
    conflict::detect_conflicts(&mut report);

    log::info!(
        "分析完成: 共享资源 {} 项, 冲突 {} 项",
        report.shared_resources.len(),
        report.conflicts.len()
    );
    Ok(report)
}
