//! DTS 文本解析器：递归下降实现。
//!
//! 支持 dtc 反编译输出与常见手写 DTS 语法：
//! - `/dts-v1/;` 版本头、`/plugin/;`
//! - `/memreserve/ <addr> <size>;`
//! - `/include/ "file"`（相对当前文件目录解析）
//! - 节点（含 label、`@unit-address`）、属性（字符串 / cell / 字节数组 / 混合）
//! - `&label { ... };` 与 `&{/path} { ... };` 节点覆盖
//! - `/delete-node/` 与 `/delete-property/`
//! - C 风格注释与字符引用 `&phandle`

use std::path::{Path, PathBuf};

use thiserror::Error;

use super::node::DtsNode;
use super::property::{PropItem, Property};
use crate::utils::address::AddressRange;

/// 解析错误。
#[derive(Debug, Error)]
pub enum DtsError {
    #[error("{file}:{line}: {msg}")]
    Syntax { file: String, line: usize, msg: String },
    #[error("读取文件失败 {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("include 嵌套过深: {0}")]
    IncludeDepth(String),
}

/// 解析结果：整棵设备树 + 顶层 memreserve 区段。
#[derive(Debug, Clone)]
pub struct DtsFile {
    pub root: DtsNode,
    pub memreserves: Vec<AddressRange>,
    pub version: Option<String>,
}

