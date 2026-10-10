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
- `src/services/selfhostedAuth.ts`、`selfhostedServerUrl.ts`（新增）：登录码 → `POST /api/auth` 换 JWT；运行时录入服务端地址（localStorage 持久化，免按用户编译）
- `src/utils/supabase.ts`：`NEXT_PUBLIC_SELFHOSTED=1` 开关；selfhost 下 supabase client 为 null（调用点 `supabase!`）
- `src/utils/access.ts`、`src/helpers/auth.ts`、`src/context/AuthContext.tsx`：token/用户派生走 JWT `sub`；过期后用持久化登录码静默重登
- `src/app/auth/*`（AuthPanel / EmailPasswordAuth / page / recovery / update）：登录 UI——服务端地址框 + 登录码框（密码框复用），隐藏 OAuth
- `src/hooks/useQuotaStats.ts`、`useAvailablePlans.ts`：配额走自建服务端；跳过 Stripe/IAP
- `src/libs/storage.ts`、`src/utils/transfer.ts`、三个 sync provider：本地磁盘 blob URL（相对路径）带 Bearer 头上传/下载；`tauriUpload` headers 类型修正为 `Record`
- `src/services/transferManager.ts`：错误信息透传（利于排障自建服务端错误）
- `src-tauri/src/transfer_file.rs` + `Cargo.toml`（tokio `time`）：分块下载失败重试/严格 206 校验/失败即中止删残片——对家庭宽带自建服务端重要
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

## 重生成补丁

```bash
cd tmp/readest-selfhosted          # ywz 仓库 clone（untracked，随用随更新）
git remote add official https://github.com/readest/readest.git && git fetch official main
BASE=$(git merge-base official/main master)
git diff $BASE..master -- <README 上列出的收录文件清单> > body.patch
# 前置说明头后覆盖 docs/selfhosted/readest-client-selfhost.patch
```
