<div align="center">
  <img src="src/assets/icon.svg" alt="WEave Logo" width="120" height="120">
  
  # WEave
  
  **用于管理 Wallpaper Engine 的 Steam 创意工坊壁纸的现代桌面应用**
  
  [![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg?logo=opensourceinitiative&logoColor=white)](LICENSE)
  [![Platform](https://img.shields.io/badge/Platform-Windows%2010%2F11-0078D6?logo=data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHZpZXdCb3g9IjAgMCAyNCAyNCIgZmlsbD0id2hpdGUiPjxwYXRoIGQ9Ik0wIDMuNDQ5TDkuNzUgMi4xdjkuNDVIMFYzLjQ0OXpNMCAxMi40NWg5Ljc1djkuNDVMMCAyMC41NTF2LTguMTAxek0xMS4yNSAxLjg5bDEyLjc1LTEuODl2MTEuNDVoLTEyLjc1VjEuODl6bTEyLjc1IDEwLjU2djExLjQ1bC0xMi43NS0xLjg5di05LjU2aDEyLjc1eiIvPjwvc3ZnPg==)](https://www.microsoft.com/windows)
  [![Tauri](https://img.shields.io/badge/Tauri-v2-FFC131?logo=tauri&logoColor=white)](https://tauri.app)
  [![Rust](https://img.shields.io/badge/Rust-%3E%3D1.77-black?logo=rust&logoColor=white)](https://www.rust-lang.org)
  [![React](https://img.shields.io/badge/React-v19-61DAFB?logo=react&logoColor=black)](https://react.dev)
  [![TypeScript](https://img.shields.io/badge/TypeScript-v6-3178C6?logo=typescript&logoColor=white)](https://www.typescriptlang.org)
  [![Tailwind CSS](https://img.shields.io/badge/Tailwind_CSS-v4-38BDF8?logo=tailwindcss&logoColor=white)](https://tailwindcss.com)
  [![Vite](https://img.shields.io/badge/Vite-v8-646CFF?logo=vite&logoColor=white)](https://vite.dev)
  [![Node.js](https://img.shields.io/badge/Node.js-%3E%3D20-339933?logo=node.js&logoColor=white)](https://nodejs.org)
  
  [English](README.en.md) · [Русская версия](README.ru.md)
</div>

---

## 📖 简介

WEave 是一款基于 Tauri 2 与 React 构建的强大 Wallpaper Engine 创意工坊管理器。无需打开 Steam 或浏览器，即可发现、下载并管理创意工坊里的海量壁纸。

https://github.com/user-attachments/assets/45bf371d-6b76-4ecc-93d5-9bc8d95c467a

<div align="center">
  <img src="https://github.com/user-attachments/assets/084d0710-4fb4-492c-a2eb-b9e1941a89bf" width="49%"> <img src="https://github.com/user-attachments/assets/5cd7ba5b-bb4e-4a87-be55-b4b74143cf94" width="49%">
  <img src="https://github.com/user-attachments/assets/21eb0771-b350-47e0-8057-ccbcc774c83f" width="49%"> <img src="https://github.com/user-attachments/assets/1042d7d8-8f5b-4c7a-9097-2d0d7319876f" width="49%">
</div>

---

## ✨ 主要特性

* **按指定条件搜索创意工坊壁纸**
* **一键安装壁纸**
* **管理已安装的壁纸**

*具体包括：*

<details>
<summary><b>创意工坊浏览</b></summary>

- **Steam 创意工坊集成**：复刻标准 Steam 创意工坊网页界面的功能，包括基础搜索、排序与筛选（按分类、年龄分级、分辨率等）。
- **作者主页**：查看指定作者发布的全部壁纸与合集。
- **合集**：支持浏览 Steam 合集及其内容。
- **页面预加载**：提前抓取下一页条目，翻页更顺畅。
- **描述翻译**：壁纸描述自动翻译（Google Translate API）。

</details>

<details>
<summary><b>下载管理</b></summary>

- **Steam 账号支持**：添加你自己的 Steam 账号，一次登录同时用于浏览创意工坊和下载（支持 Steam 令牌）。
- **进度跟踪**：实时显示下载进度（已下载量），并可取消下载。
- **自动应用**：可选功能，自动应用最新下载完成的壁纸。

</details>

<details>
<summary><b>库管理</b></summary>

- **应用与删除**：快速应用（激活）桌面壁纸，或将其删除。
- **参数与预设**：实时调整壁纸属性（播放速度、对齐方式、音量、色彩设置、亮度、对比度等），并可直接保存到 `project.json`（自动创建备份）。
- **批量操作**：多选模式，可批量删除或提取已安装的壁纸。
- **RePKG 解包器**：一键解包 `.pkg` 文件。

</details>

<details>
<summary><b>Wallpaper Engine 集成</b></summary>

- **自动检测**：自动探测你的 Wallpaper Engine 安装目录。
- **控制中心**：集成媒体控制栏，可播放、暂停、静音、切换壁纸，并切换桌面图标显示状态。
- **播放列表与配置方案**：在控制栏中按名称快速加载 Wallpaper Engine 的播放列表或设置方案。

</details>

<details>
<summary><b>个性化与快捷键</b></summary>

- **界面主题**：6 套界面主题与 10 种自定义强调色。
- **全局快捷键**：可自定义键盘快捷键，用于控制播放、切换页面与打开视图。
- **多语言**：界面提供英语、俄语与简体中文——首次启动时自动检测操作系统语言（无法识别时回落英语），并可随时在「设置 → 通用」中切换。

</details>

*以及其他未在此列举的功能*

---

## 🚀 安装

### 终端用户

#### 前置要求
- **Windows 10/11**（x64）
- **Wallpaper Engine**（已安装）
- **.NET 运行时 8/9/10**（缺失时自动下载）

安装时，请从 [**GitHub Releases**](https://github.com/psyattack/weave/releases) 下载最新版本：
- **安装包（`.exe .msi`）：** 运行安装程序并按提示操作。
- **便携版（`.zip`）：** 将压缩包解压到磁盘上任意方便的位置，运行 `WEave.exe`。

> [!NOTE]
> 首次登录时，若系统中未检测到 .NET 运行时/SDK 8/9/10，WEave 会自动下载附加工具以及便携版 .NET 运行时 9.0.17。

---

### 开发者

<details>
<summary><b>开发环境搭建</b></summary>

#### 前置要求
- **Node.js**（v20+）
- **Rust**（v1.77+）
- **.NET 运行时/SDK 8/9/10**
- **Wallpaper Engine**

#### 搭建与其他选项

```bash
# 克隆仓库
git clone https://github.com/psyattack/WEave.git
cd WEave

# 安装依赖
npm install

# 开发模式运行
npm run tauri dev

# 构建生产版本
npm run tauri build

# 运行测试
npm run test  # 前端测试
cd src-tauri && cargo test  # 后端测试

# 检查后端
cargo check

# 检查前端
npm run typecheck
npm run lint

# 版本号提升
npm run bump
```

</details>

---

## 🛠️ 技术栈

<table>
<tr>
<td width="50%">

### 前端
- **React 19** + TypeScript
- **Tauri 2**（桌面框架）
- **Vite**（构建工具）
- **TailwindCSS v4**（样式）
- **Framer Motion**（动画，支持 reduced motion）
- **Radix UI**（组件）
- **Zustand**（状态管理）
- **类型安全 i18n**（自研系统）
- **Lucide React**（图标）
- **Vitest**（单元、集成与 E2E 测试，含自研 Tauri mock）

</td>
<td width="50%">

### 后端
- **Rust**（Tauri 后端）
- **Tokio**（异步运行时）
- **Reqwest**（HTTP 客户端）
- **Scraper**（HTML 解析）
- **zip + unrar**（压缩包解包）
- **AES-GCM + PBKDF2**（加密）
- **Serde**（序列化）
- **dirs**（平台目录）
- **log + env_logger**（日志）

</td>
</tr>
</table>

---

## 📂 项目结构

<details>
<summary><b>展开结构</b></summary>

```
WEave/
├── src/                              # React 前端
│   ├── assets/                       # 静态资源（Logo、图标）
│   ├── components/                   # React 组件
│   │   ├── common/                   # 可复用 UI 组件（Select、ToastStack、SetupOverlay 等）
│   │   │   ├── DetailsPanel.tsx      # 壁纸详情主面板
│   │   │   └── details/              # 详情面板子组件（Actions、MetaGrid、Sidebar、Presets）
│   │   ├── dialogs/                  # 弹窗与对话框（法律声明、更新、登录/两步验证、批量下载等）
│   │   ├── installed/                # 已安装壁纸组件（Grid、SelectionBar、Toolbar、WallpaperCard）
│   │   ├── layout/                   # 应用布局组件（Sidebar、TitleBar、TopBar）
│   │   ├── settings/                 # 设置组件（通用、账号、快捷键、设置对话框）
│   │   ├── tasks/                    # 任务抽屉组件（TasksDrawer）
│   │   ├── views/                    # 视图/页面（创意工坊、合集、已安装、作者）
│   │   └── workshop/                 # 创意工坊组件（Card、FilterBar、Pagination）
│   ├── e2e/                          # UI 与集成测试（Vitest + Tauri mock）
│   ├── hooks/                        # React hooks（useWallpaperActions、useTheme、useBootstrap、useHotkeys 等）
│   ├── i18n/                         # 类型安全 i18n 系统
│   │   ├── languages.ts              # 唯一事实源：支持的语言注册表
│   │   ├── system-language.ts        # 纯函数：操作系统/浏览器语言检测与归一化
│   │   └── locales/                  # 翻译源文件（en.ts、ru.ts、zh.ts）
│   ├── lib/                          # 工具函数（errors、logger、tauri、tauri-mock、workshop）
│   ├── stores/                       # Zustand 状态仓库（app、dotnet、filters、hotkeys/ 等）
│   └── types/                        # TypeScript 类型（workshop.ts）
│
└── src-tauri/                        # Rust 后端（Tauri）
    ├── capabilities/                 # Tauri v2 权限与安全配置
    └── src/                          # 后端源码
        ├── commands/                 # Tauri 命令（由前端调用）
        │   ├── accounts.rs           # 账号管理命令
        │   ├── download.rs           # 下载编排命令
        │   ├── extract.rs            # 解包命令（RePKG）
        │   ├── steam.rs              # Steam 登录、Cookie 与 webview 命令
        │   ├── we.rs                 # Wallpaper Engine 命令（安装、应用、控制）
        │   └── ...                   # 元数据、配置、i18n、自动更新等命令
        ├── config/                   # 配置管理
        │   ├── settings.rs           # 用户设置服务（JSON）
        │   └── metadata.rs           # 元数据缓存服务
        ├── core/                     # 核心应用逻辑
        │   ├── app_state.rs          # 全局应用状态（AppState）
        │   ├── errors.rs             # 结构化错误类型与格式化
        │   ├── logger.rs             # 滚动文件日志初始化
        │   └── runtime.rs            # 便携式 .NET 运行时管理器
        ├── plugins/                  # 外部工具插件
        │   ├── plugin_manager.rs     # 插件下载与更新逻辑
        │   └── plugin_paths.rs       # 外部可执行文件路径解析
        ├── services/                 # 业务逻辑服务
        │   ├── accounts/             # 加密的 Steam 账号存储服务
        │   ├── workshop/             # Steam 创意工坊抓取、解析与 webview 处理
        │   ├── download.rs           # DepotDownloader 封装服务
        │   ├── extract.rs            # RePKG 解包器封装服务
        │   ├── we_client.rs          # Wallpaper Engine API 与显示器集成服务
        │   └── ...                   # i18n、translator、metadata 等服务
        ├── lib.rs                    # Tauri context 与 builder 初始化
        └── main.rs                   # 可执行入口
```

</details>

---

## 📝 配置

配置文件与插件存放于：
**`%LOCALAPPDATA%\WEave\`**

| 文件 / 目录 | 说明 |
|------|-------------|
| `settings.json` | 应用设置 |
| `metadata.json` | 壁纸元数据缓存 |
| `user_accounts.enc` | 加密的 Steam 账号 |
| `SteamWebView/` | Steam 解析器数据 |
| `EBWebView/` | 应用数据 |
| `plugins/` | 自动下载的插件（DepotDownloaderMod、RePKG） |
| `dotnet/` | 便携版 .NET 运行时 9.0.17（如需要） |
| `.log` | 应用日志文件 |

---

## 🤝 参与贡献

欢迎贡献！请随时提交 Pull Request。

---

## 📄 许可证

本项目基于 [MIT License](LICENSE) 授权。完整的第三方声明与组件许可证见 [NOTICE.md](NOTICE.md)。

---

## 🙏 致谢

- **下载工具：** [DepotDownloaderMod](https://github.com/SteamAutoCracks/DepotDownloaderMod)
- **解包工具：** [RePKG](https://github.com/notscuffed/repkg)

---

## ⚠️ 免责声明

本应用**与 Valve Corporation 或 Wallpaper Engine 无任何关联，也未获其认可**。Steam 与 Wallpaper Engine 是各自所有者的商标。

---

## 💬 支持

如有问题、疑问或功能建议，请在 [**GitHub**](https://github.com/psyattack/weave/issues) 上提交 issue。

---
