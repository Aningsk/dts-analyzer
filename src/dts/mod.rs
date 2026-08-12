//! DTS/DTB 解析模块：将 Device Tree Source 文本或编译后的 DTB 二进制解析为节点树。

pub mod dtb;
pub mod node;
pub mod parser;
pub mod property;

pub use dtb::{parse_dtb_bytes, parse_dtb_file, DtbError};
pub use node::DtsNode;
pub use parser::{parse_dts_file, parse_dts_text, DtsFile};
pub use property::{PropItem, Property};

/// 统一输入入口：按文件内容魔数自动分流。
///
/// - `\xd0\x0d\xfe\xed`（FDT blob）→ DTB 二进制解析（fdt crate）
/// - 其余 → DTS 文本解析（自写 parser）
pub fn parse_input_file(path: &std::path::Path) -> anyhow::Result<DtsFile> {
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow::anyhow!("无法读取文件 {}: {}", path.display(), e))?;
    if dtb::is_dtb_bytes(&bytes) {
        let file = parse_dtb_bytes(&bytes, &path.display().to_string())
            .map_err(|msg| anyhow::anyhow!("{}: 无效的 DTB 文件: {}", path.display(), msg))?;
        log::debug!("输入 [{}] 识别为 DTB 二进制", path.display());
        Ok(file)
    } else {
        let src = String::from_utf8(bytes)
            .map_err(|e| anyhow::anyhow!("{}: 既非 DTB 也非有效 UTF-8 文本: {}", path.display(), e))?;
        log::debug!("输入 [{}] 识别为 DTS 文本", path.display());
        parse_dts_text(&src, &path.display().to_string())
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
}
