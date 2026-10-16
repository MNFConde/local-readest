# 自托管同步（selfhosted 分支）

本目录存放"官方客户端 + 自建同步服务端"路线的核心资产。分支模型：

- `main` = 官方 `readest/readest` 的镜像，只 fast-forward（`git fetch official && git merge --ff-only official/main`）
- `selfhosted` = `main` + 本补丁 + `sync-server/`（Rust 重写的同步服务端）+ 契约文档/测试

## 跟上游循环

```bash
git checkout main && git fetch official && git merge --ff-only official/main && git push origin main
git checkout selfhosted && git merge main
git apply --3way docs/selfhosted/readest-client-selfhost.patch   # 解 6+ 文件冲突
# 跑 sync-server 契约测试 + web 构建，通过后 commit
```

`git apply --3way` 需要 pre-image blob：本仓库已 `git remote add official https://github.com/readest/readest.git && git fetch official main`，官方全量历史在库即可。

## 补丁内容（甄选自 ywz626/readest-selfhosted）

基线：官方 `ac757777d`（merge-base），提取时官方已领先 478 提交——**冲突大概率集中在文件级重排，机械解决即可；若官方改了同步语义需产品裁决，停下讨论**。

收录（自托管核心）：
- `src/services/environment.ts`：`getBaseUrl()` 自托管地址优先（登录时录入，localStorage 持久化）——**漏掉它会让 sync/storage 全部静默回退官方域名**（T10 E2E 抓到过）
- `src/services/selfhostedAuth.ts`、`selfhostedServerUrl.ts`（新增）：账号+密码 → `POST /api/auth` 换 JWT（密码可空=免密账号；凭据 localStorage 持久化用于静默重登，兼容旧「登录码」key）；运行时录入服务端地址（自动补 `http://` scheme，localStorage 持久化，免按用户编译）
- `src/utils/supabase.ts`：`NEXT_PUBLIC_SELFHOSTED=1` 开关；selfhost 下 supabase client 为 null（调用点 `supabase!`）
- `src/utils/access.ts`、`src/helpers/auth.ts`、`src/context/AuthContext.tsx`：token/用户派生走 JWT `sub`；过期后用持久化账号密码静默重登
- `src/app/auth/*`（AuthPanel / EmailPasswordAuth / page / recovery / update）：登录 UI——服务端地址框 + 账号（邮箱）框 + 密码框（可空），隐藏 OAuth
- `src/admin/`（自包含管理页子树）+ `src/app/admin/page.tsx`（单路由挂载）：多端管理控制台（建/删用户、重置密码、角色、停用启用、删除书目、blob GC、Open Library 自动归组），admin 账号在 web/桌面/安卓任意端访问 `/admin`；**工程约束**：只依赖 node_modules 的 daisyUI/Radix 与 fork 自有模块（`selfhostedAuth.jwtRole` 解码 role claim），不 import 上游业务组件，上游合并冲突面≈0
- `src/admin/AdminConsoleMenuItem.tsx`（admin 子树内）+ `src/app/library/components/SettingsMenu.tsx` 的两行挂载：设置菜单「管理控制台」入口——仅 `NEXT_PUBLIC_SELFHOSTED=1` 且所存 JWT 带 `role=admin` 时渲染（登录/登出后无需刷新页面即出现/消失；role 只做 UI 门，`/api/admin` 服务端才是闸）；URL 直达 `/admin` 仍是兼容方式
- `src/admin/HostMonitorSection.tsx`（admin 子树内，待办⑦）：管理页「服务监控」标签页——轮询 `GET /api/admin/host-snapshot` 渲染 supervisor 推来的双服务状态/延迟/内存/CPU；无 supervisor 数据时降级为「管理层未接入」提示（发布包外部署管理页其余功能不受影响）
- `sync-server/supervisor/`（新增独立 crate `readest-supervisor`，待办⑦）：发布包双进程管理层——spawn/守护 sync-server+web（退避重启+熔断）、TUI 面板（ratatui）、Windows Job Object/Unix PDEATHSIG 全停防孤儿、手写 localhost HTTP 探活直连、指标快照推送（防环铁律：控制权只在 supervisor/探活不借道/推送失败不参与重启决策）；与 sync-server 零代码依赖（自带空 `[workspace]`），服务端仅加 `/api/admin/host-snapshot` 存取端点
- `src/hooks/useQuotaStats.ts`、`useAvailablePlans.ts`：配额走自建服务端；跳过 Stripe/IAP
- `src/libs/storage.ts`、`src/libs/sync.ts`、`src/libs/share.ts`、`src/libs/user.ts`、`src/libs/metadata.ts`、`src/services/translators/providers/deepl.ts`：**API 端点一律调用时求值**（模块级常量会在登录前固化为官方域名）+ blob 相对 URL 经 `toFullUrl` 拼接且带 Bearer 头（web 的 XHR 分支也必须走 `toFullUrl`）；`src/utils/transfer.ts`、三个 sync provider：`tauriUpload` headers 类型修正为 `Record`
- `src/services/transferManager.ts`：错误信息透传（利于排障自建服务端错误）
- `src-tauri/src/transfer_file.rs`：分块下载失败重试/严格 206 校验/失败即中止删残片——对家庭宽带自建服务端重要（其 tokio `time` 依赖官方已自带，对应 Cargo.toml hunk 已从补丁剔除——重放会把官方的 `fs,net,sync,time` 挤回旧集致编译失败，2026-10-11 抓到）
- `src/__tests__/utils/selfhostedAuth.test.ts`、`helpers/auth.test.ts`、`helpers/supabase-mock.ts`