// ---------------------------------------------------------------------------
// 词法分析
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// `/dts-v1/` 之类的斜杠关键字。
    Keyword(String),
    /// 标识符 / 节点名 / 属性名。
    Ident(String),
    /// 字符串字面量（转义已解码）。
    Str(String),
    /// 数字字面量。
    Num(u64),
    /// `&label` 或 `&{/path}` 引用。
    Ref(String),
    /// 独立的 `/`（根节点声明）。
    Slash,
    /// `:`（节点标签分隔符）。
    Colon,
    Char(char),
    OpenBrace,
    CloseBrace,
    Semi,
    Eq,
    Comma,
    Lt,
    Gt,
    OpenBracket,
    CloseBracket,
    Eof,
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: usize,
    file: String,
    /// 字节数组 `[..]` 内部：单位十六进制数字按字节解析而非标识符。
    byte_mode: bool,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str, file: &str) -> Self {
        Self { src: src.as_bytes(), pos: 0, line: 1, file: file.to_string(), byte_mode: false }
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T, DtsError> {
        Err(DtsError::Syntax { file: self.file.clone(), line: self.line, msg: msg.into() })
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek_at(&self, off: usize) -> Option<u8> {
        self.src.get(self.pos + off).copied()
    }

    /// 判断 `pos` 处的 `#` 是否为预处理器行标记（`#` 后可跟空格 + 数字）。
    fn is_line_marker(src: &[u8], pos: usize) -> bool {
        let mut i = pos + 1;
        while matches!(src.get(i), Some(b' ') | Some(b'\t')) {
            i += 1;
        }
        src.get(i).map(|c| c.is_ascii_digit()).unwrap_or(false)
    }

    fn bump(&mut self) -> Option<u8> {
        let b = self.peek();
        if b == Some(b'\n') {
            self.line += 1;
        }
        self.pos += 1;
        b
    }

    fn skip_ws_comments(&mut self) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => {
                    self.bump();
                }
                Some(b'/') if self.peek_at(1) == Some(b'*') => {
                    self.bump();
                    self.bump();
                    loop {
                        match self.bump() {
                            Some(b'*') if self.peek() == Some(b'/') => {
                                self.bump();
                                break;
                            }
                            None => break,
                            _ => {}
                        }
                    }
                }
                Some(b'/') if self.peek_at(1) == Some(b'/') => {
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                Some(b'#') if Self::is_line_marker(self.src, self.pos) => {
                    // C 预处理器行标记（cpp 输出的 `# 1 "file.dts"` 等），整行跳过；
                    // `#address-cells` 之类的属性名仍由词法器处理
                    while let Some(b) = self.peek() {
                        if b == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                _ => break,
            }
        }
    }

    fn next_token(&mut self) -> Result<Tok, DtsError> {
        self.skip_ws_comments();
        let Some(b) = self.peek() else { return Ok(Tok::Eof) };
        match b {
            b'{' => { self.bump(); Ok(Tok::OpenBrace) }
            b'}' => { self.bump(); Ok(Tok::CloseBrace) }
            b';' => { self.bump(); Ok(Tok::Semi) }
            b'=' => { self.bump(); Ok(Tok::Eq) }
            b':' => { self.bump(); Ok(Tok::Colon) }
            b',' => { self.bump(); Ok(Tok::Comma) }
            b'<' => { self.bump(); Ok(Tok::Lt) }
            b'>' => { self.bump(); Ok(Tok::Gt) }
            b'[' => { self.bump(); self.byte_mode = true; Ok(Tok::OpenBracket) }
            b']' => { self.bump(); self.byte_mode = false; Ok(Tok::CloseBracket) }
            b'"' => self.lex_string(),
            b'&' => self.lex_ref(),
            b'\'' => self.lex_char(),
            b'/' if self.peek_at(1).map(|c| c.is_ascii_lowercase()).unwrap_or(false) => {
                self.lex_keyword()
            }
            b'/' => {
                // 独立斜杠：根节点 `/ { ... };`
                self.bump();
                Ok(Tok::Slash)
            }
            _ if is_ident_start(b) && !self.byte_mode => self.lex_ident(),
            _ if b.is_ascii_hexdigit() => self.lex_number(),
            other => self.err(format!("unexpected character '{}'", other as char)),
        }
    }

    fn lex_string(&mut self) -> Result<Tok, DtsError> {
        self.bump(); // opening quote
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return self.err("unterminated string"),
                Some(b'"') => break,
                Some(b'\\') => match self.bump() {
                    Some(b'n') => out.push('\n'),
                    Some(b't') => out.push('\t'),
                    Some(b'r') => out.push('\r'),
                    Some(b'0') => out.push('\0'),
                    Some(b'a') => out.push('\u{07}'),
                    Some(b'b') => out.push('\u{08}'),
                    Some(b'f') => out.push('\u{0c}'),
                    Some(b'v') => out.push('\u{0b}'),
                    Some(b'\\') => out.push('\\'),
                    Some(b'"') => out.push('"'),
                    Some(b'\'') => out.push('\''),
                    Some(b'x') => {
                        let mut v = 0u8;
                        let mut n = 0;
                        while n < 2 {
                            match self.peek() {
                                Some(h) if h.is_ascii_hexdigit() => {
                                    v = v * 16 + hex_val(h);
                                    self.bump();
                                    n += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(v as char);
                    }
                    Some(o) if (b'0'..=b'7').contains(&o) => {
                        let mut v = o - b'0';
                        let mut n = 1;
                        while n < 3 {
                            match self.peek() {
                                Some(d) if (b'0'..=b'7').contains(&d) => {
                                    v = v * 8 + (d - b'0');
                                    self.bump();
                                    n += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(v as char);
                    }
                    Some(c) => out.push(c as char),
                    None => return self.err("unterminated escape sequence"),
                },
                Some(c) => out.push(c as char),
            }
        }
        Ok(Tok::Str(out))
    }

    fn lex_ref(&mut self) -> Result<Tok, DtsError> {
        self.bump(); // '&'
        if self.peek() == Some(b'{') {
            self.bump();
            let mut path = String::new();
            loop {
                match self.bump() {
                    Some(b'}') => break,
                    Some(c) => path.push(c as char),
                    None => return self.err("unterminated &{path} reference"),
                }
            }
            Ok(Tok::Ref(path))
        } else {
            let mut label = String::new();
            while let Some(c) = self.peek() {
                if c.is_ascii_alphanumeric() || c == b'_' {
                    label.push(c as char);
                    self.bump();
                } else {
                    break;
                }
            }
            if label.is_empty() {
                return self.err("expected label after '&'");
            }
            Ok(Tok::Ref(label))
        }
    }

    fn lex_char(&mut self) -> Result<Tok, DtsError> {
        self.bump(); // '\''
        let c = match self.bump() {
            Some(b'\\') => match self.bump() {
                Some(b'n') => '\n',
                Some(b't') => '\t',
                Some(b'0') => '\0',
                Some(b'\\') => '\\',
                Some(b'\'') => '\'',
                Some(o) => o as char,
                None => return self.err("unterminated char literal"),
            },
            Some(c) => c as char,
            None => return self.err("unterminated char literal"),
        };
        if self.bump() != Some(b'\'') {
            return self.err("expected closing quote for char literal");
        }
        Ok(Tok::Char(c))
    }

    fn lex_keyword(&mut self) -> Result<Tok, DtsError> {
        self.bump(); // leading '/'
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' {
                name.push(c as char);
                self.bump();
            } else {
                break;
            }
        }
        if self.bump() != Some(b'/') {
            return self.err(format!("malformed keyword /{name}"));
        }
        Ok(Tok::Keyword(name))
    }

    fn lex_ident(&mut self) -> Result<Tok, DtsError> {
        let mut s = String::new();
        while let Some(c) = self.peek() {
            if is_ident_char(c) {
                s.push(c as char);
                self.bump();
            } else {
                break;
            }
        }
        Ok(Tok::Ident(s))
    }

    fn lex_number(&mut self) -> Result<Tok, DtsError> {
        let start = self.pos;
        let mut s = String::new();
        let mut prefixed = false;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                s.push(c as char);
                self.bump();
            } else if (c == b'x' || c == b'X') && s == "0" && self.peek_at(1).map(|h| h.is_ascii_hexdigit()).unwrap_or(false) {
                // 0x 前缀
                prefixed = true;
                self.bump();
            } else if prefixed && c.is_ascii_hexdigit() {
                s.push(c as char);
                self.bump();
            } else if !prefixed && self.byte_mode && c.is_ascii_hexdigit() {
                // 字节数组内的十六进制字节（如 1b / a0 / 0f）
                prefixed = true;
                s.push(c as char);
                self.bump();
            } else if !prefixed && s.starts_with('0') && !s.is_empty() && c.is_ascii_hexdigit() && c.is_ascii_lowercase() {
                // 无前缀十六进制（如 `<0f 00>`，常见于反编译输出的位域值）
                prefixed = true;
                s.push(c as char);
                self.bump();
            } else {
                break;
            }
        }
        // 尺寸后缀 K / M / G
        let mut shift = 0;
        if let Some(c) = self.peek() {
            match c {
                b'k' | b'K' => { shift = 10; self.bump(); }
                b'm' | b'M' => { shift = 20; self.bump(); }
                b'g' | b'G' => { shift = 30; self.bump(); }
                _ => {}
            }
        }
        let value = if prefixed {
            u64::from_str_radix(&s, 16)
        } else {
            s.parse::<u64>()
        };
        match value {
            Ok(v) => Ok(Tok::Num(v.checked_shl(shift).unwrap_or(v))),
            Err(_) => self.err(format!("invalid number literal '{}'",
                std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("?"))),
        }
    }
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'#'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b',' | b'.' | b'+' | b'-' | b'@' | b'#')
}

