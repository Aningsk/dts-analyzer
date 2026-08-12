//! DTB（编译后的设备树二进制）解析适配器。
//!
//! 使用 [`fdt`] crate 解析 FDT blob，并转换为与 DTS 文本解析器一致的
//! [`DtsNode`] / [`DtsFile`] 模型，供上层 analyzer 统一消费。
//!
//! DTB 中属性值为无类型原始字节，按 dtc 反编译的启发式还原：
//! 字符串（`\0` 分隔的可打印序列）→ [`PropItem::Str`]，
//! 其余按 4 字节大端 cell 还原为 [`PropItem::Cells`]。
//! DTB 不含 label、注释与顶层 `/memreserve/` 之外的源码信息。

use std::path::Path;

use super::node::DtsNode;
use super::parser::DtsFile;
use super::property::{PropItem, Property};
use crate::utils::address::AddressRange;

/// FDT blob 魔数（大端 0xd00dfeed）。
pub const FDT_MAGIC: [u8; 4] = [0xd0, 0x0d, 0xfe, 0xed];

/// 判断字节内容是否为 DTB 二进制（按魔数识别）。
pub fn is_dtb_bytes(data: &[u8]) -> bool {
    data.len() >= 4 && data[..4] == FDT_MAGIC
}

/// DTB 解析错误。
#[derive(Debug, thiserror::Error)]
pub enum DtbError {
    #[error("{path}: 无法读取文件: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: 无效的 DTB 文件: {msg}")]
    Invalid { path: std::path::PathBuf, msg: String },
}

/// 解析 DTB 文件为 [`DtsFile`]。
pub fn parse_dtb_file(path: &Path) -> Result<DtsFile, DtbError> {
    let bytes = std::fs::read(path)
        .map_err(|e| DtbError::Io { path: path.to_path_buf(), source: e })?;
    parse_dtb_bytes(&bytes, &path.display().to_string()).map_err(|msg| DtbError::Invalid {
        path: path.to_path_buf(),
        msg,
    })
}

/// 解析 DTB 字节内容。`file` 仅用于日志。
pub fn parse_dtb_bytes(bytes: &[u8], file: &str) -> Result<DtsFile, String> {
    let fdt = fdt::Fdt::new(bytes).map_err(|e| e.to_string())?;

    let fdt_root = fdt.find_node("/").ok_or_else(|| "找不到根节点".to_string())?;
    // 根节点在 FDT 中名为空串，先归一化为 "/"（含根节点自身属性），
    // 再递归转换子树（assign_paths 依赖根节点名为 "/"）
    let mut root = DtsNode::new("/");
    for prop in fdt_root.properties() {
        root.properties.insert(prop.name.to_string(), convert_property(prop.name, prop.value));
    }
    for child in fdt_root.children() {
        root.children.insert(child.name.to_string(), convert_node(child));
    }
    root.assign_paths();

    let memreserves: Vec<AddressRange> = fdt
        .memory_reservations()
        .map(|r| AddressRange::new(r.address() as u64, r.size() as u64))
        .collect();

    log::debug!("DTB [{}] 解析完成: {} 个节点", file, root.count_nodes());
    Ok(DtsFile { root, memreserves, version: None })
}

/// 递归转换 fdt 节点为 [`DtsNode`]。
fn convert_node(node: fdt::node::FdtNode<'_, '_>) -> DtsNode {
    let mut out = DtsNode::new(node.name);
    for prop in node.properties() {
        out.properties.insert(prop.name.to_string(), convert_property(prop.name, prop.value));
    }
    for child in node.children() {
        out.children.insert(child.name.to_string(), convert_node(child));
    }
    out
}

