# dts-analyzer

多 OS Device Tree Source (DTS) 资源分配分析工具。

解析多个 OS 的 DTS 文件（如 `dtc` 反编译输出），提取 CPU、系统内存、保留内存、
外设、中断等资源，识别资源在 OS 之间的**分配、共享与冲突**，并生成 Excel 报告。

## 功能特性

- **DTS 解析**：内置手写递归下降解析器，支持节点/属性/cell 数组/字节数组、
  label、节点合并、`&label { }` 覆盖、`/delete-node/`、`/delete-property/`、
  `/memreserve/`、`/include/` 等 DTS 语法要素。
- **资源提取**：
  - CPU（按 `cpus` 节点 `reg` 解析 MPIDR，跨 OS 去重编号）
  - 系统内存（`memory` 节点，支持 64 位地址）
  - 保留内存（`reserved-memory`，识别 `no-map` / `shared-dma-pool` / `status`）
  - 外设（按节点名与 compatible 分类：UART / I2C / SPI / Ethernet / Storage /
    Display / Camera / Video / Audio / GPU / IOMMU / Watchdog 等 22 类）
  - 中断（GIC 三元组解析，换算 Linux IRQ 号）
- **共享识别**：
  - 同名同址保留区出现在多个 OS（如 `msi_reserved*`、`gdc_shmem`）
  - `gipc_<src>_<dst>_<idx>` 命名的 IPC 共享内存（自动解析对端 OS）
  - 子系统 `shared-memory` 节点（如 sdd/ADSP 音频共享区，多 core 切片自动合并）
  - 系统内存区间跨 OS 重叠
- **冲突检测**：CPU 重复分配、同一中断号被多 OS 外设使用、同一外设被多 OS 同时使能、
  保留内存区间重叠。
- **Excel 输出**：基于 `rust_xlsxwriter` 生成带样式（配色 / 边框 / 合并单元格 /
  冻结窗格）的 6 个 Sheet。

## 构建

要求 Rust 2021 工具链：

```bash
cd dts-analyzer
cargo build          # 或 cargo build --release
cargo test           # 运行单元测试
```

## 使用

```bash
dts-analyzer --config <配置文件> [--output <Excel 路径>] [--log-level <级别>] [--verbose]
```

| 参数 | 缩写 | 说明 |
| --- | --- | --- |
| `--config` | `-c` | TOML 配置文件路径，默认 `config.toml` |
| `--output` | `-o` | 覆盖配置文件中的输出 Excel 路径 |
| `--log-level` | `-l` | 日志级别：debug/info/warn/error，默认 info |
| `--verbose` | `-v` | 详细输出（等价于 `--log-level debug`） |

示例：

```bash
# 首次使用：复制配置模板并按实际平台修改
cp config.example.toml config.toml
./target/debug/dts-analyzer --config config.toml
```

运行结束会打印统计摘要（OS 数量、CPU 总数、共享/冲突条目数）及生成的 Excel 路径。

## 配置文件格式

仓库提供通用模板 [config.example.toml](config.example.toml)，复制为 `config.toml`
后按实际平台修改（`config.toml` 已加入 .gitignore，不会被提交）：

```toml
[output]
excel_file = "resources_list.xlsx"   # 输出 Excel 路径
title = "多 OS 资源分配分析"          # 报告标题（可选）

[[os]]                      # 每个 OS 一个条目
name = "OS-A"               # OS 名称（报告中显示）
dts_file = "path/to/os-a.dts"  # DTS 文件路径（相对路径基于配置文件所在目录）
aliases = ["os-a"]          # 短名（用于 gipc_<a>_<b> 对端匹配，可选）

[[os]]
name = "OS-B"
dts_file = "path/to/os-b.dts"

[rules]                     # 可选解析规则
memory_node_names = ["memory"]                    # 系统内存节点名
shared_keywords = ["gipc", "shm", "shmem", "shared", "ipc", "mailbox"]
```

约束：至少一个 `[[os]]`；OS 名称不可重复；DTS 文件必须存在。

## 输出 Excel 内容

| Sheet | 内容 |
| --- | --- |
| 总览 (Overview) | 各 OS 资源统计与全局摘要（CPU 去重总数、共享/冲突条目数等） |
| 资源分配矩阵 | Resources × OS 矩阵：CPU（按 MPIDR 编号）、Memory、各类外设归属（√/×） |
| 内存分配矩阵 | 系统内存 + 保留内存明细：起止地址、大小、类型、节点名、OS、备注 |
| 共享资源清单 | 跨 OS 共享项：内存重叠、保留内存、gipc IPC 区、子系统共享区、外设、中断 |
| 外设分配 | 按类别分组的外设清单：基地址、IRQ、各 OS 状态（√/disabled/-）、compatible |
| 冲突报告 | CPU/中断/外设/内存冲突，附描述与处理建议 |

## 代码结构

```
dts-analyzer/
├── src/
│   ├── main.rs          # CLI 入口（clap + fern 日志）
│   ├── lib.rs           # 业务入口：加载配置 → 分析 → 导出
│   ├── config.rs        # TOML 配置解析与校验
│   ├── dts/             # DTS 解析器（词法/语法/节点/属性模型）
│   ├── analyzer/        # 资源提取与共享/冲突分析
│   ├── export/          # Excel 导出（rust_xlsxwriter，6 Sheet 布局）
│   └── utils/           # 地址范围等工具
├── config.example.toml  # 配置文件模板
└── README.md
```

## 已知限制

- 外设收集限于根节点下深度 ≤ 2 且带 `reg` 属性的节点；总线桥下挂的
  深层子节点（如 i2c 从设备）不单独列出。
- 中断冲突判定基于 Linux IRQ 号相等，不区分 hypervisor 层的中断路由策略。
- 外设分类基于节点名/compatible 关键字启发式匹配，未覆盖的归入 Other。
