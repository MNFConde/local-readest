# selfhosted 分支纪律:apps/readest-app/ 目录约定

> FORK(selfhosted)专属文档。本目录是上游 `readest/readest` 的客户端代码:上游会持续演进并定期合入本分支(`git merge main`)。本文件的存在意义是让每次合入的冲突面最小——**每写一个 hunk 前自问:上游合并时,这是不是又一个冲突点?**
> 仓库级纪律(分支模型、合并流程、提交规范)见 `docs/selfhosted/contributing.md`。

## 放置优先级阶梯

新改动能放多低就放多低,只有全都不可行才允许动上游共享文件:

1. **fork 自有子树/文件**(零冲突):`src/libs/selfhost/`、`src/admin/`、`src/services/selfhostedAuth.ts`、`selfhostedServerUrl.ts`、`src/app/admin/`、fork 自有组件与测试。新功能一律先找这里。
2. **SELFHOSTED 门控块**(小冲突):共享文件内的 `if (SELFHOSTED) ...` 早分支或委托调用,行数个位数,带 `SELFHOSTED (fork)` 注释。
3. **最小 hunk**(不可避免时):类型放宽(`User | SelfhostedUser`)、可空客户端守卫、`#[cfg]` 类似的单行接缝。写清注释,行数越少越好。

## 现有接缝(复用,别新造)

- **开关**:`SELFHOSTED`(src/utils/supabase.ts,构建期 `NEXT_PUBLIC_SELFHOSTED=1`)。
- **可空客户端**:`supabase` 导出为 `SupabaseClient | null`;selfhost 下为 null,消费方判空分流。
- **运行时 API 基址**:`getBaseUrl()`(src/services/environment.ts)登录后从 localStorage 读服务端地址——任何 API 端点必须**调用时求值**(模块级常量会在登录前固化为官方域名)。
- **委托库**:`src/services/selfhostedAuth.ts`(登录/JWT/凭据/会话刷新/回调处理);`src/libs/selfhost/`(SelfhostedAuthForm、shelfScope、transferAdapters)。共享文件里的自托管逻辑应委托到这里,而不是内联展开。

## i18n 政策

- **禁止直接编辑 `public/locales/*/translation.json`**:这些文件必须与上游逐字节一致(CI 硬失败)。
- fork 词条一律进 `locales/<lng>/selfhost.json`(zh-CN 有翻译;en 放 key=原文兜底;其余 locale 走 en 回退),组件用 `useTranslation('selfhost')`(已配 `fallbackNS: 'translation'`,共享键照常解析)。
- **禁止运行 `pnpm i18n:extract`**:单配置 scanner 不认识 selfhost namespace,会把 fork 词条以 `__STRING_NOT_TRANSLATED__` 写回全部 translation.json。

## 受保护文件与守卫

`scripts/selfhost/check-decoupling.mjs`(`pnpm check:decoupling`,CI 在 `.github/workflows/selfhost-checks.yml`):translation.json 与上游有 diff 直接失败;下述共享文件改动行数(相对上游 merge-base 的增+删)超预算警告:

- 接缝根:`src/utils/supabase.ts`、`src/services/environment.ts`、`src/i18n/i18n.ts`
- 五个已收敛接缝:`EmailPasswordAuth.tsx`、`AuthContext.tsx`、`library/page.tsx`、`libraryService.ts`、`libs/storage.ts`、`helpers/auth.ts`
- 构建/CI/配置:`package.json`(根与 app)、`pull-request.yml`、`codeql.yml`、`Dockerfile`、`scripts/tauri.mjs`
- 本目录 `AGENTS.md`:唯一允许的永久 diff 是顶部 FORK 指针(≤2 行)

**预算只收紧、不放宽**:每次接缝收敛后把该文件的预算降到新的实际值。

## 测试注意

- mock `@/utils/supabase` 时必须继续导出 `SELFHOSTED`(真实值为构建期常量,测试里通常 mock 为 `false`)。
- 抽取新模块时,检查新模块**整条 import 链在模块求值期**调用的导出(例:`libraryStore` 顶层调 `isTauriAppPlatform()`):既有测试的 `vi.mock` 工厂须同步补齐这些导出,否则测试文件在收集期即炸(报错形如 `No "X" export is defined on the "..." mock`)。
- 共享组件的既有测试都按 `SELFHOSTED=false` 跑,重构必须保持非 selfhost 路径是上游行为。

## 依赖

- 新增 npm 依赖前先评估合并冲突(上游也会动 package.json/lockfile);优先用现有依赖。Rust 侧(src-tauri)依赖上游管,fork 不动。
