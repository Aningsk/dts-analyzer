//! DTS 属性模型：属性值由字符串、cell 数组、字节数组三类片段组成。

/// 属性值片段。
#[derive(Debug, Clone, PartialEq)]
pub enum PropItem {
    /// 字符串（可能含 `\0` 分隔的多个子串）。
    Str(String),
    /// cell 数组 `<...>` 中的 32 位数值序列。
    Cells(Vec<u32>),
    /// 字节数组 `[..]`。
    Bytes(Vec<u8>),
}

/// 一个 DTS 属性。
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub name: String,
    pub items: Vec<PropItem>,
}

impl Property {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), items: Vec::new() }
    }

    /// 是否存在有效值（无值属性如 `no-map;` 返回 false）。
    pub fn has_value(&self) -> bool {
        !self.items.is_empty()
    }

    /// 收集全部字符串片段（不拆分 `\0`）。
    pub fn string_items(&self) -> Vec<&str> {
        self.items.iter().filter_map(|i| match i {
            PropItem::Str(s) => Some(s.as_str()),
            _ => None,
        }).collect()
    }

    /// 将字符串值按 `\0` 拆分为子串列表（compatible 等属性常用）。
    pub fn as_strings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for s in self.string_items() {
            for part in s.split('\0') {
                if !part.is_empty() {
                    out.push(part.to_string());
                }
            }
        }
        out
    }

    /// 取第一个字符串值（去除 `\0` 尾部）。
    pub fn as_string(&self) -> Option<String> {
        self.string_items().first().map(|s| s.trim_end_matches('\0').to_string())
    }

    /// 若属性值全部为 cell 数组，拼接返回全部 cell。
    pub fn as_cells(&self) -> Option<Vec<u32>> {
        if self.items.is_empty() {
            return None;
        }
        let mut out = Vec::new();
        for item in &self.items {
            match item {
                PropItem::Cells(c) => out.extend_from_slice(c),
                _ => return None,
            }
        }
        Some(out)
    }

    /// 取第一个 cell 值。
    pub fn first_cell(&self) -> Option<u32> {
        self.as_cells().and_then(|c| c.first().copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_as_strings_split_nul() {
        let p = Property {
            name: "compatible".into(),
            items: vec![PropItem::Str("arm,scmi\0arm,scmi2\0".into())],
        };
        assert_eq!(p.as_strings(), vec!["arm,scmi", "arm,scmi2"]);
    }

    #[test]
    fn test_as_cells() {
        let p = Property {
            name: "reg".into(),
            items: vec![PropItem::Cells(vec![0, 0x1000, 0, 0x100])],
        };
        assert_eq!(p.as_cells().unwrap(), vec![0, 0x1000, 0, 0x100]);
        assert_eq!(p.first_cell(), Some(0));
    }

    #[test]
    fn test_bool_property() {
        let p = Property::new("no-map");
        assert!(!p.has_value());
        assert!(p.as_cells().is_none());
    }
}
