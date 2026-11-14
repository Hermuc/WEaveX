# 启动与视图切换时的「透明框」问题：完整排查记录

> 本文记录一次持续多轮的疑难 bug 排查过程，包括走过的弯路、错误的假设、以及最终定位到的机制。
> **写作目的**：让后来的人不必重走这些弯路，并且不会把已经排除的方向当成「还没查过」。

最后更新：2026-11-14

---

## 一、现象

两类几乎相同的表现，但触发时机不同：

| 编号 | 触发时机 | 表现 |
|---|---|---|
| **A** | 冷启动，双击 exe 后约 0.5–3 秒 | 整个主窗口区域出现一层半透明矩形，随后消失 |
| **B** | **首次**切换到「已安装」页 | 同样的一层半透明矩形，闪现后消失；第二次及以后切换 **不再出现** |

用户的关键观察（对定位起了决定性作用）：

- A 和 B 的视觉表现几乎一致；
- B **只有第一次**切换出现，之后不再出现。

历史背景：更早的一个版本里，「已安装」页每次切换都会闪**两个**透明框，
那个问题当时已按「同步命令阻塞主线程」的思路修掉了（见第四节）。

---

## 二、最重要的结论（先看这个）

### 2.1 透明框 = 窗口有一帧没有内容可合成

主线程被占用 → 窗口无法重绘 → DWM 合成时拿不到新帧 → 露出下一层（桌面/父窗口）
→ 视觉上就是一层半透明矩形。

**凡是能在主线程上造成「较长同步阻塞」的东西，都能产生这个现象。**

### 2.2 本机的两个已知元凶

| 元凶 | 阻塞原因 | 修复方式 | 状态 |
|---|---|---|---|
| **dotnet --list-runtimes 子进程** | Windows 创建进程时会短暂阻塞**父进程的所有线程**（加载器持有进程级锁） | 不再创建子进程，改为纯文件系统探测 | ✅ 已修 |
| **we_list_installed 扫盘时持有客户端读锁** | 扫盘数秒，全程持 RwLock 读锁；所有同步 we_* 命令跑在主线程，全部卡在该锁上 | **未完成**（见第五节） | ❌ 待修 |

### 2.3 一个必须记住的陷阱

**spawn_blocking 解决不了子进程创建导致的卡顿。**

```rust
// 这样写没用：卡顿发生在进程创建本身，不在等待线程
tauri::async_runtime::spawn_blocking(is_dotnet_installed).await
```

真正有效的只有两条路：
1. **不创建子进程**（首选）；
2. 把创建推迟到窗口完全稳定之后（治标，且推迟后问题会重现）。

---PLACEHOLDER

## 三、症状 A（启动透明框）的排查过程

### 3.1 走过的弯路（**请勿重复**）

这段弯路浪费了大量时间，核心教训是**取证手段本身不可靠时，所有结论都不可靠**。

| 尝试 | 当时的结论 | 为什么错 |
|---|---|---|
| 用 CopyFromScreen 定时截图分析 | 认为抓到了透明框 | **该 API 抓不到 WebView2 的 GPU 合成层**。拍出来的画面往往是对的，据此分析等于分析了一个不存在的现象 |
| 高频抓帧对比亮度 | 找到若干异常帧 | 同上，所谓异常帧多半是图片加载中间态，不是窗口级透明框 |
| 怀疑隐藏的 Steam WebView2 窗口 | 做移出屏幕前后 A/B 对比，发现 34959 像素差异，一度认定是它 | **A/B 实验控制变量失败**：A 图拍到时框还在，B 图拍摄时框已自行消失，差异来自时间流逝而非变量 |
| 用 IsWindowVisible / WS_VISIBLE 判断窗口是否可见 | 得到 false，认为窗口确实隐藏 | 对普通窗口成立，但 **WebView2 宿主窗口的合成层不受该位控制**，仍可能被 DWM 合成 |
| 改窗口显示时机（rAF / setTimeout / ready 前后） | 反复调整，收效不明 | 显示时机确实有影响，但不是本次根因 |
| 用 requestAnimationFrame 延迟显示窗口 | —— | **导致软件完全打不开**：窗口 visible=false 时浏览器不产生动画帧，rAF 永不触发，show() 永不被调用 |
| 关闭窗口阴影 shadow: false | 无效 | 与阴影无关 |

### 3.2 最终有效的做法：**删除法二分**

不再试图观察透明框，而是**把启动任务逐个注释掉，让用户肉眼判断**。

这是唯一可靠的路子，因为**用户的肉眼就是最终的观测仪器**。

| 步骤 | 启用的启动任务 | 用户观察 | 推断 |
|---|---|---|---|
| 基线 | 全部禁用（只读配置） | 无框 | 元凶在这些任务里 |
| +1 | syncSteamSession | 无框 | 它无罪 |
| +2 | dotnet_init + plugins_init | **有框** | 元凶在二者之一 |
| +3 | 只 dotnet_init | **有框** | **锁定 dotnet_init** |

