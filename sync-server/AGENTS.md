# sync-server/ 子树约定(fork 自有)

> 这里是 fork 独有的 Rust 同步服务端,零上游合并冲突,放开手改;但它是**多端客户端(官方客户端 fork)的唯一后端**,改的是契约就是改协议。仓库级通用纪律见 `docs/selfhosted/contributing.md`。

## API 契约

- 契约正本:`docs/selfhosted/sync-protocol.md`;端点的 wire shape 由 `tests/contract.rs` 逐端点钉死(真实 router + 内存 SQLite)。
- **改任何 `/api/*` 端点的请求/响应形状前**:先改 sync-protocol.md,再改 contract.rs,再改实现;三者必须同一提交落齐。
- 只加字段可以宽松处理(客户端忽略未知字段);改/删字段、改语义 = 破坏性变更,须评估已发布客户端(桌面/安卓/网页)的兼容窗口,必要时加版本化端点而不是原地改。
- `/api/admin/host-snapshot`(supervisor 推送指标)受 `ADMIN_TOKEN` 门控;防环铁律:控制权只在 supervisor、探活不借道推送、推送失败不参与重启决策。

## 数据库

- schema 在 `src/db.rs` 的 `CREATE TABLE IF NOT EXISTS` 初始化块中;新增列/表用 `ALTER TABLE ... `(或新表 + 迁移代码)保持**旧数据目录可原地升级**,禁止要求用户清库。
- 自建服务端的数据是用户唯一副本——破坏性 schema 变更必须附迁移路径并在 PR/提交正文写明。

## 质量门禁

- `cargo fmt --check` → `cargo clippy -- -D warnings` → `cargo test`;contract.rs 全绿是合并前置。
- 发布前/依赖升级后:`cargo audit`(CI `selfhost-checks.yml` 硬闸)。

## 依赖升级

- Cargo.toml 是 fork 自有,无上游把关:升级前看 release notes 的 breaking 项,升级后全量 `cargo test` + `cargo audit`;锁版本提交独立于功能提交。
- `supervisor/` 是独立 crate(空 `[workspace]`,不进本目录依赖树),与 sync-server 保持**零代码依赖**——共享的东西只能是协议(HTTP 端点/快照 JSON)。

## 部署形态(改动时想一下)

- 同一份二进制跑在 Docker 镜像、GitHub Release 五平台产物、Windows bundle(supervisor 拉起)三种形态;端口/地址必须可用环境变量覆写,数据路径不得硬编码绝对路径。
