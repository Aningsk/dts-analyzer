//! 资源数据结构与单 OS 资源提取。

use std::collections::BTreeMap;

use crate::config::{OsConfig, Rules};
use crate::dts::{DtsFile, DtsNode};
use crate::utils::address::{cells_to_ranges, AddressRange};

/// CPU 核心信息。
#[derive(Debug, Clone)]
pub struct CpuInfo {
    /// 节点名（如 `cpu@20300`）。
    pub name: String,
    /// MPIDR（reg 属性值），作为全局唯一标识。
    pub mpidr: u32,
    pub compatible: Vec<String>,
}

/// 内存区域类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    /// 系统主内存（memory 节点）。
    System,
    /// 保留内存（reserved-memory）。
    Reserved,
    /// 明确标注共享的内存（shared-dma-pool / gipc / shared-memory 等）。
    Shared,
    /// DMA 池。
    Dma,
    /// 安全区域（secure / optee）。
    Secure,
}

impl MemoryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::System => "System",
            MemoryType::Reserved => "Reserved",
            MemoryType::Shared => "Shared",
            MemoryType::Dma => "DMA",
            MemoryType::Secure => "Secure",
        }
    }
}

/// 内存区域。
#[derive(Debug, Clone)]
pub struct MemoryRegion {
    /// 区域名称（节点名）。
    pub name: String,
    /// 节点路径。
    pub node_path: String,
    pub range: AddressRange,
    pub region_type: MemoryType,
    pub compatible: Vec<String>,
    /// 布尔/字符串属性摘要（no-map、reusable、status 等）。
    pub attributes: Vec<String>,
}

/// 中断描述（GIC 三元组解析结果）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IrqInfo {
    /// GIC 中断类型（0 = SPI, 1 = PPI）。
    pub type_cell: u32,
    /// 中断号（GIC 内部编号）。
    pub number: u32,
    /// 触发方式 flags（bit0 高电平，bit1 低电平，bit2 上升沿，bit3 下降沿）。
    pub flags: u32,
}

impl IrqInfo {
    /// 换算为 Linux 风格 IRQ 号（SPI +32，PPI +16）。
    pub fn linux_irq(&self) -> u32 {
        match self.type_cell {
            0 => self.number + 32,
            1 => self.number + 16,
            _ => self.number,
        }
    }

    pub fn trigger_str(&self) -> &'static str {
        match self.flags & 0x0F {
            0x01 => "level-high",
            0x02 => "level-low",
            0x04 => "edge-rising",
            0x08 => "edge-falling",
            _ => "unknown",
        }
    }
}

/// 外设类别。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PeripheralType {
    Cpu,
    InterruptController,
    Uart,
    I2c,
    Spi,
    Gpio,
    Pwm,
    Ethernet,
    Wireless,
    Pcie,
    Usb,
    Can,
    Storage,
    Gpu,
    Display,
    Camera,
    Video,
    Audio,
    Iommu,
    Watchdog,
    Crypto,
    Other,
}

impl PeripheralType {
    pub fn as_str(&self) -> &'static str {
        match self {
            PeripheralType::Cpu => "CPU",
            PeripheralType::InterruptController => "GIC",
            PeripheralType::Uart => "UART",
            PeripheralType::I2c => "I2C",
            PeripheralType::Spi => "SPI",
            PeripheralType::Gpio => "GPIO",
            PeripheralType::Pwm => "PWM",
            PeripheralType::Ethernet => "Ethernet",
            PeripheralType::Wireless => "Wireless",
            PeripheralType::Pcie => "PCIe",
            PeripheralType::Usb => "USB",
            PeripheralType::Can => "CAN",
            PeripheralType::Storage => "Storage",
            PeripheralType::Gpu => "GPU",
            PeripheralType::Display => "Display",
            PeripheralType::Camera => "Camera/ISP",
            PeripheralType::Video => "Video",
            PeripheralType::Audio => "Audio",
            PeripheralType::Iommu => "IOMMU",
            PeripheralType::Watchdog => "WDT",
            PeripheralType::Crypto => "Crypto",
            PeripheralType::Other => "Other",
        }
    }
}

/// 外设实例。
#[derive(Debug, Clone)]
pub struct Peripheral {
    /// 节点名（如 `serial@270a1000`）。
    pub name: String,
    /// 节点路径（如 `/soc/serial@270a1000`）。
    pub node_path: String,
    pub peripheral_type: PeripheralType,
    pub compatible: Vec<String>,
    pub reg_ranges: Vec<AddressRange>,
    pub interrupts: Vec<IrqInfo>,
    /// status 属性值（默认 "okay"）。
    pub status: String,
    /// 附加说明（如 os-type）。
    pub note: String,
}

