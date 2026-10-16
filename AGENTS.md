# Readest 协作规则

> 本项目使用 Project Cairn 组织项目知识:Cairn 全套规则(初始化配置/阅读顺序/文档职责/冲突仲裁/知识沉淀)见 `cairn/AGENTS.md`。
> 本机装有 project-cairn skill 且仓库存在 `cairn/` 时生效;否则视为不适用,跳过。
> 同目录的 `CLAUDE.md` 只是一行 `@AGENTS.md` 存根,让 Claude Code 读到同一套规则。

## 项目一句话简介

Readest 是一款开源电子书阅读器,是 Foliate 的现代化重写,基于 Next.js 16 与 Tauri v2 构建,为 macOS、Windows、Linux、Android、iOS 与 Web 提供沉浸式深度阅读体验。本仓库是其 selfhosted 分支开发仓库(官方客户端 fork + 自建 sync-server)。

## 阅读顺序

1. 先读本文件。
2. 读 `cairn/AGENTS.md`(Cairn 全套规则,含 ROADMAP/LOG 的阅读顺序)。
3. 按任务需要读 `cairn/` 知识专题文档与 `docs/selfhosted/` 工程文档。

## 工程纪律(selfhosted 分支)

- **动 `apps/readest-app/` 下任何上游共享文件前,必读 `apps/readest-app/selfhost-agent.md`**(放置阶梯与接缝约定);仓库级分支模型与上游合并流程见 `docs/selfhosted/contributing.md`。
- `apps/readest-app/public/locales/*/translation.json` 禁止直接编辑——fork 词条一律进 `locales/<lng>/selfhost.json`(selfhost namespace)。
- 改动行数守卫:`pnpm check:decoupling`(CI 同源;translation.json 硬失败,共享文件超预算警告)。

## 质量门禁

- push 前必须过 `pnpm format:check` + `pnpm lint` + `pnpm test`(pre-push 钩子已挂,与 CI 对齐)。

## 完成回复闸门

- 任何完成声明之前,先执行 Cairn 检查点(project-cairn skill 的 `references/maintenance.md`);细则与触发矩阵见 `cairn/AGENTS.md`。明确的只读/禁改请求免除。
