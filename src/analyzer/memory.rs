//! 内存与 CPU 跨 OS 分析：
//! - 全局 CPU 分配矩阵
//! - 系统内存分配矩阵与重叠检测
//! - 保留内存 / 子系统共享内存（shared-memory、gipc）识别

use std::collections::BTreeMap;

use crate::utils::address::{merge_ranges, AddressRange};

use super::resource::{
    AnalysisReport, MemoryRegion, MemoryType, SharedKind, SharedMemoryRow, SharedResource,
};

/// 执行内存维度的跨 OS 分析。
pub fn analyze_memory(report: &mut AnalysisReport) {
    build_cpu_matrix(report);
    build_memory_matrix(report);
    collect_reserved_shared(report);
    collect_subsystem_shared_memory(report);
    // 共享内存明细按地址排序，便于阅读
    report.shared_memory_rows.sort_by_key(|r| r.range.start);
}

/// 全局 CPU 矩阵：按 MPIDR 聚合各 OS 的核心。
fn build_cpu_matrix(report: &mut AnalysisReport) {
    for os in &report.os_resources {
        for cpu in &os.cpus {
            report.cpu_matrix.entry(cpu.mpidr).or_default().push(os.os_name.clone());
        }
    }
}

/// 系统内存矩阵：逐 OS 列出 memory 节点区间，并标注重叠。
fn build_memory_matrix(report: &mut AnalysisReport) {
    // 先收集全部区间用于重叠检测
    let all: Vec<(usize, AddressRange)> = report
        .os_resources
        .iter()
        .enumerate()
        .flat_map(|(i, os)| os.memory_regions.iter().map(move |r| (i, r.range)))
        .collect();

    for (i, os) in report.os_resources.iter().enumerate() {
        for region in &os.memory_regions {
            let mut overlap_with = Vec::new();
            for (j, other_range) in &all {
                if *j == i {
                    continue;
                }
                if region.range.overlaps(other_range) {
                    let name = &report.os_resources[*j].os_name;
                    if !overlap_with.contains(name) {
                        overlap_with.push(name.clone());
                    }
                }
            }
            let note = if overlap_with.is_empty() {
                String::new()
            } else {
                let note = format!("与 {} 重叠（repeated）", overlap_with.join(", "));
                report.shared_resources.push(SharedResource {
                    name: format!(
                        "{} {}",
                        AddressRange::fmt_addr(region.range.start),
                        AddressRange::fmt_size(region.range.size)
                    ),
                    kind: SharedKind::MemoryOverlap,
                    range: Some(region.range),
                    os_list: {
                        let mut l = vec![os.os_name.clone()];
                        for n in &overlap_with {
                            if !l.contains(n) {
                                l.push(n.clone());
                            }
                        }
                        l
                    },
                    details: format!("系统内存区间在多个 OS 中重复声明: {}", region.node_path),
                });
                note
            };
            report.memory_matrix.push(super::resource::MemoryMatrixRow {
                os_name: os.os_name.clone(),
                node_path: region.node_path.clone(),
                range: region.range,
                note,
            });
        }
    }
    report.memory_matrix.sort_by_key(|r| r.range.start);
}

/// 保留内存共享分析：同一区域（名称+地址）出现在多个 OS 的 DTS 中即视为共享。
fn collect_reserved_shared(report: &mut AnalysisReport) {
    // key: (名称, start, size) -> (owners, 样本区域)
    let mut groups: BTreeMap<(String, u64, u64), (Vec<String>, MemoryRegion)> = BTreeMap::new();
    for os in &report.os_resources {
        for region in &os.reserved_regions {
            if region.attributes.iter().any(|a| a == "status=disabled") {
                continue;
            }
            let key = (region.name.clone(), region.range.start, region.range.size);
            match groups.get_mut(&key) {
                Some((owners, _)) => {
                    if !owners.contains(&os.os_name) {
                        owners.push(os.os_name.clone());
                    }
                }
                None => {
                    groups.insert(key, (vec![os.os_name.clone()], region.clone()));
                }
            }
        }
    }

    for ((name, _, _), (owners, sample)) in groups {
        let is_gipc = name.to_lowercase().starts_with("gipc_");
        let shared_by_name = sample.region_type == MemoryType::Shared;
        if owners.len() > 1 || is_gipc || shared_by_name {
            let mut details = Vec::new();
            if !sample.compatible.is_empty() {
                details.push(format!("compatible={}", sample.compatible.join(",")));
            }
            if !sample.attributes.is_empty() {
                details.push(sample.attributes.join(", "));
            }
            if is_gipc {
                if let Some(peers) = parse_gipc_peers(&name, report) {
                    details.push(format!("IPC 对端: {}", peers));
                }
            }
            let kind = if is_gipc { SharedKind::GipcIpc } else { SharedKind::ReservedMemory };
            report.shared_resources.push(SharedResource {
                name: name.clone(),
                kind,
                range: Some(sample.range),
                os_list: owners.clone(),
                details: details.join("; "),
            });
            if owners.len() > 1 || is_gipc {
                report.shared_memory_rows.push(SharedMemoryRow {
                    name,
                    source: sample.node_path.clone(),
                    range: sample.range,
                    os_list: owners,
                    description: details.join("; "),
                });
            }
        }
    }
}