impl Peripheral {
    pub fn is_enabled(&self) -> bool {
        self.status == "okay" || self.status == "ok"
    }

    /// 跨 OS 识别外设身份的键：节点名（含 unit-address）。
    pub fn identity(&self) -> String {
        self.name.clone()
    }
}

/// 单个 OS 的全部资源。
#[derive(Debug, Clone)]
pub struct OsResources {
    pub os_name: String,
    pub dts_path: String,
    pub cpus: Vec<CpuInfo>,
    pub memory_regions: Vec<MemoryRegion>,
    pub reserved_regions: Vec<MemoryRegion>,
    pub peripherals: Vec<Peripheral>,
    /// /memreserve/ 区段。
    pub memreserves: Vec<AddressRange>,
    /// 总系统内存字节数。
    pub total_memory: u64,
}

/// 全局 CPU 分配表：MPIDR -> 拥有该核心的 OS 列表。
pub type CpuMatrix = BTreeMap<u32, Vec<String>>;

/// 共享资源类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedKind {
    /// reserved-memory 中出现在多个 OS DTS 的区域。
    ReservedMemory,
    /// gipc 跨 OS IPC 缓冲区。
    GipcIpc,
    /// 子系统 shared-memory 节点内的区域。
    SubsystemSharedMemory,
    /// 多个 OS 系统内存重叠。
    MemoryOverlap,
    /// 多个 OS 同时使能的外设。
    Peripheral,
    /// 共享中断。
    Interrupt,
}

impl SharedKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SharedKind::ReservedMemory => "保留内存",
            SharedKind::GipcIpc => "IPC 共享内存(gipc)",
            SharedKind::SubsystemSharedMemory => "子系统共享内存",
            SharedKind::MemoryOverlap => "系统内存重叠",
            SharedKind::Peripheral => "外设",
            SharedKind::Interrupt => "中断",
        }
    }
}

/// 共享资源条目。
#[derive(Debug, Clone)]
pub struct SharedResource {
    pub name: String,
    pub kind: SharedKind,
    pub range: Option<AddressRange>,
    /// 参与共享的 OS（配置顺序）。
    pub os_list: Vec<String>,
    /// 详细描述。
    pub details: String,
}

/// 冲突类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictType {
    MemoryOverlap,
    PeripheralConflict,
    InterruptConflict,
    CpuConflict,
}

impl ConflictType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ConflictType::MemoryOverlap => "内存重叠",
            ConflictType::PeripheralConflict => "外设竞争",
            ConflictType::InterruptConflict => "中断冲突",
            ConflictType::CpuConflict => "CPU 重复分配",
        }
    }
}

/// 冲突条目。
#[derive(Debug, Clone)]
pub struct Conflict {
    pub conflict_type: ConflictType,
    pub resource_name: String,
    pub os_list: Vec<String>,
    pub description: String,
    pub suggestion: String,
}

/// 跨 OS 内存明细行（系统内存）。
#[derive(Debug, Clone)]
pub struct MemoryMatrixRow {
    pub os_name: String,
    pub node_path: String,
    pub range: AddressRange,
    /// 与其他 OS 重叠的备注。
    pub note: String,
}

/// 某 OS 对某外设的占用状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    /// 该 OS 的 DTS 中不存在此外设。
    Absent,
    /// 存在但 status = disabled。
    Disabled,
    /// 存在且使能。
    Enabled,
}

/// 全局外设分配行（资源矩阵用）。
#[derive(Debug, Clone)]
pub struct PeripheralRow {
    pub ptype: PeripheralType,
    pub name: String,
    pub base_addr: Option<u64>,
    pub irqs: Vec<u32>,
    /// 与 os_names 一一对应。
    pub presence: Vec<Presence>,
    pub note: String,
}

impl PeripheralRow {
    pub fn enabled_os_count(&self) -> usize {
        self.presence.iter().filter(|p| **p == Presence::Enabled).count()
    }
}

/// 共享内存明细行（reserved / shared-memory 级别）。
#[derive(Debug, Clone)]
pub struct SharedMemoryRow {
    pub name: String,
    /// 区域来源（父节点路径，如 `/sdd@33000000/shared-memory`）。
    pub source: String,
    pub range: AddressRange,
    pub os_list: Vec<String>,
    pub description: String,
}

