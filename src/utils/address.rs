//! 地址范围处理工具。

use std::fmt;

/// 一段连续的物理地址区间（左闭右开语义由 `end()` 体现）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AddressRange {
    pub start: u64,
    pub size: u64,
}

impl AddressRange {
    pub fn new(start: u64, size: u64) -> Self {
        Self { start, size }
    }

    /// 区间末地址（不含），使用饱和加法避免溢出。
    pub fn end(&self) -> u64 {
        self.start.saturating_add(self.size)
    }

    /// 区间末地址（含）。
    pub fn last(&self) -> u64 {
        if self.size == 0 {
            self.start
        } else {
            self.end() - 1
        }
    }

    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// 判断两个区间是否重叠。
    pub fn overlaps(&self, other: &AddressRange) -> bool {
        self.size > 0 && other.size > 0 && self.start < other.end() && other.start < self.end()
    }

    /// 返回重叠部分（若有）。
    pub fn intersection(&self, other: &AddressRange) -> Option<AddressRange> {
        if !self.overlaps(other) {
            return None;
        }
        let start = self.start.max(other.start);
        let end = self.end().min(other.end());
        Some(AddressRange::new(start, end - start))
    }

    /// 判断该区间是否完全包含另一个区间。
    pub fn contains(&self, other: &AddressRange) -> bool {
        self.start <= other.start && other.end() <= self.end()
    }

    /// 判断两个区间是否首尾相接（可用于合并连续区域）。
    pub fn adjacent_to(&self, other: &AddressRange) -> bool {
        self.end() == other.start || other.end() == self.start
    }

    /// 格式化为 `0x0000EF800000` 风格（12 位十六进制，不足补零）。
    pub fn fmt_addr(addr: u64) -> String {
        format!("0x{:012X}", addr)
    }

    /// 将字节数格式化为可读字符串（B / KB / MB / GB）。
    pub fn fmt_size(size: u64) -> String {
        const GB: u64 = 1024 * 1024 * 1024;
        const MB: u64 = 1024 * 1024;
        const KB: u64 = 1024;
        if size.is_multiple_of(GB) && size >= GB {
            format!("{} GB", size / GB)
        } else if size.is_multiple_of(MB) && size >= MB {
            format!("{} MB", size / MB)
        } else if size.is_multiple_of(KB) && size >= KB {
            format!("{} KB", size / KB)
        } else {
            format!("{} B", size)
        }
    }
}

impl fmt::Display for AddressRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} .. {} (size 0x{:X}, {})",
            Self::fmt_addr(self.start),
            Self::fmt_addr(self.last()),
            self.size,
            Self::fmt_size(self.size)
        )
    }
}

/// 解析十六进制字符串（支持 `0x` 前缀、大小写混合）。
pub fn parse_hex(s: &str) -> Option<u64> {
    let s = s.trim();
    let s = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u64::from_str_radix(s, 16).ok()
}

/// 将 cells 数组按 `#address-cells` / `#size-cells` 解析为地址区间列表。
pub fn cells_to_ranges(cells: &[u32], address_cells: usize, size_cells: usize) -> Vec<AddressRange> {
    let mut ranges = Vec::new();
    let entry = address_cells + size_cells;
    if entry == 0 || address_cells == 0 {
        return ranges;
    }
    for chunk in cells.chunks_exact(entry) {
        let start = join_cells(&chunk[..address_cells]);
        let size = if size_cells > 0 {
            join_cells(&chunk[address_cells..])
        } else {
            0
        };
        ranges.push(AddressRange::new(start, size));
    }
    ranges
}

/// 将大端序 cell 序列拼接为 u64（最多取低 64 位）。
pub fn join_cells(cells: &[u32]) -> u64 {
    let mut v: u64 = 0;
    for (i, c) in cells.iter().rev().enumerate() {
        if i >= 2 {
            break;
        }
        v |= (*c as u64) << (32 * i);
    }
    v
}

/// 合并相邻/重叠的区间，返回不相交的区间集合（按起始地址排序）。
pub fn merge_ranges(mut ranges: Vec<AddressRange>) -> Vec<AddressRange> {
    ranges.sort_by_key(|r| r.start);
    let mut merged: Vec<AddressRange> = Vec::new();
    for r in ranges {
        if r.is_empty() {
            continue;
        }
        match merged.last_mut() {
            Some(last) if r.start <= last.end() => {
                let new_end = last.end().max(r.end());
                last.size = new_end - last.start;
            }
            _ => merged.push(r),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overlap() {
        let a = AddressRange::new(0x1000, 0x1000);
        let b = AddressRange::new(0x1500, 0x1000);
        let c = AddressRange::new(0x2000, 0x1000);
        assert!(a.overlaps(&b));
        assert!(!a.overlaps(&c));
    }

    #[test]
    fn test_intersection() {
        let a = AddressRange::new(0x1000, 0x2000);
        let b = AddressRange::new(0x2000, 0x2000);
        let i = a.intersection(&b).unwrap();
        assert_eq!(i.start, 0x2000);
        assert_eq!(i.size, 0x1000);
    }

    #[test]
    fn test_cells_to_ranges() {
        // reg = <0x00 0xef800000 0x00 0x8000000>  (2 address cells, 2 size cells)
        let cells = [0x00u32, 0xef80_0000, 0x00, 0x0800_0000];
        let ranges = cells_to_ranges(&cells, 2, 2);
        assert_eq!(ranges.len(), 1);
        assert_eq!(ranges[0].start, 0xef80_0000);
        assert_eq!(ranges[0].size, 0x0800_0000);
    }

    #[test]
    fn test_merge() {
        let merged = merge_ranges(vec![
            AddressRange::new(0x1000, 0x1000),
            AddressRange::new(0x2000, 0x1000),
            AddressRange::new(0x5000, 0x100),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].size, 0x2000);
    }

    #[test]
    fn test_fmt() {
        assert_eq!(AddressRange::fmt_addr(0xef80_0000), "0x0000EF800000");
        assert_eq!(AddressRange::fmt_size(128 * 1024 * 1024), "128 MB");
    }
}
