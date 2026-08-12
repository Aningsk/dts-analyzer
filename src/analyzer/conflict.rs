//! 冲突检测：
//! - CPU 核心被多个 OS 重复分配
//! - 同一外设被多个 OS 同时使能（可能为透传设计，需人工确认）
//! - 同一中断号被不同 OS 的不同外设使用

use std::collections::BTreeMap;

use super::resource::{AnalysisReport, Conflict, ConflictType, Presence};

/// 冲突检测入口。
pub fn detect_conflicts(report: &mut AnalysisReport) {
    detect_cpu_conflicts(report);
    detect_peripheral_conflicts(report);
    detect_interrupt_conflicts(report);
}

/// 同一 MPIDR 核心出现在多个 OS 中。
fn detect_cpu_conflicts(report: &mut AnalysisReport) {
    for (mpidr, owners) in &report.cpu_matrix {
        if owners.len() > 1 {
            report.conflicts.push(Conflict {
                conflict_type: ConflictType::CpuConflict,
                resource_name: format!("cpu@{:x}", mpidr),
                os_list: owners.clone(),
                description: format!(
                    "CPU 核心 (MPIDR=0x{:X}) 被 {} 个 OS 同时声明",
                    mpidr,
                    owners.len()
                ),
                suggestion:
                    "确认 Hypervisor 的 CPU 分配策略；若为硬隔离方案，同一核心只应归属一个 OS，\
                     请修正多余 OS 的 DTS cpus 节点"
                        .to_string(),
            });
        }
    }
}

/// 同一外设被多个 OS 同时使能。
fn detect_peripheral_conflicts(report: &mut AnalysisReport) {
    for row in &report.peripheral_rows {
        if row.enabled_os_count() <= 1 {
            continue;
        }
        // 中断控制器、IOMMU 等系统级组件多 OS 声明属常见设计，仅提示
        let os_list: Vec<String> = row
            .presence
            .iter()
            .enumerate()
            .filter(|(_, p)| **p == Presence::Enabled)
            .map(|(i, _)| report.os_names[i].clone())
            .collect();
        let is_system_component = matches!(
            row.ptype,
            super::resource::PeripheralType::InterruptController
                | super::resource::PeripheralType::Iommu
        );
        report.conflicts.push(Conflict {
            conflict_type: ConflictType::PeripheralConflict,
            resource_name: row.name.clone(),
            os_list,
            description: format!(
                "{} 外设被 {} 个 OS 同时使能{}",
                row.ptype.as_str(),
                row.enabled_os_count(),
                if is_system_component { "（系统级组件，可能为预期设计）" } else { "" }
            ),
            suggestion: if is_system_component {
                "系统级组件（GIC/IOMMU）通常由 Hypervisor 统一管理，确认各 OS 访问权限配置即可"
                    .to_string()
            } else {
                "若该外设非共享/透传设计，应将非归属 OS 的 DTS 中该节点 status 设为 disabled，\
                 或在 Hypervisor 层做设备分配隔离"
                    .to_string()
            },
        });
    }
}

/// 同一中断号被不同 OS 的不同外设使用。
fn detect_interrupt_conflicts(report: &mut AnalysisReport) {
    // irq -> Vec<(os, peripheral)>
    let mut irq_users: BTreeMap<u32, Vec<(String, String)>> = BTreeMap::new();
    for os in &report.os_resources {
        for p in &os.peripherals {
            if !p.is_enabled() {
                continue;
            }
            for irq in &p.interrupts {
                let num = irq.linux_irq();
                let entry = irq_users.entry(num).or_default();
                let item = (os.os_name.clone(), p.name.clone());
                if !entry.contains(&item) {
                    entry.push(item);
                }
            }
        }
    }
    for (irq, users) in &irq_users {
        // 同一 OS 内不同外设用同一中断是常见的（级联/复用），仅关注跨 OS
        let os_set: Vec<String> = {
            let mut v: Vec<String> = users.iter().map(|(o, _)| o.clone()).collect();
            v.sort();
            v.dedup();
            v
        };
        if os_set.len() > 1 {
            let detail: Vec<String> = users.iter().map(|(o, p)| format!("{}: {}", o, p)).collect();
            report.conflicts.push(Conflict {
                conflict_type: ConflictType::InterruptConflict,
                resource_name: format!("IRQ {}", irq),
                os_list: os_set,
                description: format!("中断 {} 被多个 OS 的外设使用: {}", irq, detail.join("; ")),
                suggestion: "确认 Hypervisor 中断路由配置；若中断不支持共享，\
                            需调整外设分配或使用中断转发机制"
                    .to_string(),
            });
        }
    }
}
