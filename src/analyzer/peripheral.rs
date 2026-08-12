//! 外设跨 OS 分析：构建全局外设分配矩阵，识别共享外设。

use indexmap::IndexMap;

use super::resource::{AnalysisReport, PeripheralRow, Presence, SharedKind, SharedResource};

/// 外设分析入口。
pub fn analyze_peripherals(report: &mut AnalysisReport) {
    let os_count = report.os_resources.len();

    // identity -> 行
    let mut rows: IndexMap<String, PeripheralRow> = IndexMap::new();

    for (i, os) in report.os_resources.iter().enumerate() {
        for p in &os.peripherals {
            let entry = rows.entry(p.identity()).or_insert_with(|| {
                let mut presence = vec![Presence::Absent; os_count];
                presence[i] = presence_for(p.is_enabled());
                PeripheralRow {
                    ptype: p.peripheral_type.clone(),
                    name: p.name.clone(),
                    base_addr: p.reg_ranges.first().map(|r| r.start),
                    irqs: p.interrupts.iter().map(|q| q.linux_irq()).collect(),
                    presence,
                    note: p.note.clone(),
                }
            });
            // 同名节点再次出现（同一 OS 内）不覆盖，仅更新状态
            entry.presence[i] = presence_for(p.is_enabled());
            if entry.note.is_empty() {
                entry.note = p.note.clone();
            }
        }
    }

    // 排序：先按类别，再按基地址 / 名称
    let mut all: Vec<PeripheralRow> = rows.into_values().collect();
    all.sort_by(|a, b| {
        a.ptype
            .cmp(&b.ptype)
            .then_with(|| a.base_addr.unwrap_or(u64::MAX).cmp(&b.base_addr.unwrap_or(u64::MAX)))
            .then_with(|| a.name.cmp(&b.name))
    });

    // 多个 OS 同时使能的外设 -> 共享资源
    for row in &all {
        if row.enabled_os_count() > 1 {
            let os_list: Vec<String> = row
                .presence
                .iter()
                .enumerate()
                .filter(|(_, p)| **p == Presence::Enabled)
                .map(|(i, _)| report.os_names[i].clone())
                .collect();
            let addr_note = row
                .base_addr
                .map(|a| format!("base=0x{:08X}", a))
                .unwrap_or_default();
            report.shared_resources.push(SharedResource {
                name: row.name.clone(),
                kind: SharedKind::Peripheral,
                range: row.base_addr.map(|a| crate::utils::address::AddressRange::new(a, 0)),
                os_list,
                details: format!("{} 外设被多个 OS 同时使能{}", row.ptype.as_str(),
                    if addr_note.is_empty() { String::new() } else { format!("; {}", addr_note) }),
            });
        }
    }

    report.peripheral_rows = all;
}

fn presence_for(enabled: bool) -> Presence {
    if enabled {
        Presence::Enabled
    } else {
        Presence::Disabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::resource::*;
    use crate::config::Config;
    use crate::utils::address::AddressRange;

    fn make_report() -> AnalysisReport {
        let cfg_toml = r#"
[output]
excel_file = "t.xlsx"
[[os]]
name = "A"
dts_file = "a.dts"
[[os]]
name = "B"
dts_file = "b.dts"
"#;
        let cfg: Config = toml::from_str(cfg_toml).unwrap();
        AnalysisReport::new(&cfg)
    }

    fn make_periph(name: &str, enabled: bool) -> Peripheral {
        Peripheral {
            name: name.into(),
            node_path: format!("/{}", name),
            peripheral_type: PeripheralType::Uart,
            compatible: vec![],
            reg_ranges: vec![AddressRange::new(0x270a_0000, 0x1000)],
            interrupts: vec![],
            status: if enabled { "okay".into() } else { "disabled".into() },
            note: String::new(),
        }
    }

    #[test]
    fn test_shared_peripheral_detection() {
        let mut report = make_report();
        let os_a = OsResources {
            os_name: "A".into(),
            dts_path: "a.dts".into(),
            cpus: vec![],
            memory_regions: vec![],
            reserved_regions: vec![],
            peripherals: vec![make_periph("serial@270a0000", true)],
            memreserves: vec![],
            total_memory: 0,
        };
        let mut os_b = os_a.clone();
        os_b.os_name = "B".into();
        report.os_resources.push(os_a);
        report.os_resources.push(os_b);

        analyze_peripherals(&mut report);
        assert_eq!(report.peripheral_rows.len(), 1);
        assert_eq!(report.peripheral_rows[0].enabled_os_count(), 2);
        assert!(report.shared_resources.iter().any(|s| s.name == "serial@270a0000"));
    }
}