fn hex_val(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------
// 语法分析
// ---------------------------------------------------------------------------

const MAX_INCLUDE_DEPTH: usize = 16;

struct Parser<'a> {
    lexer: Lexer<'a>,
    peeked: Option<Tok>,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str, file: &str) -> Self {
        Self { lexer: Lexer::new(src, file), peeked: None }
    }

    fn next(&mut self) -> Result<Tok, DtsError> {
        if let Some(t) = self.peeked.take() {
            return Ok(t);
        }
        self.lexer.next_token()
    }

    fn peek(&mut self) -> Result<Tok, DtsError> {
        if self.peeked.is_none() {
            self.peeked = Some(self.lexer.next_token()?);
        }
        Ok(self.peeked.clone().unwrap())
    }

    fn expect(&mut self, expect: Tok) -> Result<(), DtsError> {
        let t = self.next()?;
        if t == expect {
            Ok(())
        } else {
            self.lexer.err(format!("expected {:?}, found {:?}", expect, t))
        }
    }

    fn expect_ident(&mut self) -> Result<String, DtsError> {
        match self.next()? {
            Tok::Ident(s) => Ok(s),
            other => self.lexer.err(format!("expected identifier, found {:?}", other)),
        }
    }

    /// 解析属性值（直到遇到 `;`）。
    fn parse_prop_value(&mut self) -> Result<Vec<PropItem>, DtsError> {
        let mut items = Vec::new();
        loop {
            match self.peek()? {
                Tok::Semi => break,
                Tok::Str(_) => match self.next()? {
                    Tok::Str(s) => items.push(PropItem::Str(s)),
                    _ => unreachable!(),
                },
                Tok::Lt => {
                    self.next()?;
                    let mut cells = Vec::new();
                    loop {
                        match self.next()? {
                            Tok::Gt => break,
                            Tok::Num(n) => cells.push(n as u32),
                            Tok::Char(c) => cells.push(c as u32),
                            Tok::Ref(r) => {
                                // 未解析的 phandle 引用：记录为 0（反编译 DTS 不会出现）
                                log::debug!("cell reference '{}' resolved to 0", r);
                                cells.push(0);
                            }
                            Tok::OpenBracket => {
                                // <...> 内嵌字节数组
                                let bytes = self.parse_byte_array()?;
                                for b in bytes {
                                    cells.push(b as u32);
                                }
                            }
                            Tok::Keyword(k) if k == "bits" => {
                                // /bits/ N：简化处理，读掉位宽数字后继续
                                if let Tok::Num(_) = self.next()? {}
                            }
                            other => {
                                return self.lexer.err(format!(
                                    "unexpected token {:?} in cell array",
                                    other
                                ));
                            }
                        }
                    }
                    items.push(PropItem::Cells(cells));
                }
                Tok::OpenBracket => {
                    self.next()?;
                    items.push(PropItem::Bytes(self.parse_byte_array()?));
                }
                Tok::Ref(r) => {
                    // 裸 phandle 引用（如 aliases 中 `serial0 = &uart0;`），
                    // 等价于单 cell 的 phandle 值；未解析时记为 0
                    self.next()?;
                    log::debug!("bare reference '{}' resolved to 0", r);
                    items.push(PropItem::Cells(vec![0]));
                }
                Tok::Keyword(k) if k == "bits" => {
                    // 属性值顶层的 /bits/ N（如 `prop = /bits/ 8 <...>;`）：
                    // 简化处理，读掉位宽数字，后续 <...> 按普通 cell 解析
                    self.next()?;
                    if let Tok::Num(_) = self.next()? {}
                }
                other => {
                    return self.lexer.err(format!(
                        "unexpected token {:?} in property value",
                        other
                    ));
                }
            }
            if let Tok::Comma = self.peek()? {
                self.next()?;
            }
        }
        Ok(items)
    }

    fn parse_byte_array(&mut self) -> Result<Vec<u8>, DtsError> {
        let mut bytes = Vec::new();
        loop {
            match self.next()? {
                Tok::CloseBracket => break,
                Tok::Num(n) => bytes.push(n as u8),
                Tok::Ref(r) => {
                    log::debug!("byte-array reference '{}' resolved to 0", r);
                    bytes.push(0);
                }
                other => {
                    return self.lexer.err(format!("unexpected token {:?} in byte array", other));
                }
            }
        }
        Ok(bytes)
    }

    /// 解析节点体 `{ ... }`，写入 `node`。
    fn parse_node_body(&mut self, node: &mut DtsNode) -> Result<(), DtsError> {
        let mut pending_labels: Vec<String> = Vec::new();
        loop {
            match self.next()? {
                Tok::CloseBrace => break,
                Tok::Ident(label) => {
                    if let Tok::Colon = self.peek()? {
                        self.next()?;
                        pending_labels.push(label);
                        continue;
                    }
                    self.parse_statement(node, label, &mut pending_labels)?;
                }
                Tok::Keyword(kw) => match kw.as_str() {
                    "delete-node" => {
                        if let Tok::Ident(name) = self.peek()? {
                            self.next()?;
                            node.children.shift_remove(&name);
                            self.expect(Tok::Semi)?;
                        } else {
                            // 节点体内的 /delete-node/; 作用于自身，交由上层处理
                            self.expect(Tok::Semi)?;
                        }
                    }
                    "delete-property" => {
                        let name = self.expect_ident()?;
                        node.properties.shift_remove(&name);
                        self.expect(Tok::Semi)?;
                    }
                    other => {
                        return self.lexer.err(format!(
                            "unexpected keyword /{other}/ inside node"
                        ));
                    }
                },
                Tok::Eof => return self.lexer.err("unexpected end of file in node body"),
                other => {
                    return self.lexer.err(format!("unexpected token {:?} in node body", other));
                }
            }
        }
        Ok(())
    }

    /// 处理节点体内一条语句：属性或子节点（首 token 已读取为 `first`）。
    fn parse_statement(
        &mut self,
        node: &mut DtsNode,
        first: String,
        pending_labels: &mut Vec<String>,
    ) -> Result<(), DtsError> {
        match self.peek()? {
            Tok::Eq => {
                // 属性
                self.next()?;
                let items = self.parse_prop_value()?;
                self.expect(Tok::Semi)?;
                let mut prop = Property::new(first);
                prop.items = items;
                node.properties.insert(prop.name.clone(), prop);
                pending_labels.clear();
            }
            Tok::Semi => {
                // 布尔属性
                self.next()?;
                node.properties.insert(first.clone(), Property::new(first));
                pending_labels.clear();
            }
            Tok::OpenBrace => {
                // 子节点
                self.next()?;
                let mut child = DtsNode::new(first);
                child.labels = std::mem::take(pending_labels);
                self.parse_node_body(&mut child)?;
                self.expect(Tok::Semi)?;
                match node.children.get_mut(&child.name) {
                    Some(existing) => existing.merge(child),
                    None => {
                        node.children.insert(child.name.clone(), child);
                    }
                }
            }
            Tok::Ident(_) => {
                return self.lexer.err(format!(
                    "unexpected token after '{}', expected '=', ';' or '{{'",
                    first
                ));
            }
            other => {
                return self.lexer.err(format!(
                    "unexpected token {:?} after '{}'",
                    other, first
                ));
            }
        }
        Ok(())
    }
}

