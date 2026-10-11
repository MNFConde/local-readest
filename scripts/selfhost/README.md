# selfhost 自动化验证脚本

自托管路线（`selfhosted` 分支）的 L1/L2 验证门脚本。开发验证策略与里程碑状态见 `cairn/ROADMAP.md` 的「执行计划」节；用法细节沉淀在 `cairn/readest-selfhost.md` 经验教训 10–12。

## 前置

- 桌面：`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9223 target/release/readest.exe`
- 安卓模拟器：`emulator -avd test36 -no-window -no-snapshot -gpu host`（或去掉 -no-window 看窗口），app 启动后：
  `adb forward tcp:9222 localabstract:webview_devtools_remote_<pid>`（pid = `adb shell pidof com.bilingify.readest`；改进程后 pid 会变，须重 forward）
- sync-server：`JWT_SECRET=… DATA_DIR=… LISTEN_ADDR=127.0.0.1:8080 sync-server.exe`（模拟器内宿主机是 `http://10.0.2.2:8080`）
- 本机 curl 记得 `--noproxy '*'`（系统代理会吞本地请求）。

## desktop-cdp.js — 桌面全链路门（M1 汉化 + 登录 + OPDS 浏览/下载）

```
node scripts/selfhost/desktop-cdp.js 9223 http://127.0.0.1:8080 <登录码> x 流架构
```
输出各步 PASS/FAIL 与 `DESKTOP-GATE=PASS|FAIL`，截图落 `tmp/spotcheck/desktop-m3.png`。

要点（改 UI 前请重读）：
- React 合成事件对部分组件忽略 `el.click()`——脚本一律走 `__reactProps.onClick` 直调（invokeText），并用 `location.pathname` 断言防「库内同名书」假阳性。
- 添加目录表单的提交按钮文案是 **添加目录/Add Catalog**（非「保存」），保存校验会真拉一次目录 URL（≤10s）。
- 自定义目录卡**无浏览按钮**（整卡即点击），内置目录才有；凭据存在 Tauri 文件型 settings（非 localStorage），`/opds?url=&id=` 需带目录 id 才会带认证。

## android-spotcheck.js — 安卓轻量抽检（浏览+下载直连）

```
adb forward tcp:9222 …  # 先完成
node scripts/selfhost/android-spotcheck.js 9222 10.0.2.2:8080 <登录码> 流架构
```
用 userinfo 凭据直开 `/opds/books`：浏览可用它验证；**下载不要用 userinfo**（Rust 下载器不支持 userinfo URL，会「下载失败」）——完整下载门用 desktop-cdp.js 的管理器流程，或真机手动。

## cdp-login.js — 最早的登录闭环脚本（保留备查）

模拟器/桌面自托管登录自动化（填 server-url + 登录码 → 断言 JWT 落 localStorage）。
