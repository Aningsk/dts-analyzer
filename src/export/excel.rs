//! Excel 报告生成器：将 [`AnalysisReport`] 导出为 6 个工作表的 xlsx 文件。
//!
//! 基于 `rust_xlsxwriter` 实现。
//!
//! Sheet 结构：
//! 1. 总览 (Overview)
//! 2. 资源分配矩阵 (Resource Allocation Matrix)
//! 3. 内存分配矩阵 (Memory Allocation Matrix)
//! 4. 共享资源清单 (Shared Resources)
//! 5. 外设分配 (Peripheral Allocation)
//! 6. 冲突报告 (Conflict Report)

use std::path::Path;

use anyhow::Result;
use rust_xlsxwriter::{Color, Format, FormatAlign, FormatBorder, Workbook, Worksheet};

use crate::analyzer::resource::{
    AnalysisReport, ConflictType, MemoryType, PeripheralType, Presence, SharedKind,
};
use crate::utils::address::{merge_ranges, AddressRange};

// 常用颜色
const COLOR_HEADER_BG: u32 = 0x4472C4;
const COLOR_CATEGORY_BG: u32 = 0xD9E1F2;
const COLOR_SHARED_BG: u32 = 0xFFF2CC;
const COLOR_CONFLICT_BG: u32 = 0xFFC7CE;
const COLOR_OK_BG: u32 = 0xE2EFDA;
const COLOR_TITLE_FONT: u32 = 0x1F4E79;
const COLOR_CONFLICT_FONT: u32 = 0x9C0006;
const COLOR_GRAY_FONT: u32 = 0x595959;

/// 报表样式集合。
struct Styles {
    title: Format,
    subtitle: Format,
    header: Format,
    category: Format,
    normal: Format,
    center: Format,
    check: Format,
    shared: Format,
    shared_bold: Format,
    conflict: Format,
    ok: Format,
    gray: Format,
}

impl Styles {
    fn new() -> Self {
        Self {
            title: Format::new()
                .set_bold()
                .set_font_size(16)
                .set_font_color(Color::RGB(COLOR_TITLE_FONT)),
            subtitle: Format::new().set_font_color(Color::RGB(COLOR_GRAY_FONT)),
            header: Format::new()
                .set_bold()
                .set_font_color(Color::White)
                .set_background_color(Color::RGB(COLOR_HEADER_BG))
                .set_align(FormatAlign::Center)
                .set_border(FormatBorder::Thin),
            category: Format::new()
                .set_bold()
                .set_background_color(Color::RGB(COLOR_CATEGORY_BG))
                .set_border(FormatBorder::Thin),
            normal: Format::new().set_border(FormatBorder::Thin),
            center: Format::new()
                .set_border(FormatBorder::Thin)
                .set_align(FormatAlign::Center),
            check: Format::new()
                .set_border(FormatBorder::Thin)
                .set_align(FormatAlign::Center)
                .set_bold(),
            shared: Format::new()
                .set_background_color(Color::RGB(COLOR_SHARED_BG))
                .set_border(FormatBorder::Thin),
            shared_bold: Format::new()
                .set_background_color(Color::RGB(COLOR_SHARED_BG))
                .set_border(FormatBorder::Thin)
                .set_bold(),
            conflict: Format::new()
                .set_background_color(Color::RGB(COLOR_CONFLICT_BG))
                .set_border(FormatBorder::Thin)
                .set_font_color(Color::RGB(COLOR_CONFLICT_FONT)),
            ok: Format::new()
                .set_background_color(Color::RGB(COLOR_OK_BG))
                .set_border(FormatBorder::Thin),
            gray: Format::new()
                .set_font_color(Color::RGB(COLOR_GRAY_FONT))
                .set_border(FormatBorder::Thin),
        }
    }
}

// ---------------------------------------------------------------------------
// 自适应列宽：写单元格时记录各列内容，sheet 写完后按最长内容计算列宽。
// ---------------------------------------------------------------------------

/// 列宽下限（字符数）。
const MIN_COL_WIDTH: f64 = 10.0;
/// 列宽上限：超长文本（如冲突描述/建议）截断到此宽度，避免单列过宽。
const MAX_COL_WIDTH: f64 = 100.0;