### 3.3 根因与修复

旧实现（src-tauri/src/core/runtime.rs）：

```rust
fn is_dotnet_installed() -> anyhow::Result<bool> {
    let output = Command::new("dotnet").arg("--list-runtimes").output();
    // ...
}
```

Command::output() 会启动 dotnet.exe 并**同步等待其退出**（冷启动 1-3 秒）。
Windows 在创建进程时要获取进程级加载器锁，**父进程的所有线程都会被短暂挂起**，
包括正在首次绘制的主窗口。

**修复**：改成纯文件系统探测，完全不创建子进程。

```rust
fn is_dotnet_installed() -> anyhow::Result<bool> {
    const CANDIDATE_ROOTS: [&str; 2] = [
        r"C:\Program Files\dotnet\shared\Microsoft.NETCore.App",
        r"C:\Program Files (x86)\dotnet\shared\Microsoft.NETCore.App",
    ];
    for root in CANDIDATE_ROOTS {
        // 读目录，看有没有 8.x / 9.x / 10.x 版本文件夹
    }
    // 也检查 DOTNET_ROOT 指向的自定义安装
    Ok(false)
}
```

**验证**：用户确认打开软件时没有透明框。

> 注意：修复前曾用延迟 8 秒执行验证过一次。用户反馈
> 打开没有框了，但过了几秒又有了 —— 这正好证明延迟只是把问题推后，
> **不是修复**。这一点值得记住：**延迟 = 问题仍在**。

---

## 四、症状 B（首次切到已安装页）的排查过程

### 4.1 已确认的关键事实

| 事实 | 依据 |
|---|---|
| **只有第一次切换出现** | 用户观察，多轮稳定复现 |
| 第二次及以后切换正常 | 用户观察 |

只有第一次几乎等价于 **缓存冷启动**：首次需要真正扫盘，之后命中缓存。

### 4.2 相关的现有实现

useWallpaperActions 的加载逻辑（src/hooks/useWallpaperActions.ts）：

```typescript
// 命中全局缓存就直接用，不扫盘
const cachedStore = useInstalledStore.getState();
const cachedItems = Object.values(cachedStore.byId);
if (!opts?.force && cachedStore.ready && cachedItems.length > 0) {
  setItems(cachedItems);
}
// 否则真的调用 we_list_installed 扫盘
const [list, meta] = await Promise.all([
  tryInvoke("we_list_installed", { force: opts?.force ?? false }, []),
  tryInvoke("metadata_get_all", undefined, {}),
]);
```

**这个缓存由启动时的 useInstalledStore.getState().refresh() 预热。**

### 4.3 高度怀疑的根因（**尚未修复**）

src-tauri/src/commands/we.rs 的 we_list_installed：

```rust
pub async fn we_list_installed(...) -> Result<Vec<InstalledWallpaper>, String> {
    let client = state.we_client.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = client.read();          // 读锁
        if force {
            guard.invalidate_installed();   // 内部还要拿 installed_cache 的 Mutex
        }
        guard.installed_wallpapers()        // 扫盘数秒，全程持有 client 读锁
    })
}
```

**问题**：扫盘期间一直持有 RwLock 的读锁。

而所有同步 we_* 命令都跑在**主线程**上，且都要 state.we_client.read()：

```rust
pub fn we_active_pubfileids(...) -> Result<Vec<String>, String> {
    Ok(state.we_client.read().active_pubfileids())   // 等锁，主线程卡住
}
```

**主线程卡在锁上 → 窗口无法重绘 → 透明框。**

这正好解释只有第一次：第一次扫盘（数秒，持锁）→ 有框；
之后命中 installed_cache（几乎瞬时）→ 无框。

### 4.4 一次失败的修复尝试（**请勿重犯**）

曾尝试这样改：

```rust
// 错的：临时值的生命周期覆盖整个语句
let dirs = { let guard = client.read(); guard.scan_directories_copy() };
client.read().scan_without_lock(dirs)
//     ^^^^^^ 这个临时 guard 活到语句结束，扫盘全程仍然持锁
```

用户反馈现在每次切到已安装页面都会有透明框 —— **比修复前更糟**，
因为同时恢复了启动预热，让每次切换都走扫盘路径。

**教训**：
1. Rust 的临时值生命周期到**语句结束**，expr.method() 里 expr 产生的临时值
   在整个方法调用期间都存活。要真正提前释放，必须显式 let guard = ...;
   并在独立的作用域块里让它 drop。
2. **改完必须自己验证，不能假设这样应该就行**。

### 4.5 正确的修复方向（待实施）

