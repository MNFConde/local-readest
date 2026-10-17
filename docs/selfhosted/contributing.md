# selfhosted 分支贡献纪律(仓库级)

> 本文件是被 git 跟踪的仓库级约束,对 `apps/readest-app/`、`sync-server/`、`scripts/selfhost/` 一体生效。
> 客户端目录专属细则见 `apps/readest-app/selfhost-agent.md`;服务端子树细则见 `sync-server/AGENTS.md`。

## 分支模型

- `main` = 官方 `readest/readest` 的镜像,**只 fast-forward**:`git fetch official && git merge --ff-only official/main`。
- `selfhosted` = main + fork 全部工作,**永不回合 main**;方向只有一个——main 定期合入 selfhosted。
- 远程:`official` = 官方上游,`origin` = fork(MNFConde/local-readest)。

## 上游合并循环

```bash
git checkout main && git fetch official && git merge --ff-only official/main && git push origin main
git checkout selfhosted && git merge main
# pnpm-lock.yaml 冲突:先 `git checkout --theirs pnpm-lock.yaml` 再 `pnpm install` 重新生成
# 冲突解法会被 rerere 记录,下次同型冲突自动复用(新机器先 `git config rerere.enabled true`)
pnpm check:decoupling                       # 守卫:translation.json 必须零冲突
cd apps/readest-app && pnpm install && pnpm lint && pnpm test   # 冒烟闸
```

- 冲突预期集中在共享文件的 SELFHOSTED 接缝行与 `package.json`/CI 等小文件;`apps/readest-app/AGENTS.md` 冲突的解法恒为**保留顶部 FORK 指针**。
- 合并前后各跑一次 `pnpm check:decoupling`,确认预算没有被合并意外撑爆。

## 发布与版本号

- **唯一版本真源**:`apps/readest-app/package.json`(`tauri.conf.json` 引用它,`release-selfhost.yml` 从这里读版本);Rust 侧**两个 crate** 随之同步改版本,并各自提交更新的 `Cargo.lock`:`sync-server/Cargo.toml` + `sync-server/supervisor/Cargo.toml`(supervisor 的 metrics 上报与 bundle README 都读它)。
- **正式版直接用上游版本号**:合并上游后按上游当前号发版(上游 0.12.16 → `v0.12.16`),不自造 +1——更新链只看自家 latest.json,上游号在本线内天然严格递增。
- **合并上游前的中间版 = 戳版** `<主体>-<YYYYMMDDHH>`(如 `0.12.14-2026101712`):比较器(`apps/readest-app/src/utils/version.ts` 及其 Rust 镜像)把「纯 10 位数字预发布号」识别为同主体更新,且同主体上戳版 > 正式版;戳精确到小时,同一小时第二次发布手动加一。
- **发布动作**:bump 版本提交 → push → `git tag -a v<版本> && git push origin v<版本>`(触发 `release-selfhost.yml`)→ 盯流水线全绿。release 由 `gh release create` 创建(无 `--prerelease`),自动成为 `releases/latest/download/latest.json` 更新源;戳版首次进 stable 通道需冒烟验证桌面/安卓更新链。
- **fetch 纪律**:`git fetch official` 不拉 tags(`remote.official` 只配分支引用),避免上游未来同名 tag 与自推 `v` 同名冲突。
- **既成事实**:`v0.12.13`/`v0.12.14` 为纯 fork 号,与上游同名版本无对应;0.12.13 曾因 CI env 漏 `NEXT_PUBLIC_SELFHOSTED=1` 静默出标准构建——发版前按「UI 里有编译期门」的清单核对流水线 env。
- **记录惯例**:每次正式发布后在本地 cairn LOG 记一行映射「我们的 X ≈ 上游 Y + fork 改动」(cairn 不随 clone 走)。

## 新机器初始化

```bash
git config rerere.enabled true        # 冲突解法记忆(仓库本地配置,不随 clone 走)
pnpm install                          # 根 workspace
```

## 解耦守卫(scripts/selfhost/check-decoupling.mjs)

- **硬失败**:任何 `apps/readest-app/public/locales/*/translation.json` 与上游 merge-base 有 diff。
- **预算警告**:受保护共享文件改动行数(增+删)超预算;跑稳一个合并周期后以 `--strict`(`DECOUPLING_STRICT=1`)升为硬失败。
- **表外报告**:修改了预算表之外的上游文件会列出提醒——新动上游文件必须同步登记预算。
- CI:`.github/workflows/selfhost-checks.yml`(decoupling job + cargo audit 硬闸 + pnpm audit 报告级)。

## 临时脚本三次法则

- **判定口径**:同一功能的分析/验证逻辑重复出现即算(参数/目标变化不算新脚本);单行 shell 惯用语不算;同一会话重写与跨会话重现同等计数。
- **第 3 次出现必须固化**:进 `scripts/selfhost/` + 在 `scripts/selfhost/README.md` 登记作用/用法;固化后**只准扩展**(新参数/新函数),禁止再为该功能另写临时脚本散落 /tmp 或会话内。
- **一次性探针登记义务**:单/双次使用的诊断代码在 `scripts/selfhost/README.md` 台账记一行(日期/内容/用途/次数);跨会话累计 3 次的探针同样升级固化。
- 台账是记忆装置:写探针前先查台账——已有近似条目且自己就是第 3 次,直接固化。

## 提交信息规范

- 格式:`type(scope): 摘要`(英文小写祈使句),正文(可选)写动机与影响——为什么改,而不是改了什么。
- type ∈ `feat` / `fix` / `build` / `style` / `docs` / `refactor` / `test` / `chore`;scope 常用 `reader` / `admin` / `selfhost` / `supervisor` / `i18n` / `auth` / `library` / `bundle` 等。
- 每个逻辑单元一个提交;接缝收敛类提交须在正文写明"共享文件接缝 N → M 行"与守卫预算变化。

## 质量门禁

- **push 前**(`.husky/pre-push` 已挂):`pnpm format:check` + `pnpm lint` + `pnpm test`(全量 vitest)。
- **CI**:上游 `pull-request.yml`(lint + web 构建 + e2e,含 sync-server 真实构建)+ 本 fork `selfhost-checks.yml`(守卫 + audit)。
- Rust(sync-server):`cargo fmt --check` → `cargo clippy -- -D warnings` → `cargo test`;发布前 `cargo audit`。