**明确排除**（ywz 混入的自有功能，勿在重放时顺手带回）：
`pinnedAt`/`libraryPinnedGroups`（书签置顶功能：types/settings.ts、types/book.ts、types/records.ts、merge.ts、adapters/settings.ts、useBooksSync.ts、constants.ts、library 组件）、统计页面（app/stats/*、statisticsDb、migrations、useReadingStats 等）、OPDS 改动、Android 品牌改造（`src-tauri/gen/android` 全部、`tauri.conf.json` 的 `com.ywz626.readest.sh`）、`layout.tsx` 旧 WebView polyfill、locale 文件（登录 UI 英文回退，需要时补 zh-CN 词条）、`lib.rs`（debug devtools）。

## 手工位（补丁之外，跟上游后需人工核对）

1. `apps/readest-app/package.json`——ywz 加了 server 构建脚本，我们不用它们的脚本；启用方式见下条
2. 构建期开关放 `apps/readest-app/.env.local`（gitignored，Next 自动加载，无需改脚本）：
   ```
   NEXT_PUBLIC_SELFHOSTED=1
   # NEXT_PUBLIC_API_BASE_URL 可留空：安装包支持登录时运行时输入服务端地址
   ```
3. 服务端本体在 `sync-server/`（Rust，独立 crate，空 `[workspace]` 不进根 workspace），契约见 `sync-protocol.md`
4. **Windows 宿主交叉编 Android**（上游 turso bug，Linux CI 不触发）：`turso_ext 0.6.1` 的 build.rs
   用 `cfg!(target_os)`（判宿主）给链接塞 `-ladvapi32`，中间 cdylib `turso_sdk_kit` 链接时在
   Android sysroot 找不到该库即失败。缓解双保险：①NDK sysroot 手工放空壳库（NDK 重装后重做）：
   ```bash
   STUB=apps/readest-app/src-tauri/android-stubs/libadvapi32.a
   SYS=$NDK_HOME/toolchains/llvm/prebuilt/windows-x86_64/sysroot/usr/lib
   for d in aarch64-linux-android arm-linux-androideabi i686-linux-android x86_64-linux-android; do
     cp "$STUB" "$SYS/$d/26/libadvapi32.a"; done
   ```
   ②`src-tauri/build.rs` 的 android 分支已注入 `android-stubs/` 搜索路径（管最终链接；上游修复后①②一并移除）。
   桌面出包（Windows/NSIS）不需要这两步。
5. **Windows 宿主出 Android 包——完整手工链路（2026-10-11 全程验证）**：`tauri android build` 在本机
   卡两处上游问题——①CLI 往 jniLibs symlink 需 Windows 开发者模式；②gradle 的 rustBuild 任务经
   pnpm 重入 CLI 时 node 链稳定 fastfail（手动同命令正常）。绕行全程无需特权：
   ```bash
   # a) Rust 产物，每个 ABI 一次（最后 symlink 报错可忽略，libreadestlib.so 已生成在
   #    **仓库根** target/<abi>/release/——不是 src-tauri/target；CLI 还可能清空 jniLibs，b 步两个 ABI 都要重拷）
   pnpm exec dotenv -v KEEP_SOURCEMAPS=1 -e .env.tauri -- tauri android build -t aarch64 \
     --config <selfhost 覆盖 json> -- --features devtools     # 再跑一遍 -t x86_64
   # b) 把两个 libreadestlib.so 复制进 gen/android/app/src/main/jniLibs/{arm64-v8a,x86_64}/
   # c) gradle 打包，排掉 rustBuild 任务：
   (cd src-tauri/gen/android && gradlew.bat assembleUniversalRelease --console=plain \
     -x rustBuildArm64Release -x rustBuildArmRelease -x rustBuildX86_64Release -x rustBuildUniversalRelease)
   # d) 签名：build-tools 的 zipalign -f -p 4 + apksigner sign（调试 keystore，pass android）
   ```
   配套一次性环境：**JDK 21**（`scoop install temurin21-jdk`；Android Studio JBR 是 25，Gradle 8.14
   不认）、`D:\Software\Scoop\shims\pnpm.bat` 垫片（gradle 找 .bat 形态）、gen/android/gradle.properties
   加本机代理 systemProp（services.gradle.org 直连超时）、`gen/android` 未物化时先
   `pnpm tauri android init`（会按模板覆盖已提交的定制文件——`git checkout -- <gen 路径>` 恢复，
   根级新生成的 gradle 工程文件保留）并补 `app/src/main/res/values/ic_launcher_background.xml`
   （模板漏发，颜色对齐 drawable 的 #3DDC84）。已提交的自动修复：`src-tauri/build.rs`（advapi32 搜索路径）、
   `scripts/tauri.mjs`（android-studio-script 三元组→短架构名归一）。

## 网页端自托管部署（2026-10-14）

网页客户端与桌面/安卓共用一套代码，靠**构建期** `NEXT_PUBLIC_SELFHOSTED=1` 切到自托管模式（登录页出现服务端地址框、Supabase 关闭）。上游根 `Dockerfile` 本就是网页自托管镜像（运行时 `SELF_HOSTED=true` 解锁会员功能），本项目只补了 `NEXT_PUBLIC_SELFHOSTED` ARG 透传（2 行，不传时保持上游官方模式不变）。

- **Docker 一键栈**（推荐，NAS/Linux）：仓库根 `compose.selfhost.yml`——`sync-server`（:8080，数据落卷）+ `web`（:3000，`NEXT_PUBLIC_SELFHOSTED=1` 构建）。`docker compose -f compose.selfhost.yml up -d --build`；建首个管理员：`docker compose -f compose.selfhost.yml exec sync-server sync-server add-user <email> --admin`。注意根镜像构建需要 git 子模块已初始化（Dockerfile 内置检查）。
- **Windows 原生（与 sync-server.exe 同机）**：`cd apps/readest-app && NEXT_PUBLIC_SELFHOSTED=1 pnpm build-web && pnpm start-web`（或把 `NEXT_PUBLIC_SELFHOSTED=1` 写进 `.env.local`，`pnpm build-web` 经 dotenv 自动加载）。浏览器开 `http://<PC>:3000`，登录时服务端地址填 `http://<PC>:8080`（跨源已被服务端 CORS reflect 覆盖，无需反代）。
- 管理页：admin 账号登录后,书库页设置菜单有「管理控制台」入口（2026-10-15 起,需带该改动的客户端包）;URL 直达 `http://<host>:3000/admin` 继续可用,服务端内嵌静态页 `:8080/admin` 仍作兜底。
- 桌面/安卓包要获得管理页与账号密码登录，需在 `.env.local` 设 `NEXT_PUBLIC_SELFHOSTED=1` 后重打包（随下一版发布）。

## 自托管发布包（方案 B,2026-10-15;⑦ 管理层 2026-10-16 并入）

无 Docker 的一条命令全套：一个 Windows zip,入口是 `readest-supervisor.exe`（待办⑦ 的双进程管理层）——拉起并守护 sync-server（:8080）与网页端（:3000,standalone 产物 `node` 直跑,免 pnpm,需 Node ≥20 在 PATH 或 `READEST_NODE`）。

- **产出方式一（本地）**：
  ```bash
  cd sync-server && cargo build --release
  cd sync-server/supervisor && cargo build --release
  cd apps/readest-app && NEXT_PUBLIC_SELFHOSTED=1 BUILD_STANDALONE=true pnpm build-web
  node scripts/selfhost/package-server-bundle.mjs --zip
  # → build/selfhost-server/ + build/readest-selfhost-server_<ver>_x86_64-pc-windows-msvc.zip
  ```
- **产出方式二（CI）**：`release-selfhost.yml` 的 `build-server-bundle` job（tag 推送触发）,同一脚本打包并挂到 GitHub Release。
- **包内容**：`readest-supervisor.exe`（入口/管理者）+ `sync-server.exe` + `web/`（standalone 树 + static/public,`.pnpm` 已摊平成 npm 式扁平 `node_modules`,**零链接**——junction/symlink 进 zip 后普通账号解压即断,勿回退）+ `start.bat`/`stop.bat` + `README.txt` + `logs/`。端口覆写:`PORT`/`LISTEN_ADDR` 环境变量（同一份 `.env`）。
- **supervisor 行为**：崩溃自动重启（指数退避 1s/4s/16s,连续 5 次快速失败熔断并写明原因）;TUI 面板显示双服务状态/延迟/内存/CPU/日志尾巴（`q` 退出=全停）;**子进程与 supervisor 同生共死**（Windows Job Object,`taskkill /F` 管理层即全灭,无孤儿占端口）;每 5 秒向 sync-server `POST /api/admin/host-snapshot` 推指标（设了 `ADMIN_TOKEN` 才推,管理页「服务监控」标签页读取,契约见 sync-protocol.md §6.6）;探活直连各自端口,推送失败不影响重启决策（防环铁律）。
- **首次运行**：解压 → `start.bat`（打开面板窗口）→ 目录内 `sync-server.exe add-user <email> --admin` → 浏览器 `http://localhost:3000`（服务端地址填 `http://localhost:8080`）/`http://localhost:3000/admin`。桌面/安卓客户端填 `http://<PC-IP>:8080`（防火墙放行 8080/3000）。
- 常驻服务（可选）：NSSM/任务计划程序包 `readest-supervisor.exe --headless`（headless 输出状态行,stdout 进服务日志）;supervisor 守护两服务,服务管理器守护 supervisor,闭合最后一环。

## 重生成补丁

```bash
cd ../ywz-readest-selfhosted       # ywz 仓库 clone（已移出仓库：D:CodeProjectywz-readest-selfhosted）
git remote add official https://github.com/readest/readest.git && git fetch official main
BASE=$(git merge-base official/main master)
git diff $BASE..master -- <README 上列出的收录文件清单> > body.patch
# 前置说明头后覆盖 docs/selfhosted/readest-client-selfhost.patch
```