```rust
// 正确：把扫盘做成不接收 &self 的关联函数，锁只用于取路径
pub fn scan_from_dirs(dirs: Vec<(PathBuf, &'static str)>) -> Vec<InstalledWallpaper> {
    // 纯函数，不碰任何锁
}

// command 里：
let dirs = {
    let guard = client.read();      // guard 在此作用域结束时 drop
    guard.scan_directories_copy()
};                                   // 锁已释放
let scanned = WallpaperEngineClient::scan_from_dirs(dirs);   // 无锁扫盘
client.read().store_installed(scanned.clone());              // 短暂加锁写缓存
```

**注意**：installed_cache 是独立的 std::sync::Mutex，不在 we_client 的
读写锁保护范围内，所以写缓存不需要 we_client 的锁。

---

## 五、当前代码状态（截至本文提交）

src/hooks/useBootstrap.ts 中，以下启动任务被**注释掉**，尚未恢复：

```typescript
// void maybeCheckForUpdates();
// if (!dotnetInitialized.current) {
//   dotnetInitialized.current = true;
//   void invoke("dotnet_init").catch(() => undefined);
//   void invoke("plugins_init").catch(() => undefined);
// }
// void registerWindowStatePersistence();
// void useInstalledStore.getState().refresh();
```

**这是一次二分实验留下的状态，不是最终形态。** 接手的人需要：

1. 先修 4.5 的锁问题；
2. 再把上述任务逐个恢复，每恢复一个都要验证：
   - 启动无透明框
   - 首次切到已安装页无透明框
3. 恢复顺序建议：useInstalledStore.refresh → registerWindowStatePersistence
   → maybeCheckForUpdates → dotnet_init / plugins_init

另外 src-tauri/src/core/app_state.rs 的 init_cookies 里有：

```rust
std::thread::sleep(std::time::Duration::from_millis(300));
```

这个 300ms 是为了避开 setup() 阶段主线程尚未进入事件循环的问题，**不要删**。

还有一处必须保留：src-tauri/src/services/workshop/webview.rs 里创建隐藏
Steam 窗口时的 .decorations(false).shadow(false)，以及它的注释。
那条注释记录了一个真实的 DWM 合成伪影，删掉注释会诱导后人把它优化回去。

---

## 六、方法论教训（比 bug 本身更重要）

### 6.1 取证手段必须可靠

本次最大的时间浪费来自**用不可靠的手段取证**：

- CopyFromScreen 抓不到 WebView2 的 GPU 合成层；
- 定时截图会错过瞬时现象；
- A/B 对比时没有控制好时间变量。

**结论**：当现象是瞬间闪现且发生在 GPU 合成层时，
**唯一可靠的观测仪器是用户的肉眼**。应当尽早改用删代码让用户看的二分法。

### 6.2 延迟不是修复

延迟 8 秒后问题重现这个反馈极其有价值 —— 它证明问题是**被推后**而不是**被消除**。
判断一条修复是否有效，标准是：把触发条件放在任意时间点上它都不再发生。

### 6.3 不要连续叠加猜测性修改

本次曾在同一轮里同时改：窗口显示时机、CSS 背景、Steam 窗口位置、阴影开关。
即使问题消失了也无法归因，而且其中某些改动（如 rAF）反而引入了更严重的新 bug。

**正确做法**：一次只改一处，改完立刻验证，验证通过再改下一处。

### 6.4 部署必须覆盖用户真正双击的那个文件

曾经有一整轮排查完全无效，原因是：开发者一直在更新 D:/PortableApps/WEave.exe，
而用户双击的是桌面上的另一个副本。所有修复用户都没测到。

**现在的约定**：桌面快捷方式 WEave.lnk 指向 D:/PortableApps/WEave.exe，
桌面不再放 exe 副本，部署只需更新 D:/PortableApps/WEave.exe。

### 6.5 编辑工具与行尾

本仓库的 .ts 文件使用 **CRLF** 行尾。用多行 oldText 做精确替换时，
必须把换行写成 \r\n，否则永远匹配失败。排查这一点也花了不少时间。

---

## 七、附录：本次涉及的文件

| 文件 | 角色 |
|---|---|
| src-tauri/src/core/runtime.rs | .NET 运行时探测（**已修**：去掉子进程）|
| src-tauri/src/core/app_state.rs | AppState::initialize、init_cookies、init_dotnet_runtime |
| src-tauri/src/commands/we.rs | we_list_installed（**待修**：扫盘持锁）|
| src-tauri/src/services/we_client.rs | installed_wallpapers / scan_installed_wallpapers |
| src-tauri/src/services/workshop/webview.rs | 隐藏 Steam 窗口的创建与 cookies 同步 |
| src/hooks/useBootstrap.ts | 启动流程编排（当前含二分残留）|
| src/hooks/useWallpaperActions.ts | 已安装页数据加载与缓存命中 |
| src/stores/installed.ts | 全局已装壁纸缓存 |