/// 解析 `gipc_<a>_<b>_<idx>` 命名的对端信息。
fn parse_gipc_peers(name: &str, report: &AnalysisReport) -> Option<String> {
    let lower = name.to_lowercase();
    let stripped = lower.strip_prefix("gipc_")?;
    let parts: Vec<&str> = stripped.split('_').collect();
    if parts.len() < 2 {
        return None;
    }
    let mut peers = Vec::new();
    for token in &parts[..2] {
        let matched = report.os_names.iter().find(|os| {
            let os_lower = os.to_lowercase();
            os_lower == *token || os_lower.starts_with(token) || token.starts_with(&os_lower)
        });
        peers.push(matched.cloned().unwrap_or_else(|| token.to_string()));
    }
    Some(peers.join(" <-> "))
}

/// 子系统共享内存分组条目：(拥有 OS 列表, 全部子区段, 来源路径)。
type SubsystemGroup = (Vec<String>, Vec<AddressRange>, String);

/// 子系统 `shared-memory` 节点分析（如 sdd/ADSP 音频共享内存）。
///
/// 规则：同一（父节点身份, 区域名）的区段在多个 OS 的 DTS 中出现时，
/// 视为这些 OS 共享；区段（suffix-names 的多 core 切片）会合并为连续区间。
fn collect_subsystem_shared_memory(report: &mut AnalysisReport) {
    // key: (父节点名, 区域名) -> (owners, 全部子区段, 来源路径)
    let mut groups: BTreeMap<(String, String), SubsystemGroup> = BTreeMap::new();

    for (idx, os) in report.os_resources.iter().enumerate() {
        let Some(dts) = report.dts_files.get(idx) else { continue };
        let mut found: Vec<(String, String, Vec<AddressRange>, String)> = Vec::new();
        dts.root.walk(&mut |node| {
            if node.name != "shared-memory" {
                return;
            }
            // 父节点身份：上一级节点名（如 sdd@33000000）
            let parent_name = node
                .path
                .rsplit('/')
                .nth(1)
                .unwrap_or("unknown")
                .to_string();
            // 仅当父节点使能时参与共享判定
            let parent_enabled = dts
                .root
                .find_path(node.path.rsplit_once('/').map(|(p, _)| p).unwrap_or("/"))
                .map(|p| p.is_enabled())
                .unwrap_or(true);
            if !parent_enabled {
                return;
            }
            let addr_cells = node.get_property("#address-cells").and_then(|p| p.first_cell()).unwrap_or(1) as usize;
            let size_cells = node.get_property("#size-cells").and_then(|p| p.first_cell()).unwrap_or(1) as usize;
            for child in node.children.values() {
                let Some(cells) = child.get_property("reg").and_then(|p| p.as_cells()) else {
                    continue;
                };
                let ranges = crate::utils::address::cells_to_ranges(&cells, addr_cells, size_cells);
                if !ranges.is_empty() {
                    found.push((parent_name.clone(), child.name.clone(), ranges, node.path.clone()));
                }
            }
        });
        for (parent_name, region_name, ranges, path) in found {
            let key = (parent_name, region_name);
            match groups.get_mut(&key) {
                Some((owners, all, _)) => {
                    if !owners.contains(&os.os_name) {
                        owners.push(os.os_name.clone());
                    }
                    all.extend(ranges);
                }
                None => {
                    groups.insert(key, (vec![os.os_name.clone()], ranges, path));
                }
            }
        }
    }

    for ((parent, name), (owners, ranges, source)) in groups {
        if owners.len() < 2 {
            continue;
        }
        let merged = merge_ranges(ranges);
        for range in merged {
            let description = format!("{}/shared-memory", parent);
            report.shared_resources.push(SharedResource {
                name: name.clone(),
                kind: SharedKind::SubsystemSharedMemory,
                range: Some(range),
                os_list: owners.clone(),
                details: description.clone(),
            });
            report.shared_memory_rows.push(SharedMemoryRow {
                name: name.clone(),
                source: source.clone(),
                range,
                os_list: owners.clone(),
                description,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gipc_peers() {
        let report = AnalysisReport {
            title: String::new(),
            os_names: vec!["IVI".into(), "Cluster".into(), "ADAS".into()],
            os_resources: Vec::new(),
            dts_files: Vec::new(),
            cpu_matrix: BTreeMap::new(),
            memory_matrix: Vec::new(),
            peripheral_rows: Vec::new(),
            shared_resources: Vec::new(),
            shared_memory_rows: Vec::new(),
            conflicts: Vec::new(),
        };
        assert_eq!(
            parse_gipc_peers("gipc_ivi_cluster_0", &report),
            Some("IVI <-> Cluster".into())
        );
        assert_eq!(
            parse_gipc_peers("gipc_cluster_scp_1", &report),
            Some("Cluster <-> scp".into())
        );
    }
}