/// 解析 DTS 文本（`file` 仅用于错误信息与 include 定位）。
pub fn parse_dts_text(src: &str, file: &str) -> Result<DtsFile, DtsError> {
    let mut root = DtsNode::new("/");
    let mut memreserves = Vec::new();
    let mut version = None;
    let dir = Path::new(file).parent().map(|p| p.to_path_buf()).unwrap_or_default();
    parse_file_into(src, file, &dir, &mut root, &mut memreserves, &mut version, 0)?;
    root.assign_paths();
    Ok(DtsFile { root, memreserves, version })
}

/// 从磁盘读取并解析 DTS 文件（递归处理 include）。
pub fn parse_dts_file(path: &Path) -> Result<DtsFile, DtsError> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| DtsError::Io { path: path.to_path_buf(), source: e })?;
    parse_dts_text(&src, &path.display().to_string())
}

fn parse_file_into(
    src: &str,
    file: &str,
    dir: &Path,
    root: &mut DtsNode,
    memreserves: &mut Vec<AddressRange>,
    version: &mut Option<String>,
    depth: usize,
) -> Result<(), DtsError> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(DtsError::IncludeDepth(file.to_string()));
    }
    let mut parser = Parser::new(src, file);
    let mut saw_root = false;
    loop {
        match parser.next()? {
            Tok::Eof => break,
            Tok::Keyword(kw) => match kw.as_str() {
                "dts-v1" => {
                    parser.expect(Tok::Semi)?;
                    *version = Some("dts-v1".to_string());
                }
                "plugin" => {
                    parser.expect(Tok::Semi)?;
                }
                "memreserve" => {
                    let addr = expect_num(&mut parser)?;
                    let size = expect_num(&mut parser)?;
                    parser.expect(Tok::Semi)?;
                    memreserves.push(AddressRange::new(addr, size));
                }
                "include" => {
                    let inc = match parser.next()? {
                        Tok::Str(s) => s,
                        other => {
                            return parser.lexer.err(format!(
                                "expected string after /include/, found {:?}",
                                other
                            ));
                        }
                    };
                    let inc_path = dir.join(&inc);
                    log::debug!("including {}", inc_path.display());
                    let inc_src = std::fs::read_to_string(&inc_path).map_err(|e| {
                        DtsError::Io { path: inc_path.clone(), source: e }
                    })?;
                    let inc_dir = inc_path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                    parse_file_into(
                        &inc_src,
                        &inc_path.display().to_string(),
                        &inc_dir,
                        root,
                        memreserves,
                        version,
                        depth + 1,
                    )?;
                }
                other => {
                    return parser
                        .lexer
                        .err(format!("unexpected top-level keyword /{other}/"));
                }
            },
            Tok::OpenBrace => {
                // 匿名根扩展 `{ ... };`
                parser.parse_node_body(root)?;
                parser.expect(Tok::Semi)?;
                saw_root = true;
            }
            Tok::Slash => {
                // 根节点声明 `/ { ... };`
                match parser.peek()? {
                    Tok::OpenBrace => {
                        parser.next()?;
                        parser.parse_node_body(root)?;
                        parser.expect(Tok::Semi)?;
                        saw_root = true;
                    }
                    other => {
                        return parser.lexer.err(format!(
                            "expected '{{' after '/', found {:?}",
                            other
                        ));
                    }
                }
            }
            Tok::Ref(r) => {
                // `&label { ... };` 节点覆盖：在反编译输出中罕见，忽略内容
                match parser.peek()? {
                    Tok::OpenBrace => {
                        parser.next()?;
                        let mut scratch = DtsNode::new(format!("&{{{}}}", r));
                        parser.parse_node_body(&mut scratch)?;
                        parser.expect(Tok::Semi)?;
                        apply_override(root, &r, scratch);
                    }
                    other => {
                        return parser.lexer.err(format!(
                            "expected '{{' after '&{}', found {:?}",
                            r, other
                        ));
                    }
                }
            }
            other => {
                return parser
                    .lexer
                    .err(format!("unexpected top-level token {:?}", other));
            }
        }
    }
    if !saw_root && depth == 0 && version.is_some() && root.children.is_empty() {
        log::warn!("no root node found in {}", file);
    }
    Ok(())
}

