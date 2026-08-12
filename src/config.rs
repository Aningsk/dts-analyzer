//! TOML 配置文件解析。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// 顶层配置。
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// 输出配置。
    pub output: OutputConfig,
    /// 各 OS 及其 DTS 文件。
    pub os: Vec<OsConfig>,
    /// 可选解析规则。
    #[serde(default)]
    pub rules: Rules,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OutputConfig {
    /// 输出 Excel 文件路径。
    pub excel_file: PathBuf,
    /// 报告标题（可选，默认 "DTS 资源分析报告"）。
    #[serde(default = "default_title")]
    pub title: String,
}

fn default_title() -> String {
    "DTS 资源分析报告".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct OsConfig {
    /// OS 名称（如 "IVI" / "Cluster" / "ADAS"）。
    pub name: String,
    /// 对应的 DTS 文件路径。
    pub dts_file: PathBuf,
    /// 用于 gipc_<a>_<b> 命名的短名列表（可选，默认取小写 name）。
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// 解析规则配置（可选）。
#[derive(Debug, Clone, Deserialize)]
pub struct Rules {
    /// 系统内存节点的名称模式（默认匹配 memory / memory@*）。
    #[serde(default = "default_memory_node_names")]
    pub memory_node_names: Vec<String>,
    /// 共享内存名称关键字（reserved-memory 内视为共享用途的线索）。
    #[serde(default = "default_shared_keywords")]
    pub shared_keywords: Vec<String>,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            memory_node_names: default_memory_node_names(),
            shared_keywords: default_shared_keywords(),
        }
    }
}

fn default_memory_node_names() -> Vec<String> {
    vec!["memory".to_string()]
}

fn default_shared_keywords() -> Vec<String> {
    vec![
        "gipc".to_string(),
        "shm".to_string(),
        "shmem".to_string(),
        "shared".to_string(),
        "ipc".to_string(),
        "mailbox".to_string(),
    ]
}

impl Config {
    /// 读取并校验配置文件。`base_dir` 用于解析相对路径（默认取配置文件所在目录）。
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("无法读取配置文件: {}", path.display()))?;
        let mut config: Config =
            toml::from_str(&text).with_context(|| format!("配置文件格式错误: {}", path.display()))?;

        if config.os.is_empty() {
            anyhow::bail!("配置文件必须至少包含一个 [[os]] 条目");
        }

        let base_dir = path.parent().filter(|p| !p.as_os_str().is_empty()).map(|p| p.to_path_buf());
        let resolve = |p: &PathBuf| -> PathBuf {
            if p.is_absolute() {
                p.clone()
            } else if let Some(base) = &base_dir {
                base.join(p)
            } else {
                p.clone()
            }
        };

        for os in &mut config.os {
            os.dts_file = resolve(&os.dts_file);
            if !os.dts_file.exists() {
                anyhow::bail!("OS '{}' 的 DTS 文件不存在: {}", os.name, os.dts_file.display());
            }
            if os.aliases.is_empty() {
                os.aliases = vec![os.name.to_lowercase()];
            }
        }
        config.output.excel_file = resolve(&config.output.excel_file);

        // 校验 OS 名称唯一
        let mut seen = std::collections::HashSet::new();
        for os in &config.os {
            if !seen.insert(os.name.to_lowercase()) {
                anyhow::bail!("OS 名称重复: {}", os.name);
            }
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config_toml() {
        let toml_text = r#"
[output]
excel_file = "report.xlsx"

[[os]]
name = "IVI"
dts_file = "DTB/android.dts"

[[os]]
name = "Cluster"
dts_file = "DTB/cluster.dts"
aliases = ["cluster", "linux"]

[rules]
memory_node_names = ["memory"]
"#;
        let cfg: Config = toml::from_str(toml_text).unwrap();
        assert_eq!(cfg.os.len(), 2);
        assert_eq!(cfg.os[1].aliases, vec!["cluster", "linux"]);
        assert_eq!(cfg.output.title, "DTS 资源分析报告");
    }
}