thread_local! {
    /// 当前 sheet 的单元格内容记录：(列号, 文本)。
    static COL_RECORDS: std::cell::RefCell<Vec<(u16, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// 开始记录一个 sheet 的列内容（写表体前调用）。
fn autowidth_begin() {
    COL_RECORDS.with(|r| r.borrow_mut().clear());
}

/// 按记录的最长内容设置各列宽度（sheet 写完后调用）。
fn autowidth_apply(ws: &mut Worksheet) -> Result<()> {
    let widths: Vec<(u16, f64)> = COL_RECORDS.with(|r| {
        let records = r.borrow();
        let mut max: std::collections::BTreeMap<u16, f64> = std::collections::BTreeMap::new();
        for (col, text) in records.iter() {
            let w = display_width(text) + 2.0; // 余量：光标/边框
            let e = max.entry(*col).or_insert(0.0);
            if w > *e {
                *e = w;
            }
        }
        max.into_iter()
            .map(|(c, w)| (c, w.clamp(MIN_COL_WIDTH, MAX_COL_WIDTH)))
            .collect()
    });
    for (col, w) in widths {
        ws.set_column_width(col, w)?;
    }
    Ok(())
}

/// 估算文本显示宽度：CJK 等宽字符按 2 计，其余按 1。
fn display_width(text: &str) -> f64 {
    text.chars()
        .map(|c| {
            if (c as u32) < 0x2E80 { 1.0 } else { 2.0 }
        })
        .sum()
}

/// 写带格式的字符串（格式按引用传入，内部克隆）。
fn ws_str(ws: &mut Worksheet, row: u32, col: u16, text: &str, fmt: &Format) {
    let _ = ws.write_string_with_format(row, col, text, fmt);
    COL_RECORDS.with(|r| r.borrow_mut().push((col, text.to_string())));
}

/// 写带格式的数字。
fn ws_num(ws: &mut Worksheet, row: u32, col: u16, value: f64, fmt: &Format) {
    let _ = ws.write_number_with_format(row, col, value, fmt);
    let text = if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        value.to_string()
    };
    COL_RECORDS.with(|r| r.borrow_mut().push((col, text)));
}

/// 写带格式的空单元格（用于保持边框样式）。
fn ws_blank(ws: &mut Worksheet, row: u32, col: u16, fmt: &Format) {
    let _ = ws.write_blank(row, col, fmt);
}

/// 导出完整报告。
pub fn export_report(report: &AnalysisReport, path: &Path) -> Result<()> {
    let mut wb = Workbook::new();
    let st = Styles::new();

    write_overview(&mut wb, report, &st)?;
    write_resource_matrix(&mut wb, report, &st)?;
    write_memory_matrix(&mut wb, report, &st)?;
    write_shared_resources(&mut wb, report, &st)?;
    write_peripheral_allocation(&mut wb, report, &st)?;
    write_conflicts(&mut wb, report, &st)?;

    wb.save(path)?;
    log::info!("Excel 报告已生成: {}", path.display());
    Ok(())
}

/// 新建一个命名工作表。
fn add_sheet<'a>(wb: &'a mut Workbook, name: &str) -> Result<&'a mut Worksheet> {
    let ws = wb.add_worksheet();
    ws.set_name(name)?;
    Ok(ws)
}

/// 写入表头行。
fn write_header_row(ws: &mut Worksheet, row: u32, headers: &[&str], st: &Styles) {
    for (c, h) in headers.iter().enumerate() {
        ws_str(ws, row, c as u16, h, &st.header);
    }
}

// ---------------------------------------------------------------------------
// Sheet 1: 总览
// ---------------------------------------------------------------------------