fn expect_num(parser: &mut Parser) -> Result<u64, DtsError> {
    match parser.next()? {
        Tok::Num(n) => Ok(n),
        other => parser.lexer.err(format!("expected number, found {:?}", other)),
    }
}

/// 应用 `&label { ... }` / `&{/path} { ... }` 覆盖。
fn apply_override(root: &mut DtsNode, target: &str, patch: DtsNode) {
    if target.starts_with('/') {
        if let Some(node) = find_path_mut(root, target) {
            node.merge(patch);
        } else {
            log::debug!("override target '{}' not found, ignored", target);
        }
    } else if let Some(node) = find_label_mut(root, target) {
        node.merge(patch);
    } else {
        log::debug!("override target label '{}' not found, ignored", target);
    }
}

/// 按标签递归查找节点（可变）。
fn find_label_mut<'a>(node: &'a mut DtsNode, label: &str) -> Option<&'a mut DtsNode> {
    if node.labels.iter().any(|l| l == label) {
        return Some(node);
    }
    for child in node.children.values_mut() {
        if let Some(n) = find_label_mut(child, label) {
            return Some(n);
        }
    }
    None
}

fn find_path_mut<'a>(root: &'a mut DtsNode, path: &str) -> Option<&'a mut DtsNode> {
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        return Some(root);
    }
    let mut cur = root;
    for seg in path.split('/') {
        if seg.is_empty() {
            continue;
        }
        cur = cur.children.get_mut(seg)?;
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let dts = r#"
/dts-v1/;
/ {
    #address-cells = <0x02>;
    #size-cells = <0x02>;
    model = "test board";

    memory@0 {
        device_type = "memory";
        reg = <0x00 0x80000000 0x00 0x40000000>;
    };

    reserved-memory {
        #address-cells = <0x02>;
        #size-cells = <0x02>;
        ranges;

        pool@90000000 {
            compatible = "shared-dma-pool";
            reusable;
            reg = <0x00 0x90000000 0x00 0x10000000>;
        };
    };
};
"#;
        let file = parse_dts_text(dts, "test.dts").unwrap();
        assert_eq!(file.version.as_deref(), Some("dts-v1"));
        let mem = file.root.find_path("/memory@0").unwrap();
        assert_eq!(mem.get_property("device_type").unwrap().as_string().as_deref(), Some("memory"));
        let cells = mem.get_property("reg").unwrap().as_cells().unwrap();
        assert_eq!(cells, vec![0, 0x8000_0000, 0, 0x4000_0000]);
        let pool = file.root.find_path("/reserved-memory/pool@90000000").unwrap();
        assert!(pool.has_property("reusable"));
        assert_eq!(pool.compatibles(), vec!["shared-dma-pool"]);
    }

    #[test]
    fn test_parse_bare_ref_and_bits() {
        // 裸 phandle 引用（aliases）与属性值顶层的 /bits/ N
        let dts = r#"
/dts-v1/;
/ {
    aliases {
        serial0 = &uart0;
    };
    uart0: serial@1000 {
        reg = <0x1000 0x100>;
        lut = /bits/ 8 <0x01 0x02 0x03>;
    };
};
"#;
        let file = parse_dts_text(dts, "test.dts").unwrap();
        let aliases = file.root.find_path("/aliases").unwrap();
        // 裸引用解析为单 cell（未解析 phandle 记 0）
        assert_eq!(aliases.get_property("serial0").unwrap().as_cells().unwrap(), vec![0]);
        let uart = file.root.find_path("/serial@1000").unwrap();
        assert_eq!(uart.get_property("lut").unwrap().as_cells().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn test_parse_labels_and_override() {
        let dts = r#"
/dts-v1/;
/ {
    soc {
        uart0: serial@1000 {
            status = "disabled";
        };
    };
};
&uart0 {
    status = "okay";
};
"#;
        let file = parse_dts_text(dts, "test.dts").unwrap();
        let uart = file.root.find_path("/soc/serial@1000").unwrap();
        assert_eq!(uart.get_property("status").unwrap().as_string().as_deref(), Some("okay"));
    }

    #[test]
    fn test_parse_memreserve_and_comments() {
        let dts = r#"
/dts-v1/;
/memreserve/ 0x80000000 0x00100000;
/* block comment
   spanning lines */
/ {
    // line comment
    compatible = "x\0y";
};
"#;
        let file = parse_dts_text(dts, "test.dts").unwrap();
        assert_eq!(file.memreserves.len(), 1);
        assert_eq!(file.memreserves[0].start, 0x8000_0000);
        assert_eq!(file.memreserves[0].size, 0x0010_0000);
        assert_eq!(file.root.compatibles(), vec!["x", "y"]);
    }

    #[test]
    fn test_parse_delete_property() {
        let dts = r#"
/dts-v1/;
/ {
    foo {
        a = <1>;
        /delete-property/ a;
        b;
    };
};
"#;
        let file = parse_dts_text(dts, "test.dts").unwrap();
        let foo = file.root.find_path("/foo").unwrap();
        assert!(!foo.has_property("a"));
        assert!(foo.has_property("b"));
    }
}
