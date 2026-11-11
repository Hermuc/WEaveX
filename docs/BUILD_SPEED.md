# 构建提速指南

> 本文档记录本机实测的构建耗时与优化方案。数据来自 2026-11-11 实测，不是估算。

## 一、耗时构成（实测）

| 阶段 | 耗时 | 占比 |
|---|---|---|
| 前端 `vite build` | 6 s | 9% |
| Rust `cargo check`（只类型检查） | 4 s | 6% |
| Rust `cargo build --release`（codegen + 链接） | **58 s** | **85%** |
| NSIS 打包 | ~7 s | 10% |
| **完整 `tauri build` 合计** | **~75 s** | 100% |

**结论：85% 的时间在 Rust 的 codegen 与链接上，前端只有 6 秒，不需要优化前端。**

## 二、日常开发怎么用（推荐）

改完代码后，按需要选一条：

```bash
# ① 只确认能不能编译 —— 4 秒（最高频）
npm run check:rust

# ② 前端 + Rust 全套检查 —— 约 15 秒
npm run check:all

# ③ 要看界面效果 / 调试 —— 改前端热更新 0 秒
npm run dev:app

# ④ 要一个能跑的 exe 但不需要安装包 —— 约 65 秒
npm run build:fast

# ⑤ 出正式安装包（交付用）—— 约 75 秒
npm run build:installer
```

**核心认知**：开发阶段绝大多数改动只需要 ① 就能确认正确性，不必每次都跑 ⑤。

## 三、profile 说明

`src-tauri/Cargo.toml` 中新增：

```toml
[profile.dev]
debug = false        # 不生成调试符号，省约 30% 构建时间
incremental = true

[profile.release-fast]
inherits = "release"
lto = false          # 跳过 LTO 全程序优化
codegen-units = 16
```

需要"快但仍是优化版"的二进制时：

```bash
cd src-tauri && cargo build --profile release-fast
```

## 四、已排除的"错误优化"（别再试）

以下每一项都做了 A/B 对照实测，**均无效或更差**：

| 假设 | 实测 | 判定 |
|---|---|---|
| `crate-type` 三种产物太耗时 | 只留 `rlib` → 58 s（原 58 s） | ❌ 无效 |
| `opt-level=3` 太激进 | 改 `opt-level=1` → **64 s（更慢）** | ❌ 更差 |
| release 没开增量 | `incremental=true` → 58 s | ❌ 无效 |
| 依赖需要重编 | 只编译 **2 个单元**，依赖全复用 | ❌ 无此事 |
| 链接太慢 | 只 codegen 不链接 → **137 s（更慢）** | ❌ 相反 |
| NSIS 打包太慢 | `--no-bundle` → 65 s（省 7 s） | ⚠️ 影响小 |

## 五、进阶方案（按性价比）

### 换更快的链接器（可选，收益 5-10 秒）

```bash
rustup component add llvm-tools
```

``.cargo/config.toml`：

```toml
[target.x86_64-pc-windows-msvc]
linker = "rust-lld.exe"
```

注意：本机实测链接不是主要瓶颈（codegen 才是），收益有限。

### 拆分 crate（工程量大，长期收益高）

把 `services/workshop`、`services/download`、`commands` 拆成独立 crate。
改 `webview.rs` 时只重编那一个 crate，其余复用。

- 代价：需重构模块边界，约 2-3 天
- 收益：增量编译可能降到 10-15 秒

### 清理 target 目录（谨慎）

`src-tauri/target` 已达 **12 GB**（release 2.4 GB + debug 8.9 GB）。

```bash
# 只清 debug 产物，保留 release 缓存（避免全量重编）
cargo clean --profile dev --manifest-path src-tauri/Cargo.toml
```

**注意**：`cargo clean` 会删除全部缓存，下次全量编译需 5-8 分钟。除非磁盘告急，否则不建议。
