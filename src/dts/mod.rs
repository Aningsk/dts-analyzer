//! DTS 解析模块：将 Device Tree Source 文本解析为节点树。

pub mod node;
pub mod parser;
pub mod property;

pub use node::DtsNode;
pub use parser::{parse_dts_file, parse_dts_text, DtsFile};
pub use property::{PropItem, Property};
