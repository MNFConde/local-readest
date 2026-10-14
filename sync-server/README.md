# sync-server

Readest 自建同步服务端（Rust 重写，多用户版）。API 契约见 [`../docs/selfhosted/sync-protocol.md`](../docs/selfhosted/sync-protocol.md)。

## 运行

```powershell
# 零配置：双击（或直接运行）即可——JWT_SECRET 缺省时首启自动生成并落盘
# <数据目录>/jwt_secret.key，之后每次启动复用（token 跨重启不失效）；
# 数据缺省放 exe 同目录 data/（SQLite + files/ + 去重 blobs/），不占 C 盘。
cargo run --release
# 可用 env 覆盖：JWT_SECRET / DATA_DIR / LISTEN_ADDR（默认 0.0.0.0:8080；
# 裸 ":8080" 会被自动归一化为 0.0.0.0:8080——Windows 上 getaddrinfo 会把
# 裸端口解析成单个本机地址，导致 127.0.0.1/局域网连不上）
# QUOTA_BYTES 默认 2 GiB/人；MAX_BODY_BYTES 默认 512 MiB（单请求体上限，
# axum 隐含默认仅 2 MiB 会拒掉整本书的上传——调大请自担内存占用，blob 体整体进内存）
```

单文件部署：`cargo build --release` 产物为独立 `sync-server.exe`（CGO 无关，SQLite 已 bundled）；更新=替换 exe、保留 `data/`。删掉 `data/` 会轮换 JWT 密钥（全员重新登录）。

## 书库（OPDS + 管理页）

默认随主程序启用（cargo feature `library`；`--no-default-features` 可整体关闭，只剩同步）：

- **OPDS 书城** `http://<host>:8080/opds`：客户端「添加目录」填此地址，用户名=邮箱、密码=账号密码。同字节的书全服务端只存一份（内容寻址去重 + 硬链接），任意用户上传即全员可见可下载；`LIBRARY_SHARED=0` 改为仅自己可见。
- **管理页** `http://<host>:8080/admin`：需先设 `$env:ADMIN_TOKEN = "<至少 8 位>"`（不设则管理接口整体 404）。书目改名/归组到作品/标记译本来源（source_hash）、Open Library 自动归组、孤儿 blob 清理、用户停用启用。

## 账户管理（T5 起）

登录凭据为「账号（邮箱）+ 密码」；历史上生成的登录码原样当作密码使用，无需迁移。密码允许为空 = 免密登录（知道邮箱即可登入），服务端启动时会对这类账号打警告。

```
sync-server add-user <email> [--password <密码，缺省随机生成并打印；空串 = 免密>]
sync-server list-users
sync-server disable-user <email>
sync-server passwd <email> [--password <新密码>]     # 缺省随机生成
```

## 测试

```
cargo test        # 契约测试：内存 SQLite + 真实 router 逐端点断言（37 个，含 library feature）
```