fn write_overview(wb: &mut Workbook, report: &AnalysisReport, st: &Styles) -> Result<()> {
    let ws = add_sheet(wb, "总览 (Overview)")?;
    autowidth_begin();

    ws.merge_range(0, 0, 0, (1 + report.os_names.len().max(3)) as u16, &report.title, &st.title)?;
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    ws_str(ws, 1, 0, &format!("生成时间: {}", now), &st.subtitle);

    // OS 概览表
    let mut row = 3;
    ws_str(ws, row, 0, "OS 与 DTS 文件", &st.header);
    ws_str(ws, row, 1, "", &st.header);
    row += 1;
    write_header_row(ws, row, &["OS", "DTS 文件", "CPU 数", "系统内存", "保留区数", "外设数"], st);
    row += 1;
    for os in &report.os_resources {
        ws_str(ws, row, 0, &os.os_name, &st.normal);
        ws_str(ws, row, 1, &os.dts_path, &st.normal);
        ws_num(ws, row, 2, os.cpus.len() as f64, &st.center);
        ws_str(ws, row, 3, &AddressRange::fmt_size(os.total_memory), &st.normal);
        ws_num(ws, row, 4, os.reserved_regions.len() as f64, &st.center);
        ws_num(ws, row, 5, os.peripherals.len() as f64, &st.center);
        row += 1;
    }

    // 统计摘要
    row += 1;
    ws_str(ws, row, 0, "统计摘要", &st.header);
    ws_str(ws, row, 1, "", &st.header);
    row += 1;
    let total_cpus = report.cpu_matrix.len();
    // 各 OS 声明量简单求和（重叠区域会被重复计入，仅作参考）
    let total_memory: u64 = report.os_resources.iter().map(|o| o.total_memory).sum();
    // 全部系统内存区间去重合并后的物理占用
    let union_memory: u64 = merge_ranges(
        report
            .os_resources
            .iter()
            .flat_map(|o| o.memory_regions.iter())
            .map(|m| m.range)
            .collect(),
    )
    .iter()
    .map(|r| r.size)
    .sum();
    // 共享内存条目以「内存分配矩阵」中的共享行（shared_memory_rows）为准
    let shared_mem_count = report.shared_memory_rows.len();
    let shared_mem_size: u64 = report
        .shared_memory_rows
        .iter()
        .map(|r| r.range.size)
        .sum();
    let conflict_fmt = if report.conflicts.is_empty() { &st.ok } else { &st.conflict };
    let stats: [(&str, String, &Format); 7] = [
        ("OS 数量", report.os_names.join(", "), &st.normal),
        ("物理 CPU 核心总数（去重）", total_cpus.to_string(), &st.center),
        ("系统内存物理占用（去重）", AddressRange::fmt_size(union_memory), &st.normal),
        ("各 OS 系统内存声明合计（含重叠）", AddressRange::fmt_size(total_memory), &st.normal),
        ("共享资源条目数", report.shared_resources.len().to_string(), &st.center),
        ("共享内存条目数 / 合计大小",
            format!("{} 项 / {}", shared_mem_count, AddressRange::fmt_size(shared_mem_size)),
            &st.shared_bold),
        ("冲突条目数", report.conflicts.len().to_string(), conflict_fmt),
    ];
    for (label, value, fmt) in stats {
        ws_str(ws, row, 0, label, &st.category);
        ws_str(ws, row, 1, &value, fmt);
        row += 1;
    }
    autowidth_apply(ws)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Sheet 2: 资源分配矩阵
// ---------------------------------------------------------------------------

/// 外设类别在矩阵中的展示顺序。
const CATEGORY_ORDER: &[PeripheralType] = &[
    PeripheralType::InterruptController,
    PeripheralType::Gpu,
    PeripheralType::Display,
    PeripheralType::Camera,
    PeripheralType::Video,
    PeripheralType::Audio,
    PeripheralType::Storage,
    PeripheralType::Ethernet,
    PeripheralType::Wireless,
    PeripheralType::Can,
    PeripheralType::Usb,
    PeripheralType::Pcie,
    PeripheralType::Uart,
    PeripheralType::I2c,
    PeripheralType::Spi,
    PeripheralType::Gpio,
    PeripheralType::Pwm,
    PeripheralType::Watchdog,
    PeripheralType::Iommu,
    PeripheralType::Crypto,
    PeripheralType::Other,
];

fn write_resource_matrix(wb: &mut Workbook, report: &AnalysisReport, st: &Styles) -> Result<()> {
    let ws = add_sheet(wb, "资源分配矩阵")?;
    let os_count = report.os_names.len();
    autowidth_begin();
    ws.set_freeze_panes(1, 0)?;

    let mut headers: Vec<&str> = vec!["Resources", ""];
    let os_refs: Vec<&str> = report.os_names.iter().map(|s| s.as_str()).collect();
    headers.extend_from_slice(&os_refs);
    headers.push("备注");
    write_header_row(ws, 0, &headers, st);

    let mut row = 1u32;

    // --- CPU 行（全局编号，按 MPIDR 排序）---
    let cpu_start = row;
    for (idx, (mpidr, owners)) in report.cpu_matrix.iter().enumerate() {
        ws_str(ws, row, 1, &format!("CPU-{}", idx), &st.normal);
        for (i, os_name) in report.os_names.iter().enumerate() {
            if owners.contains(os_name) {
                ws_str(ws, row, 2 + i as u16, "√", &st.check);
            } else {
                ws_blank(ws, row, 2 + i as u16, &st.normal);
            }
        }
        ws_str(ws, row, 2 + os_count as u16, &format!("MPIDR=0x{:X}", mpidr), &st.gray);
        row += 1;
    }
    write_category_label(ws, "CPU", cpu_start, row - 1, st);

    // --- Memory 行（系统内存）---
    let mem_start = row;
    for r in &report.memory_matrix {
        let label = format!(
            "{}, {}",
            AddressRange::fmt_addr(r.range.start),
            AddressRange::fmt_size(r.range.size)
        );
        let shared = !r.note.is_empty();
        ws_str(ws, row, 1, &label, if shared { &st.shared_bold } else { &st.normal });
        for (i, os_name) in report.os_names.iter().enumerate() {
            if *os_name == r.os_name {
                ws_str(ws, row, 2 + i as u16, "√", &st.check);
            } else {
                ws_blank(ws, row, 2 + i as u16, &st.normal);
            }
        }
        ws_str(ws, row, 2 + os_count as u16, &r.note, if shared { &st.shared } else { &st.gray });
        row += 1;
    }
    write_category_label(ws, "Memory", mem_start, row - 1, st);

    // --- 外设行（按类别分组）---
    for ptype in CATEGORY_ORDER {
        let rows: Vec<_> = report.peripheral_rows.iter().filter(|r| &r.ptype == ptype).collect();
        if rows.is_empty() {
            continue;
        }
        let cat_start = row;
        for r in rows {
            let shared = r.enabled_os_count() > 1;
            ws_str(ws, row, 1, &r.name, if shared { &st.shared_bold } else { &st.normal });
            for (i, presence) in r.presence.iter().enumerate() {
                match presence {
                    Presence::Enabled => ws_str(ws, row, 2 + i as u16, "√", &st.check),
                    Presence::Disabled => ws_str(ws, row, 2 + i as u16, "×", &st.gray),
                    Presence::Absent => ws_blank(ws, row, 2 + i as u16, &st.normal),
                }
            }
            let note = if shared {
                let os_list: Vec<&String> = r
                    .presence
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| **p == Presence::Enabled)
                    .map(|(i, _)| &report.os_names[i])
                    .collect();
                format!("共享: {}", os_list.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))
            } else {
                r.note.clone()
            };
            ws_str(ws, row, 2 + os_count as u16, &note, if shared { &st.shared } else { &st.gray });
            row += 1;
        }
        write_category_label(ws, ptype.as_str(), cat_start, row - 1, st);
    }
    autowidth_apply(ws)?;
    Ok(())
}

