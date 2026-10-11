# sync-server

Readest 自建同步服务端（Rust 重写，多用户版）。API 契约见 [`../docs/selfhosted/sync-protocol.md`](../docs/selfhosted/sync-protocol.md)。

## 运行

```powershell
# 必填 JWT_SECRET（重启后 token 失效与否取决于它是否变化）
$env:JWT_SECRET = "0123456789abcdef..."
cargo run --release
# 默认 LISTEN_ADDR=:8080，数据在 ./data（SQLite + files/ + 去重 blobs/）
# QUOTA_BYTES 默认 2 GiB/人；MAX_BODY_BYTES 默认 512 MiB（单请求体上限，
# axum 隐含默认仅 2 MiB 会拒掉整本书的上传——调大请自担内存占用，blob 体整体进内存）
```

单文件部署：`cargo build --release` 产物为独立 `sync-server.exe`（CGO 无关，SQLite 已 bundled）。

## 书库（OPDS + 管理页）

默认随主程序启用（cargo feature `library`；`--no-default-features` 可整体关闭，只剩同步）：

- **OPDS 书城** `http://<host>:8080/opds`：客户端「添加目录」填此地址，用户名=邮箱、密码=登录码。同字节的书全服务端只存一份（内容寻址去重 + 硬链接），任意用户上传即全员可见可下载；`LIBRARY_SHARED=0` 改为仅自己可见。
- **管理页** `http://<host>:8080/admin`：需先设 `$env:ADMIN_TOKEN = "<至少 8 位>"`（不设则管理接口整体 404）。书目改名/归组到作品/标记译本来源（source_hash）、Open Library 自动归组、孤儿 blob 清理、用户停用启用。

## 账户管理（T5 起）

```
sync-server add-user <email> [--code <登录码，缺省随机生成并打印>]
sync-server list-users
sync-server disable-user <email>
sync-server passwd <email>     # 重新生成登录码
```

## 测试

```
cargo test        # 契约测试：内存 SQLite + 真实 router 逐端点断言（33 个，含 library feature）
```