/// 完整分析报告。
#[derive(Debug)]
pub struct AnalysisReport {
    pub title: String,
    pub os_names: Vec<String>,
    pub os_resources: Vec<OsResources>,
    /// 与 os_resources 一一对应的解析后设备树（供后续分析复用）。
    pub dts_files: Vec<crate::dts::DtsFile>,
    pub cpu_matrix: CpuMatrix,
    pub memory_matrix: Vec<MemoryMatrixRow>,
    pub peripheral_rows: Vec<PeripheralRow>,
    pub shared_resources: Vec<SharedResource>,
    pub shared_memory_rows: Vec<SharedMemoryRow>,
    pub conflicts: Vec<Conflict>,
}

impl AnalysisReport {
    pub fn new(config: &crate::config::Config) -> Self {
        Self {
            title: config.output.title.clone(),
            os_names: config.os.iter().map(|o| o.name.clone()).collect(),
            os_resources: Vec::new(),
            dts_files: Vec::new(),
            cpu_matrix: BTreeMap::new(),
            memory_matrix: Vec::new(),
            peripheral_rows: Vec::new(),
            shared_resources: Vec::new(),
            shared_memory_rows: Vec::new(),
            conflicts: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// 单 OS 资源提取
// ---------------------------------------------------------------------------

/// 从解析好的 DTS 中提取一个 OS 的全部资源。
pub fn extract_os_resources(os_cfg: &OsConfig, dts: &DtsFile, rules: &Rules) -> OsResources {
    let root = &dts.root;
    let mut res = OsResources {
        os_name: os_cfg.name.clone(),
        dts_path: os_cfg.dts_file.display().to_string(),
        cpus: Vec::new(),
        memory_regions: Vec::new(),
        reserved_regions: Vec::new(),
        peripherals: Vec::new(),
        memreserves: dts.memreserves.clone(),
        total_memory: 0,
    };

    extract_cpus(root, &mut res);
    extract_system_memory(root, rules, &mut res);
    extract_reserved_memory(root, &mut res);
    extract_peripherals(root, &mut res);
    res.total_memory = res.memory_regions.iter().map(|r| r.range.size).sum();
    res
}

/// 提取 /cpus 下的 CPU 核心。
fn extract_cpus(root: &DtsNode, res: &mut OsResources) {
    let Some(cpus) = root.find_path("/cpus") else { return };
    let addr_cells = cell_prop(cpus, "#address-cells").unwrap_or(1);
    for child in cpus.children.values() {
        if !child.name.starts_with("cpu@") {
            continue;
        }
        let mpidr = child
            .get_property("reg")
            .and_then(|p| p.as_cells())
            .map(|cells| crate::utils::address::join_cells(&cells[..addr_cells.min(cells.len())]))
            .unwrap_or(0) as u32;
        res.cpus.push(CpuInfo {
            name: child.name.clone(),
            mpidr,
            compatible: child.compatibles(),
        });
    }
}

/// 提取系统内存（memory 节点）。
fn extract_system_memory(root: &DtsNode, rules: &Rules, res: &mut OsResources) {
    let addr_cells = cell_prop(root, "#address-cells").unwrap_or(2);
    let size_cells = cell_prop(root, "#size-cells").unwrap_or(2);
    for child in root.children.values() {
        let is_memory_name =
            rules.memory_node_names.iter().any(|n| child.name == *n || child.name.starts_with(&format!("{}@", n)));
        let is_memory_type = child
            .get_property("device_type")
            .and_then(|p| p.as_string())
            .map(|s| s == "memory")
            .unwrap_or(false);
        if !(is_memory_name || is_memory_type) {
            continue;
        }
        let Some(cells) = child.get_property("reg").and_then(|p| p.as_cells()) else {
            continue;
        };
        for range in cells_to_ranges(&cells, addr_cells, size_cells) {
            res.memory_regions.push(MemoryRegion {
                name: child.name.clone(),
                node_path: child.path.clone(),
                range,
                region_type: MemoryType::System,
                compatible: child.compatibles(),
                attributes: Vec::new(),
            });
        }
    }
}

/// 提取 /reserved-memory 下的保留区域。
fn extract_reserved_memory(root: &DtsNode, res: &mut OsResources) {
    let Some(reserved) = root.find_path("/reserved-memory") else { return };
    let addr_cells = cell_prop(reserved, "#address-cells").unwrap_or(2);
    let size_cells = cell_prop(reserved, "#size-cells").unwrap_or(2);
    for child in reserved.children.values() {
        let Some(cells) = child.get_property("reg").and_then(|p| p.as_cells()) else {
            continue;
        };
        let compatibles = child.compatibles();
        let mut attrs = Vec::new();
        for flag in ["no-map", "reusable", "compatible"] {
            if child.has_property(flag) && flag != "compatible" {
                attrs.push(flag.to_string());
            }
        }
        if let Some(status) = child.get_property("status").and_then(|p| p.as_string()) {
            attrs.push(format!("status={}", status));
        }

        let region_type = classify_reserved(child, &compatibles);
        for range in cells_to_ranges(&cells, addr_cells, size_cells) {
            res.reserved_regions.push(MemoryRegion {
                name: child.name.clone(),
                node_path: child.path.clone(),
                range,
                region_type,
                compatible: compatibles.clone(),
                attributes: attrs.clone(),
            });
        }
    }
}

/// 对 reserved-memory 子区域分类。
fn classify_reserved(node: &DtsNode, compatibles: &[String]) -> MemoryType {
    let name = node.name.to_lowercase();
    if compatibles.iter().any(|c| c == "shared-dma-pool") {
        return MemoryType::Dma;
    }
    if compatibles.iter().any(|c| c.contains("secure") || c.contains("optee") || c.contains("tz"))
        || name.contains("secure")
        || name.contains("optee")
    {
        return MemoryType::Secure;
    }
    if name.starts_with("gipc") || name.contains("shmem") || name.contains("shm") || name.contains("shared") {
        return MemoryType::Shared;
    }
    MemoryType::Reserved
}

/// 提取外设节点。
///
/// 策略：遍历全树，取满足以下条件的节点：
/// - 拥有 `reg` 属性（可解析为地址区间）；
/// - 不在 `reserved-memory` / `cpus` / `memory` 等专用子树内；
/// - 深度不超过 2（根为 0），避免 protocol@xx 之类的纯配置节点噪声；
///   但对 sdd 等子系统节点放宽：只要名字匹配外设模式即可。
fn extract_peripherals(root: &DtsNode, res: &mut OsResources) {
    let root_addr_cells = cell_prop(root, "#address-cells").unwrap_or(2);
    let root_size_cells = cell_prop(root, "#size-cells").unwrap_or(2);

    for (depth, node) in iter_with_depth(root) {
        if depth == 0 || depth > 2 {
            continue;
        }
        if node.name.starts_with("cpu@") || node.name == "cpus" {
            continue;
        }
        let ptype = classify_peripheral(&node.name, &node.compatibles());
        // 未识别类型且非浅层节点：跳过（减少噪声）
        if ptype == PeripheralType::Other && depth > 1 && node.get_property("compatible").is_none() {
            continue;
        }
        let Some(cells) = node.get_property("reg").and_then(|p| p.as_cells()) else {
            continue;
        };
        if cells.is_empty() {
            continue;
        }
        // 深度 1 的节点使用根的 cells 配置；深度 2 节点多数情况下 unit-address
        // 即物理地址（反编译 DTS 中 soc 无地址转换），同样按根配置解析。
        let ranges = cells_to_ranges(&cells, root_addr_cells, root_size_cells);
        if ranges.is_empty() {
            continue;
        }
        let interrupts = parse_interrupts(node);
        let status = node
            .get_property("status")
            .and_then(|p| p.as_string())
            .unwrap_or_else(|| "okay".to_string());
        let note = node
            .get_property("os-type")
            .and_then(|p| p.as_string())
            .unwrap_or_default();
        res.peripherals.push(Peripheral {
            name: node.name.clone(),
            node_path: node.path.clone(),
            peripheral_type: ptype,
            compatible: node.compatibles(),
            reg_ranges: ranges,
            interrupts,
            status,
            note,
        });
    }

    // 中断控制器单独收集（通常没有 usable reg 或有特殊 reg）
    root.walk(&mut |node| {
        if node.has_property("interrupt-controller") && !node.name.starts_with("cpu") {
            if res.peripherals.iter().any(|p| p.node_path == node.path) {
                return;
            }
            let ranges = node
                .get_property("reg")
                .and_then(|p| p.as_cells())
                .map(|cells| cells_to_ranges(&cells, root_addr_cells, root_size_cells))
                .unwrap_or_default();
            res.peripherals.push(Peripheral {
                name: node.name.clone(),
                node_path: node.path.clone(),
                peripheral_type: PeripheralType::InterruptController,
                compatible: node.compatibles(),
                reg_ranges: ranges,
                interrupts: Vec::new(),
                status: node
                    .get_property("status")
                    .and_then(|p| p.as_string())
                    .unwrap_or_else(|| "okay".to_string()),
                note: String::new(),
            });
        }
    });
}

/// 带深度的先序遍历（根深度为 0）。
fn iter_with_depth(root: &DtsNode) -> Vec<(usize, &DtsNode)> {
    let mut out = Vec::new();
    fn rec<'a>(node: &'a DtsNode, depth: usize, out: &mut Vec<(usize, &'a DtsNode)>) {
        out.push((depth, node));
        for child in node.children.values() {
            rec(child, depth + 1, out);
        }
    }
    rec(root, 0, &mut out);
    out
}

/// 解析 interrupts 属性（GIC 三元组）。
fn parse_interrupts(node: &DtsNode) -> Vec<IrqInfo> {
    let mut out = Vec::new();
    for prop_name in ["interrupts", "interrupts-extended"] {
        let Some(cells) = node.get_property(prop_name).and_then(|p| p.as_cells()) else {
            continue;
        };
        // 仅处理标准三元组；interrupts-extended 的 phandle 前缀无法通用解析，跳过
        if prop_name == "interrupts" {
            for chunk in cells.chunks_exact(3) {
                out.push(IrqInfo {
                    type_cell: chunk[0],
                    number: chunk[1],
                    flags: chunk[2],
                });
            }
        }
    }
    out
}

/// 根据节点名与 compatible 分类外设。
pub fn classify_peripheral(name: &str, compatibles: &[String]) -> PeripheralType {
    let base = name.split('@').next().unwrap_or(name).to_lowercase();
    let base = base.trim_end_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
    let compat: Vec<String> = compatibles.iter().map(|c| c.to_lowercase()).collect();
    let compat_joined = compat.join("|");

    let base_is = |candidates: &[&str]| candidates.contains(&&*base);
    let base_has = |pat: &str| base.contains(pat);
    let compat_has = |pat: &str| compat_joined.contains(pat);

    // 存储优先于其它匹配（ufs 不与其他冲突）
    if base_is(&["ufs", "mmc", "sdhci", "emmc"]) || base_has("ufs") || compat_has("jedec,ufs") || compat_has("mmc") {
        return PeripheralType::Storage;
    }
    if base_is(&["serial", "uart"]) || base_has("uart") || compat_has("uart") || compat_has("serial") {
        return PeripheralType::Uart;
    }
    if base == "i2c" || base_has("i2c-") || compat_has("i2c") {
        // 注意：i2c 下挂的传感器节点不在深度限制内，不会误报
        return PeripheralType::I2c;
    }
    if (base == "spi" || base_has("qspi") || base_has("ospi") || compat_has("spi"))
        && !base_has("spinlock")
        && !compat_has("spinlock")
    {
        return PeripheralType::Spi;
    }
    if base == "gpio" || base_has("gpio") || compat_has("gpio") {
        return PeripheralType::Gpio;
    }
    if base.starts_with("pwm") || base.starts_with("lpwm") || compat_has("pwm") {
        return PeripheralType::Pwm;
    }
    if base == "gxe" || base == "gmac" || base == "ethernet" || base == "eth" || compat_has("ethernet") || compat_has("gmac") || compat_has(",gxe") {
        return PeripheralType::Ethernet;
    }
    if compat_has("wlan") || compat_has("wifi") || compat_has("cnss") || compat_has("bluetooth") || base_has("wlan") || base_has("wifi") {
        return PeripheralType::Wireless;
    }
    if base == "pci" || base == "pcie" || base_has("pcie") || compat_has("pci") {
        return PeripheralType::Pcie;
    }
    if base_has("usb") || base_has("xhci") || base_has("dwc3") || compat_has("usb") {
        return PeripheralType::Usb;
    }
    if base_is(&["can", "m_can", "mcan"]) || compat_has("can") {
        return PeripheralType::Can;
    }
    if base_has("gpu") || compat_has("gpu") || compat_has("mali") {
        return PeripheralType::Gpu;
    }
    if base_has("dpu") || base_has("dp0") || base_has("dp1") || base_has("dp2") || base_has("dsi")
        || base == "crossbar" || base_has("backlight") || base_has("panel") || compat_has("display")
        || compat_has("dpu") {
        return PeripheralType::Display;
    }
    if base_has("isp") || base_has("cim") || base_has("mipi") || base_has("ynr") || base_has("pym")
        || base_has("gdc") || base_has("stitch") || base_has("cam") || compat_has("isp") {
        return PeripheralType::Camera;
    }
    if base_has("video") || base_has("venc") || base_has("vdec") || base_has("jenc") || base_has("jdec")
        || compat_has("video") || compat_has("codec") {
        return PeripheralType::Video;
    }
    if base == "sdd" || base_has("adsp") || base_has("audio") || compat_has("adsp") || compat_has("sdd") {
        return PeripheralType::Audio;
    }
    if base_has("smmu") || base_has("iommu") || compat_has("smmu") || compat_has("iommu") {
        return PeripheralType::Iommu;
    }
    if base == "wdt" || base_has("watchdog") || compat_has("watchdog") || compat_has("wdt") {
        return PeripheralType::Watchdog;
    }
    if base_has("crypto") || base_has("trng") || compat_has("crypto") {
        return PeripheralType::Crypto;
    }
    PeripheralType::Other
}

/// 读取节点 cell 数属性（如 #address-cells）。
fn cell_prop(node: &DtsNode, name: &str) -> Option<usize> {
    node.get_property(name).and_then(|p| p.first_cell()).map(|v| v as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify() {
        assert_eq!(classify_peripheral("serial@270a0000", &[]), PeripheralType::Uart);
        assert_eq!(classify_peripheral("i2c@27080000", &[]), PeripheralType::I2c);
        assert_eq!(classify_peripheral("gxe@28000000", &[]), PeripheralType::Ethernet);
        assert_eq!(classify_peripheral("ufs0@279a0000", &[]), PeripheralType::Storage);
        assert_eq!(classify_peripheral("wdt@21090000", &["gua,gua-wdt".into()]), PeripheralType::Watchdog);
        assert_eq!(classify_peripheral("lpwm0@27918000", &[]), PeripheralType::Pwm);
        assert_eq!(classify_peripheral("dpu_core0@30000000", &[]), PeripheralType::Display);
        assert_eq!(classify_peripheral("hwspinlock@27920000", &["gua,hwspinlock".into()]), PeripheralType::Other);
    }

    #[test]
    fn test_irq_linux_number() {
        let irq = IrqInfo { type_cell: 0, number: 0x1c6 - 32, flags: 4 };
        assert_eq!(irq.linux_irq(), 0x1c6);
        assert_eq!(irq.trigger_str(), "edge-rising");
    }

    #[test]
    fn test_extract_from_text() {
        let dts = crate::dts::parse_dts_text(
            r#"
/dts-v1/;
/ {
    #address-cells = <2>;
    #size-cells = <2>;

    cpus {
        #address-cells = <1>;
        #size-cells = <0>;
        cpu@0 { device_type = "cpu"; compatible = "arm,cortex-a76"; reg = <0x0>; };
        cpu@100 { device_type = "cpu"; compatible = "arm,cortex-a76"; reg = <0x100>; };
    };

    memory@0 {
        device_type = "memory";
        reg = <0x00 0xef800000 0x00 0x8000000>;
    };

    serial@270a1000 {
        compatible = "snps,dw-apb-uart";
        reg = <0x00 0x270a1000 0x00 0x1000>;
        interrupts = <0x00 0x1c7 0x04>;
        status = "okay";
    };
};
"#,
            "test.dts",
        )
        .unwrap();
        let cfg = OsConfig {
            name: "TestOS".into(),
            dts_file: "test.dts".into(),
            aliases: vec![],
        };
        let res = extract_os_resources(&cfg, &dts, &Rules::default());
        assert_eq!(res.cpus.len(), 2);
        assert_eq!(res.cpus[1].mpidr, 0x100);
        assert_eq!(res.memory_regions.len(), 1);
        assert_eq!(res.memory_regions[0].range.start, 0xef80_0000);
        assert_eq!(res.total_memory, 0x800_0000);
        assert!(res.peripherals.iter().any(|p| p.name == "serial@270a1000"));
        let serial = res.peripherals.iter().find(|p| p.name == "serial@270a1000").unwrap();
        assert_eq!(serial.interrupts[0].linux_irq(), 0x1c7 + 32);
    }
}
