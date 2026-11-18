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
- **管理页**：两种入口共存——客户端内置管理页（admin 账号登录后访问 `<web客户端>/admin`，web/桌面/安卓通用）与服务端内嵌静态页 `http://<host>:8080/admin`（兜底）。鉴权双轨：admin 用户登录 JWT（首个 admin 用 `add-user --admin` 或 `set-role` 产生），或 `ADMIN_TOKEN`（≥8 位，运维兜底轨，不设不影响 JWT 轨道）。功能：建/删用户、重置密码、角色管理、停用启用、书目改名/归组/删除、Open Library 自动归组、孤儿 blob 清理；「服务监控」标签页在有 readest-supervisor 推送时显示双服务状态并提供 启动/停止/重启 按钮（命令经 `/api/admin/service-command` 排队、由 supervisor 在下次推送时取走执行，契约见 `docs/selfhosted/sync-protocol.md` §6.6——停 sync-server 会熄火本邮箱，面板会先警示，恢复走 TUI/托盘/重启 supervisor）。

## 用户存档导出/导入（待办⑫）

每个账号可把自己的全部数据（书目+阅读记录+可选书文件）打包成一个 tar，再导入任意账号（本机或另一台服务器）：`GET /api/archive/export?scope=full|metadata` 流式导出，`POST /api/archive/import` 合并式导入（LWW，永不覆盖对方更新数据；重复导入幂等）。客户端入口=设置菜单「存档传输」；blob 走内容寻址去重，同服务器重复内容零新增字节。契约 `docs/selfhosted/sync-protocol.md` §6.8。

## 用户间推书（待办⑬）

同一服务器上的账号可以互相推荐书籍：`POST /api/pushes`（只推**已上传**的书，创建时快照书目元数据，不含笔记进度）→ 对方 `GET /api/pushes/inbox` 看到待收列表 → `accept`（文件 file→file 硬链接进收件方书库，去重零新增字节；快照行走 LWW，永不覆盖收件方更新的数据）或 `decline`（什么都不落地）。客户端入口=设置菜单「推送书籍」/「推荐收件箱」。契约 `docs/selfhosted/sync-protocol.md` §6.9。

## 设备身份（待办⑪-P1）

设 `ALLOW_AUTO_PROVISION=1` 后，客户端登录页「使用设备身份登录」可自生成 UUID+密钥直接成为新账号（`kind='device'`，免管理员建号）；密钥只显示一次、无任何找回，客户端强制先备份（备份文件=文本，含地址+ID+密钥）。设备号在用户列表单独分组（客户端管理页与内嵌页均可隐藏），`add-user --kind device` 也可手工建；用户间推书（⑬）双向拒绝设备号。缺省关闭=行为与旧版完全一致。契约 §1/§7。

## 账户管理（T5 起）

登录凭据为「账号（邮箱）+ 密码」；历史上生成的登录码原样当作密码使用，无需迁移。密码允许为空 = 免密登录（知道邮箱即可登入），服务端启动时会对这类账号打警告。

```
sync-server add-user <email> [--password <密码，缺省随机生成并打印；空串 = 免密>] [--admin] [--kind person|device]
sync-server set-role <email> <admin|user>
sync-server list-users
sync-server disable-user <email>
sync-server passwd <email> [--password <新密码>]     # 缺省随机生成
```

`role` 决定能否使用客户端管理页（JWT 携带 role claim）；既有库升级时 `role` 列自动迁移补齐，存量用户均为 `user`。

## 测试

```
cargo test        # 契约测试：内存 SQLite + 真实 router 逐端点断言（40 个，含 library feature 与 admin 用户/书目管理）
```