/// 写入并合并类别列标签。
fn write_category_label(ws: &mut Worksheet, label: &str, start_row: u32, end_row: u32, st: &Styles) {
    if end_row < start_row {
        return;
    }
    // merge_range 不经过 ws_str，手动补记列宽
    COL_RECORDS.with(|r| r.borrow_mut().push((0, label.to_string())));
    for r in start_row + 1..=end_row {
        ws_blank(ws, r, 0, &st.category);
    }
    if end_row > start_row {
        let _ = ws.merge_range(start_row, 0, end_row, 0, label, &st.category);
    } else {
        ws_str(ws, start_row, 0, label, &st.category);
    }
}

// ---------------------------------------------------------------------------
// Sheet 3: 内存分配矩阵
// ---------------------------------------------------------------------------

fn write_memory_matrix(wb: &mut Workbook, report: &AnalysisReport, st: &Styles) -> Result<()> {
    let ws = add_sheet(wb, "内存分配矩阵")?;
    autowidth_begin();
    ws.set_freeze_panes(2, 0)?;

    ws.merge_range(0, 0, 0, 7, "内存分配矩阵（系统内存 + 保留内存）", &st.title)?;

    write_header_row(
        ws,
        1,
        &["起始地址", "结束地址", "大小(hex)", "大小", "类型", "节点 / 名称", "OS", "备注"],
        st,
    );

    #[derive(Clone)]
    struct RowData {
        range: AddressRange,
        type_str: String,
        name: String,
        os: String,
        note: String,
        shared: bool,
        reserved: bool,
    }

    let mut rows: Vec<RowData> = Vec::new();
    for r in &report.memory_matrix {
        rows.push(RowData {
            range: r.range,
            type_str: "System".into(),
            name: r.node_path.clone(),
            os: r.os_name.clone(),
            note: r.note.clone(),
            shared: !r.note.is_empty(),
            reserved: false,
        });
    }
    for os in &report.os_resources {
        for region in &os.reserved_regions {
            let disabled = region.attributes.iter().any(|a| a == "status=disabled");
            rows.push(RowData {
                range: region.range,
                type_str: region.region_type.as_str().into(),
                name: region.name.clone(),
                os: os.os_name.clone(),
                note: if disabled { "status=disabled".into() } else { region.attributes.join(", ") },
                shared: region.region_type == MemoryType::Shared,
                reserved: true,
            });
        }
    }
    rows.sort_by_key(|r| (r.range.start, r.reserved, r.os.clone()));

    for (i, r) in rows.iter().enumerate() {
        let row = 2 + i as u32;
        let fmt = if r.shared { &st.shared } else { &st.normal };
        ws_str(ws, row, 0, &AddressRange::fmt_addr(r.range.start), fmt);
        ws_str(ws, row, 1, &AddressRange::fmt_addr(r.range.last()), fmt);
        ws_str(ws, row, 2, &format!("0x{:X}", r.range.size), fmt);
        ws_str(ws, row, 3, &AddressRange::fmt_size(r.range.size), fmt);
        ws_str(ws, row, 4, &r.type_str, fmt);
        ws_str(ws, row, 5, &r.name, fmt);
        ws_str(ws, row, 6, &r.os, fmt);
        ws_str(ws, row, 7, &r.note, fmt);
    }
    autowidth_apply(ws)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Sheet 4: 共享资源清单
// ---------------------------------------------------------------------------

fn write_shared_resources(wb: &mut Workbook, report: &AnalysisReport, st: &Styles) -> Result<()> {
    let ws = add_sheet(wb, "共享资源清单")?;
    autowidth_begin();
    ws.set_freeze_panes(1, 0)?;

    write_header_row(ws, 0, &["资源名称", "共享类型", "地址范围", "共享 OS", "详细信息"], st);

    // 排序：按类型聚合
    let mut items: Vec<&_> = report.shared_resources.iter().collect();
    items.sort_by_key(|s| (kind_order(s.kind), s.range.map(|r| r.start).unwrap_or(0)));

    let mut row = 1u32;
    for s in items {
        let fmt = match s.kind {
            SharedKind::MemoryOverlap => &st.shared_bold,
            SharedKind::Peripheral => &st.normal,
            _ => &st.shared,
        };
        ws_str(ws, row, 0, &s.name, fmt);
        ws_str(ws, row, 1, s.kind.as_str(), fmt);
        let range_str = match s.range {
            Some(r) if r.size > 0 => format!(
                "{} ~ {} ({})",
                AddressRange::fmt_addr(r.start),
                AddressRange::fmt_addr(r.last()),
                AddressRange::fmt_size(r.size)
            ),
            Some(r) => AddressRange::fmt_addr(r.start),
            None => String::new(),
        };
        ws_str(ws, row, 2, &range_str, fmt);
        ws_str(ws, row, 3, &s.os_list.join(", "), fmt);
        ws_str(ws, row, 4, &s.details, fmt);
        row += 1;
    }
    if row == 1 {
        ws_str(ws, 1, 0, "未发现共享资源", &st.gray);
    }
    autowidth_apply(ws)?;
    Ok(())
}

fn kind_order(kind: SharedKind) -> u8 {
    match kind {
        SharedKind::MemoryOverlap => 0,
        SharedKind::ReservedMemory => 1,
        SharedKind::GipcIpc => 2,
        SharedKind::SubsystemSharedMemory => 3,
        SharedKind::Peripheral => 4,
        SharedKind::Interrupt => 5,
    }
}

// ---------------------------------------------------------------------------
// Sheet 5: 外设分配
// ---------------------------------------------------------------------------

fn write_peripheral_allocation(wb: &mut Workbook, report: &AnalysisReport, st: &Styles) -> Result<()> {
    let ws = add_sheet(wb, "外设分配")?;
    let os_count = report.os_names.len();
    autowidth_begin();
    ws.set_freeze_panes(1, 0)?;

    let mut headers: Vec<&str> = vec!["类别", "外设", "基地址", "IRQ"];
    let os_refs: Vec<&str> = report.os_names.iter().map(|s| s.as_str()).collect();
    headers.extend_from_slice(&os_refs);
    headers.push("compatible");
    headers.push("备注");
    write_header_row(ws, 0, &headers, st);

    // 从各 OS 中补充 compatible 信息
    let mut last_category: Option<PeripheralType> = None;
    for (i, r) in report.peripheral_rows.iter().enumerate() {
        let row = 1 + i as u32;
        let compat: Vec<String> = report
            .os_resources
            .iter()
            .flat_map(|os| os.peripherals.iter())
            .find(|p| p.name == r.name)
            .map(|p| p.compatible.clone())
            .unwrap_or_default();

        let shared = r.enabled_os_count() > 1;
        let fmt = if shared { &st.shared } else { &st.normal };

        let cat_cell = if last_category.as_ref() != Some(&r.ptype) {
            last_category = Some(r.ptype.clone());
            r.ptype.as_str().to_string()
        } else {
            String::new()
        };
        ws_str(ws, row, 0, &cat_cell, &st.category);
        ws_str(ws, row, 1, &r.name, if shared { &st.shared_bold } else { &st.normal });
        let base = r.base_addr.map(|a| format!("0x{:08X}", a)).unwrap_or_default();
        ws_str(ws, row, 2, &base, fmt);
        let irqs: Vec<String> = r.irqs.iter().map(|i| i.to_string()).collect();
        ws_str(ws, row, 3, &irqs.join(","), fmt);
        for (i, presence) in r.presence.iter().enumerate() {
            match presence {
                Presence::Enabled => ws_str(ws, row, 4 + i as u16, "√", &st.check),
                Presence::Disabled => ws_str(ws, row, 4 + i as u16, "disabled", &st.gray),
                Presence::Absent => ws_str(ws, row, 4 + i as u16, "-", &st.gray),
            }
        }
        ws_str(ws, row, 4 + os_count as u16, &compat.join(", "), fmt);
        let note = if shared {
            format!("多 OS 共享: {}", r.enabled_os_count())
        } else {
            r.note.clone()
        };
        ws_str(ws, row, 5 + os_count as u16, &note, fmt);
    }
    autowidth_apply(ws)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Sheet 6: 冲突报告
// ---------------------------------------------------------------------------

fn write_conflicts(wb: &mut Workbook, report: &AnalysisReport, st: &Styles) -> Result<()> {
    let ws = add_sheet(wb, "冲突报告")?;
    autowidth_begin();
    ws.set_freeze_panes(1, 0)?;

    write_header_row(ws, 0, &["冲突类型", "资源", "涉及 OS", "描述", "建议"], st);

    if report.conflicts.is_empty() {
        ws.merge_range(1, 0, 1, 4, "未检测到冲突", &st.ok)?;
        autowidth_apply(ws)?;
        return Ok(());
    }

    let mut items: Vec<&_> = report.conflicts.iter().collect();
    items.sort_by_key(|c| (conflict_order(c.conflict_type), c.resource_name.clone()));

    for (i, c) in items.iter().enumerate() {
        let row = 1 + i as u32;
        let fmt = match c.conflict_type {
            ConflictType::CpuConflict => &st.conflict,
            ConflictType::PeripheralConflict => &st.shared,
            _ => &st.conflict,
        };
        ws_str(ws, row, 0, c.conflict_type.as_str(), fmt);
        ws_str(ws, row, 1, &c.resource_name, fmt);
        ws_str(ws, row, 2, &c.os_list.join(", "), fmt);
        ws_str(ws, row, 3, &c.description, fmt);
        ws_str(ws, row, 4, &c.suggestion, fmt);
    }
    autowidth_apply(ws)?;
    Ok(())
}

fn conflict_order(t: ConflictType) -> u8 {
    match t {
        ConflictType::CpuConflict => 0,
        ConflictType::MemoryOverlap => 1,
        ConflictType::InterruptConflict => 2,
        ConflictType::PeripheralConflict => 3,
    }
}
