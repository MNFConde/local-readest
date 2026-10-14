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

## 重生成补丁

```bash
cd ../ywz-readest-selfhosted       # ywz 仓库 clone（已移出仓库：D:CodeProjectywz-readest-selfhosted）
git remote add official https://github.com/readest/readest.git && git fetch official main
BASE=$(git merge-base official/main master)
git diff $BASE..master -- <README 上列出的收录文件清单> > body.patch
# 前置说明头后覆盖 docs/selfhosted/readest-client-selfhost.patch
```
