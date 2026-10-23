<!--
  FORK(selfhosted) 门面文件:本 README 永不取上游内容。
  上游合并冲突时一律 `git checkout --ours README.md && git add README.md`
  (见 docs/selfhosted/contributing.md「上游合并循环」)。
  官方版说明请看上游仓库:https://github.com/readest/readest
-->

# Readest Selfhosted

[Readest](https://github.com/readest/readest) 是一款开源电子书阅读器——Foliate 的现代化重写,基于 Next.js 与 Tauri v2,覆盖 macOS、Windows、Linux、Android、iOS 与 Web。本仓库是其 **selfhosted fork**:官方客户端原样保留,只加一层自托管开关,配一套自建的同步服务端,组成完全不依赖官方云的阅读套件。

## 组成

| 部分                                     | 说明                                                                                                                                         |
| ---------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| 客户端 `apps/readest-app/`               | 上游 Readest + `NEXT_PUBLIC_SELFHOSTED=1` 自托管开关:登录连自建服务端、内置多端管理页;开关关闭时行为与上游一致。功能与排障见[上游文档][upstream-readme]。 |
| 服务端 `sync-server/`                    | Rust 多用户同步服务端:账号+密码登录、书库/进度/标注同步、OPDS 书城(内容寻址去重+硬链接)、SQLite 单文件免配置启动、admin 管理页与备份下载。                 |
| 进程管理层 `sync-server/supervisor/`     | 可选:一条命令拉起 sync-server + web 前端,自动重启 + TUI 仪表盘,子进程随退全停(Windows Job Object / Unix PDEATHSIG)。                                    |

## 分支模型

- `main` = 上游 `readest/readest` 的镜像,只 fast-forward。
- `selfhosted` = 本分支:官方客户端 + 自托管补丁 + sync-server;桌面/安卓客户端的自动更新源也指向本仓库 Releases。

## 文档

- [docs/selfhosted/README.md](./docs/selfhosted/README.md) — 自托管总览、补丁内容与分支模型
- [docs/selfhosted/contributing.md](./docs/selfhosted/contributing.md) — 上游合并循环、发布与版本号策略
- [docs/selfhosted/sync-protocol.md](./docs/selfhosted/sync-protocol.md) — 服务端 API 契约
- [sync-server/README.md](./sync-server/README.md) — 服务端运行、部署与运维
- 客户端功能介绍、构建指南与 Troubleshooting 见[上游 README][upstream-readme]

## 下载

- **桌面 / 安卓客户端**:本仓库 [Releases](https://github.com/MNFConde/local-readest/releases),已装客户端「检查更新」同源。
- **服务端**:Release 附带的 server bundle(zip 解压即用,内含 supervisor)或镜像 `ghcr.io/mnfconde/readest-sync-server`;详见 [sync-server/README.md](./sync-server/README.md)。

## License

AGPL-3.0,与上游一致(见 [LICENSE](./LICENSE));fork 的全部改动同样以 AGPL-3.0 提供。

[upstream-readme]: https://github.com/readest/readest#readme
