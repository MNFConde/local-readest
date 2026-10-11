# sync-server

Readest 自建同步服务端（Rust 重写，多用户版）。API 契约见 [`../docs/selfhosted/sync-protocol.md`](../docs/selfhosted/sync-protocol.md)。

## 运行

```powershell
# 必填 JWT_SECRET（重启后 token 失效与否取决于它是否变化）
$env:JWT_SECRET = "0123456789abcdef..."
cargo run --release
# 默认 LISTEN_ADDR=:8080，数据在 ./data（SQLite + files/）
# QUOTA_BYTES 默认 2 GiB/人；MAX_BODY_BYTES 默认 512 MiB（单请求体上限，
# axum 隐含默认仅 2 MiB 会拒掉整本书的上传——调大请自担内存占用，blob 体整体进内存）
```

单文件部署：`cargo build --release` 产物为独立 `sync-server.exe`（CGO 无关，SQLite 已 bundled）。

## 账户管理（T5 起）

```
sync-server add-user <email> [--code <登录码，缺省随机生成并打印>]
sync-server list-users
sync-server disable-user <email>
sync-server passwd <email>     # 重新生成登录码
```

## 测试

```
cargo test        # 契约测试：内存 SQLite + 真实 router 逐端点断言
```