/// 按 dtc 反编译启发式还原属性值类型。
fn convert_property(name: &str, value: &[u8]) -> Property {
    let mut prop = Property::new(name);
    if value.is_empty() {
        // 布尔属性（如 no-map）
        return prop;
    }
    if looks_like_string(value) {
        // 去掉末尾 NUL 后保留中间分隔符，与 DTS 文本解析器的 Str 语义一致
        let s = String::from_utf8_lossy(&value[..value.len() - 1]).into_owned();
        prop.items.push(PropItem::Str(s));
    } else {
        let cells: Vec<u32> = value
            .chunks(4)
            .map(|c| {
                let mut buf = [0u8; 4];
                buf[..c.len()].copy_from_slice(c);
                u32::from_be_bytes(buf)
            })
            .collect();
        prop.items.push(PropItem::Cells(cells));
    }
    prop
}

/// dtc 反编译的字符串判断启发式（对齐 util_is_printable_string 的对外行为）：
/// 以 `\0` 结尾，且每个 NUL 分隔段都非空、全为可打印 ASCII。
/// 这样 `reg = <0x0 0x2a000000 0x0 0x480000>`（首段为空）不会被误判为字符串。
fn looks_like_string(value: &[u8]) -> bool {
    if value.is_empty() || *value.last().unwrap() != 0 {
        return false;
    }
    // 剥掉末尾 NUL 后按 NUL 分段，每段均需非空且全为可打印 ASCII
    value[..value.len() - 1]
        .split(|&b| b == 0)
        .all(|seg| !seg.is_empty() && seg.iter().all(|&b| (0x20..0x7f).contains(&b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 依赖系统 dtc 将 DTS 编译为 DTB 后走 DTB 路径解析。
    #[test]
    fn test_parse_dtb_roundtrip() {
        if std::process::Command::new("dtc").arg("--version").output().is_err() {
            eprintln!("dtc 不可用，跳过");
            return;
        }
        let dts = r#"
/dts-v1/;
/ {
    #address-cells = <2>;
    #size-cells = <2>;
    model = "dtb test";
    memory@80000000 {
        device_type = "memory";
        reg = <0x0 0x80000000 0x0 0x40000000>;
    };
    soc {
        serial0: serial@1000 {
            compatible = "vendor,uart", "ns16550";
            reg = <0x0 0x1000 0x0 0x100>;
            status = "okay";
        };
    };
};
"#;
        let out = std::process::Command::new("dtc")
            .args(["-I", "dts", "-O", "dtb", "-o", "/dev/stdout"])
            .arg("-")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write;
                child.stdin.as_mut().unwrap().write_all(dts.as_bytes())?;
                child.wait_with_output()
            })
            .unwrap();
        assert!(out.status.success(), "dtc 编译失败");

        let file = parse_dtb_bytes(&out.stdout, "test.dtb").unwrap();
        assert_eq!(file.root.get_property("model").unwrap().as_string().as_deref(), Some("dtb test"));
        assert_eq!(file.root.get_property("#address-cells").unwrap().as_cells().unwrap(), vec![2]);

        let mem = file.root.find_path("/memory@80000000").unwrap();
        assert_eq!(mem.get_property("device_type").unwrap().as_string().as_deref(), Some("memory"));
        assert_eq!(
            mem.get_property("reg").unwrap().as_cells().unwrap(),
            vec![0, 0x8000_0000, 0, 0x4000_0000]
        );

        let serial = file.root.find_path("/soc/serial@1000").unwrap();
        assert_eq!(serial.compatibles(), vec!["vendor,uart", "ns16550"]);
        assert!(serial.is_enabled());
    }

    #[test]
    fn test_looks_like_string() {
        assert!(looks_like_string(b"okay\0"));
        assert!(looks_like_string(b"a,b\0c,d\0"));
        assert!(!looks_like_string(&[0, 0, 0x80, 0]));
        assert!(!looks_like_string(b"no nul"));
        // reg = <0x0 0x2a000000 0x0 0x480000>：字节恰为可打印 ASCII + NUL，
        // 但首段为空 → 按 cells 处理（与 dtc 反编译行为一致）
        assert!(!looks_like_string(&[0, 0, 0, 0, 0x2a, 0, 0, 0, 0, 0, 0, 0, 0, 0x48, 0, 0]));
    }
}
