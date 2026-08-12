//! DTS 节点数据结构。

use indexmap::IndexMap;

use super::property::Property;

/// DTS 节点。
#[derive(Debug, Clone, Default)]
pub struct DtsNode {
    /// 节点名（如 `serial@270a0000`，根节点为 `/`）。
    pub name: String,
    /// 完整路径（解析完成后填充，如 `/soc/serial@270a0000`）。
    pub path: String,
    /// 节点标签（`label: node`）。
    pub labels: Vec<String>,
    /// 属性表（保持声明顺序）。
    pub properties: IndexMap<String, Property>,
    /// 子节点表（保持声明顺序）。
    pub children: IndexMap<String, DtsNode>,
}

impl DtsNode {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), ..Default::default() }
    }

    /// 节点名中 `@` 之后的 unit-address 部分。
    pub fn unit_address(&self) -> Option<&str> {
        self.name.split_once('@').map(|(_, u)| u)
    }

    /// 节点名中 `@` 之前的部分。
    pub fn base_name(&self) -> &str {
        self.name.split_once('@').map(|(b, _)| b).unwrap_or(&self.name)
    }

    pub fn get_property(&self, name: &str) -> Option<&Property> {
        self.properties.get(name)
    }

    pub fn has_property(&self, name: &str) -> bool {
        self.properties.contains_key(name)
    }

    /// 属性 status 是否为使能状态（缺省视为 "okay"）。
    pub fn is_enabled(&self) -> bool {
        match self.get_property("status").and_then(|p| p.as_string()) {
            Some(s) => s == "okay" || s == "ok",
            None => true,
        }
    }

    /// compatible 字符串列表。
    pub fn compatibles(&self) -> Vec<String> {
        self.get_property("compatible").map(|p| p.as_strings()).unwrap_or_default()
    }

    /// 将另一棵同名子树合并进当前节点（属性覆盖、子节点递归合并）。
    pub fn merge(&mut self, other: DtsNode) {
        self.labels.extend(other.labels);
        for (k, v) in other.properties {
            self.properties.insert(k, v);
        }
        for (name, child) in other.children {
            match self.children.get_mut(&name) {
                Some(existing) => existing.merge(child),
                None => {
                    self.children.insert(name, child);
                }
            }
        }
    }

    /// 深度优先遍历全部节点（含自身），回调接收节点可变引用。
    pub fn walk_mut(&mut self, f: &mut dyn FnMut(&mut DtsNode)) {
        f(self);
        for child in self.children.values_mut() {
            child.walk_mut(f);
        }
    }

    /// 深度优先遍历全部节点（只读）。
    pub fn walk(&self, f: &mut dyn FnMut(&DtsNode)) {
        f(self);
        for child in self.children.values() {
            child.walk(f);
        }
    }

    /// 根据路径查找节点（如 `/cpus/cpu@0`）。
    pub fn find_path(&self, path: &str) -> Option<&DtsNode> {
        let path = path.trim_start_matches('/');
        if path.is_empty() {
            return Some(self);
        }
        let mut cur = self;
        for seg in path.split('/') {
            if seg.is_empty() {
                continue;
            }
            cur = cur.children.get(seg)?;
        }
        Some(cur)
    }

    /// 为整棵树填充 path 字段。
    pub fn assign_paths(&mut self) {
        fn rec(node: &mut DtsNode, prefix: &str) {
            node.path = if prefix.is_empty() || node.name == "/" {
                node.name.clone()
            } else {
                format!("{}/{}", prefix, node.name)
            };
            let p = node.path.clone();
            for child in node.children.values_mut() {
                rec(child, &p);
            }
        }
        rec(self, "");
    }

    /// 统计全部节点数量。
    pub fn count_nodes(&self) -> usize {
        let mut n = 0;
        self.walk(&mut |_| n += 1);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_names() {
        let node = DtsNode::new("serial@270a0000");
        assert_eq!(node.base_name(), "serial");
        assert_eq!(node.unit_address(), Some("270a0000"));
        let plain = DtsNode::new("cpus");
        assert_eq!(plain.base_name(), "cpus");
        assert_eq!(plain.unit_address(), None);
    }

    #[test]
    fn test_status_enabled() {
        let mut node = DtsNode::new("x");
        assert!(node.is_enabled());
        node.properties.insert(
            "status".into(),
            Property {
                name: "status".into(),
                items: vec![super::super::property::PropItem::Str("disabled".into())],
            },
        );
        assert!(!node.is_enabled());
    }
}
