# Readest 自建同步服务端契约（sync-protocol）

从 `ywz626/readest-selfhosted` 的 `readest-sync-server`（Go）源码逐文件提取，并叠加本项目的多用户改造决策。**Rust 实现（`sync-server/`）必须逐字段对齐本文**；客户端侧代码（`apps/readest-app/src/services/selfhostedAuth.ts`、`libs/storage.ts`、`libs/sync.ts`、`utils/transform.ts`）是另一半契约权威，两端冲突时以官方客户端解析代码为准。

## 0. 总则

- 所有 API 在 `/api` 前缀下；客户端经 `getBaseUrl()`（登录时运行时录入，localStorage `selfhosted_server_url`，去尾斜杠）+ `/api` 拼接。
- 除 `POST /api/auth` 外全部要求 `Authorization: Bearer <JWT>`。
- 错误信封：`{"error": "<message>", "code": "<CODE>"}`，`Content-Type: application/json`。
- 错误码约定：`AUTH`(401/403)、`VALIDATION`(400)、`LOCKED`(429)、`QUOTA_EXCEEDED`(413)、`SERVER`(500)。
- CORS：反射任意 Origin；允许头 `Authorization, Content-Type, X-Device-Id, Accept, Origin`；`Access-Control-Expose-Headers: Retry-After`；OPTIONS → 204。
- 持久化：SQLite（WAL + `busy_timeout=5000`）+ 本地磁盘文件目录。

## 1. 认证

### POST /api/auth（公开）

请求头：`X-Device-Id: <uuid>`（客户端 localStorage 持久化的设备号，用于爆破锁定）。
请求体：`{"email": "<账号邮箱>", "password": "<密码>"}`；`password` 允许空串（免密账号）。

向后兼容：旧客户端的 `{"code": "<登录码>"}` 继续接受——`code` 是 `password` 的旧拼法（同一列对），不带 email 的密码按「码即身份」老路径校验。

响应：
| 场景 | 状态 | 体/头 |
|---|---|---|
| 成功 | 200 | `{"access_token":"<JWT>","token_type":"bearer"}` |
| 凭据错误 | 401 | `{"error":"invalid code"}`（历史拼写，email/password 路径沿用同一响应体） |
| 设备/IP 锁定 | 429 | `{"error":"too many failures, device locked","code":"LOCKED"}` + `Retry-After: <秒>` |
| 体解析失败 | 400 | `{"error":"bad request"}` |
| 内部错误 | 500 | `{"error":"internal"}` |

限速：按 `ip:<host>` 与 `dev:<X-Device-Id>` 双键跟踪**连续**失败；3 次失败锁 15 分钟，1 小时无活动重置计数，成功登录清零。两键任一被锁即拒。

JWT（HS256）claims：`sub`（用户标识 = 数据隔离键）、`plan: "pro"`、`role`（`"admin"`/`"user"`，管理 API 用；**旧 token 无此 claim 视为 user**，客户端解码仅供 UI 门控，服务端才是授权边界）、`email`（展示用）、`exp`、`iat`。客户端对 JWT 只做**不验签**的 payload 解码（`jwtToUser` 读 `sub`/`email`，`jwtRole` 读 `role`），服务端是唯一信任边界。

**本项目偏离（多用户）**：凭据为每用户独立的「账号（邮箱）+ 密码」。历史沿革：曾是每用户独立登录码（服务端随机生成的高熵码），账号+密码模型落地后原码**原样作为密码**使用，零迁移；`add-user --password ""` 可建免密账号（知道邮箱即可登入，服务端启动时警告）。校验流程：SHA-256(secret) 索引 + email 定位用户 → argon2id 复核（允许自设弱密码）→ 签发 `sub=<user_id>`、`email=<用户邮箱>`、`plan=pro`、`role=<users.role>`。**TTL 缩短为 1 小时**：客户端 `AuthContext.refresh` 在过期后用持久化的账号密码静默重登（客户端补丁已实现），无需刷新端点。禁用用户返回 401 `{"error":"invalid code"}`。

### 受保护路由的鉴权失败响应

无/坏 Bearer：401 `{"error":"missing token"|"invalid token","code":"AUTH"}`。

## 2. 书库同步 /api/sync

### GET /api/sync?since=<epoch_ms>&type=books|notes|configs|stats

`since` 缺省按 0 处理。响应统一包在 `{"books":[],"notes":[],"configs":[],"statBooks":[],"statPages":[]}`（各通道只填自己的键，其余为空数组）。

- `books`（缺省 type）：`since` 转为 RFC3339 纳秒 UTC（`isoFromMs`；≤0 → `""` → 查询时取 `0001-01-01T00:00:00Z`），按 **`synced_at`（服务器接收时刻，非客户端时钟）** 升序，LIMIT 1000，**并列补全**：尾行 `synced_at` 相同的行全部返回（防分页丢行）。行体 = DB 形态 Book JSON。
- `notes`/`configs`：`since` 为 epoch ms；notes 条件 `COALESCE(updated_at,0) > ? OR COALESCE(deleted_at,0) > ?`（删除靠 deleted_at 传播），按 updated_at 升序；configs 按 `updated_at` 升序。行体 = DB 形态 JSON（注意 config 的 `updated_at` 是 **epoch ms 整数**，客户端 `new Date(x).getTime()` 对整数与 ISO 均兼容）。
- `stats`：`{"statBooks":[StatBookRow],"statPages":[StatPageRow]}`（见 §2.3），按 `updated_at_ms` 升序。
- 未知 type → 400 `{"error":"unknown type"}`。

### POST /api/sync

请求体（客户端内存形态，camelCase、时间戳为 epoch ms、`metadata` 为 JSON 对象）：
`{"books":[Book],"notes":[BookNote],"configs":[BookConfig],"statBooks":[...],"statPages":[...]}`

- 体解析失败 → 400 `{"error":"bad request","code":"VALIDATION"}`；单条 unmarshal 失败**静默跳过**；`books[].hash==""`、`notes[].id==""`、`configs[].bookHash==""` 跳过。
- 成功响应：`{}`（200）。

**服务器必须把推送记录转成 DB 形态存储**（客户端拉回时用 `transformBookFromDB` 等解析，直接存原样会让 `JSON.parse(metadata)` 崩溃）。转换规则：

| 客户端推送（camelCase） | DB 形态（snake_case） | 转换 |
|---|---|---|
| `hash` | `book_hash` | 原样；同时作主键与 `id` |
| `metaHash` | `meta_hash` | 原样 |
| `metadata`（对象） | `metadata`（字符串） | JSON 序列化；null → 缺省 |
| `createdAt/updatedAt/deletedAt/readingStatusUpdatedAt/coverUpdatedAt/metadataUpdatedAt/uploadedAt/pinnedAt`（epoch ms） | 同名 snake（`created_at` 等） | → RFC3339 UTC 字符串（config 例外：`created_at/updated_at/deleted_at` 保持 epoch ms 整数）；缺省时间戳用当前 ms |
| `title/sourceTitle/author/groupName` | 同名 snake | 去 `\u0000`（sanitize） |
| `progress/viewSettings`（对象） | 同名 snake | **book 行内为原始 JSON 值**（Go `json.RawMessage`，保持子对象形态）；config 内为 JSON 字符串（`*string`）。注意两者不同 |
| — | `user_id`、`synced_at` | 服务端填（uid；RFC3339 当前时刻） |

note/config 的字段一一对应（note: `id,type,cfi,xpointer0/1,page,text,style,color,note,global` → snake，`text/note` sanitize，时间戳保持 ms；config: `bookHash,metaHash,progress,location,xpointer,rsvpPosition,searchConfig,viewSettings,updatedAt`，子对象序列化为字符串）。

**合并语义**：
- **books 行级 LWW**：来者 `updated_at` 或 `deleted_at`（ms 整数列）更新则整行覆盖；否则走"server-wins"但仍做**字段级嫁接**：`(reading_status_updated_at→reading_status)`、`(cover_updated_at→cover_hash)`、`(metadata_updated_at→metadata)`、`(metadata_updated_at→pinned_at)` 四对，客户端字段时间戳较新则覆盖该值字段（ISO 字符串比较），多条共享同一钟时对**合并前**的服务端时间戳比较。client-wins 路径对称：保住服务端字段级时间戳 ≥ 来者的字段。`deleted_at` 传播删除。
- **notes LWW**：来者 `updated_at > 旧 || deleted_at > 旧` 才覆盖（删除传播）。
- **configs 直接覆盖**（新者胜）+ **进度搭载**：config 带 `progress` 且 `updatedAt`（ms）比已有 book 行的 `updated_at` 新时，把 progress 写进 book 行的 `progress` 并刷新其 `updated_at`/`synced_at`（对齐官方服务端，书架进度靠它更新）。
- **stats upsert**：`stat_books` 主键 `(user_id, book_hash)`，`stat_pages` 主键 `(user_id, book_hash, page, start_time)`（同页多次会话各占一行），`duration = max(旧,新)`，`updated_at_ms` 缺省/0 → 当前 ms。

## 3. 副本（CRDT）/api/sync/replicas、replica-keys

`ReplicaRow`（ wire = 存储列，无形态转换）：
`{user_id, kind, replica_id, fields_jsonb, manifest_jsonb, deleted_at_ts, reincarnation, updated_at_ts, schema_version:int}`；`fields_jsonb/manifest_jsonb` 缺省按 `"{}"` 存。

- `GET /api/sync/replicas?kind=&since=<HLC字符串>` → `{"rows":[ReplicaRow]}`，`updated_at_ts > since` 升序（HLC 字符串字典序即时序）。
- `POST /api/sync/replicas`：
  - 带 `{"rows":[...]}`：逐条 upsert（主键 `(user_id,kind,replica_id)`，`updated_at_ts` 取 `hlcMax(旧,新)`），然后**回显该用户全部副本** `{"rows":[...]}`（kind/since 全空）。
  - 带 `{"cursors":[{kind, since}]}`：批量拉取 → `{"results":[{"kind":"...","rows":[...]}]}`。
  - 都不带：`{"rows":[]}`。
- `GET /api/sync/replica-keys` → `{"rows":[{"saltId","alg","salt","createdAt"}]}`（JSON 键是 **camelCase**）。
- `POST /api/sync/replica-keys` 体 `{"alg":"..."}`（空 → 400 VALIDATION）→ 生成 `saltId=hex(8B)`、`salt=hex(32B)` → `{"row":{...}}`。
- `DELETE /api/sync/replica-keys` → 204，删该用户全部 salt。

## 4. 文件存储 /api/storage/*

**键规则（核心）**：`key = "<uid>/" + 客户端传来的 fileName 原样`。客户端 fileName 就是完整云路径（`Readest/Books/<hash>/<file>`、`Readest/Replicas/<kind>/<id>/<file>`）；下载时客户端拼 `uid + "/" + cfp` 查询——**任何前缀改写都会 404**。本地磁盘 URL 为**相对路径** `/api/storage/blob/<key>`（客户端 `toFullUrl` 负责拼 base；绝对 http(s) URL 视为外部签名 URL 不加 Bearer）。

| 端点 | 请求 | 响应 |
|---|---|---|
| `POST /api/storage/upload` | `{fileName,fileSize,bookHash?,replicaKind?,replicaId?,temp?,media?}` | `{"uploadUrl":"/api/storage/blob/<key>","downloadUrl":"..."}`（temp/media 时键为 `uid/temp/...`） |
| `PUT /api/storage/blob/<key>` | 原始字节；**先查配额**（`used + Content-Length > quota` → 413 `{"error":"storage quota exceeded","code":"QUOTA_EXCEEDED"}`） | 200；越权（非 `uid/` 前缀）403 `{"error":"forbidden","code":"AUTH"}` |
| `GET /api/storage/blob/<key>` | 支持**单段 Range**（`bytes=start-end`；多段/后缀式整对象返回）；`Accept-Ranges: bytes`，`Content-Type: application/octet-stream`；206 + `Content-Range`；越界 416 `Content-Range: bytes */<total>` | 403 空 body（越权）/404 空 body（不存在） |
| `GET /api/storage/download?fileKey=` | — | `{"downloadUrl":"..."}`；缺 fileKey 400 VALIDATION |
| `POST /api/storage/download` | `{"fileKeys":[...]}` | `{"downloadUrls":{"<key>":"<url>"}}` |
| `GET /api/storage/list` | `page,pageSize=50,sortBy=key|size|updated_at,sortOrder=asc|desc,bookHash,search` | `{"files":[{file_key,file_size,book_hash,replica_kind,replica_id,created_at,updated_at}],"total","page","pageSize","totalPages"}`（键内 `Readest/Books/<hash>/` 或旧 `books/<hash>/` 提取 book_hash；`Readest/Replicas/<kind>/<id>/` 提取副本信息；大小/时间排序平局按 key） |
| `GET /api/storage/stats` | — | `{"totalFiles","totalSize","usage","quota","usagePercentage","byBookHash":[{"bookHash","fileCount","totalSize"}]}` |
| `DELETE /api/storage/delete?fileKey=` | 越权 403 AUTH | 200（404 视为成功） |
| `DELETE /api/storage/purge` | `{"fileKeys":[...]}` | `{"success":[],"failed":[{"fileKey","error"}],"deletedCount","failedCount"}` |

磁盘安全：key 经"前缀加固 + 清洗后必须在 root 内"校验（防路径穿越）；列目录按 key 升序。`updated_at` 用文件 mtime（`2006-01-02T15:04:05Z` 格式）。
**本项目偏离**：全新部署，**省略 ywz 的旧键布局回退**（`legacyStorageKey`）；S3/Postgres 后端不做（保留 trait 便于日后扩展）。

## 5. 代理（全部需 Bearer）

| 端点 | 请求 | 行为/响应 |
|---|---|---|
| `POST /api/deepl/translate` | `{text:[...],source_lang,target_lang,use_cache}` | 有 `DEEPL_API_KEY`+`DEEPL_ENDPOINT`：转发（`Authorization: DeepL-Auth-Key <key>`，透传上游状态码与体）；否则 **mock 回显** `{"translations":[{"text":"<原文>"}]}` |
| `POST /api/yandex-translate?endpoint=session|translate&text=a&text=b&lang=..` | query 参数 | 表单转发 `translate.yandex.net/api/v1/tr.json/translate`，透传响应 |
| `ANY /api/tts/edge` | GET=列 voice：`[{name,display_name,locale,gender}]`；POST `{text,voice,rate,pitch,volume,ssml}`（voice 默认 `en-US-AriaNeural`） | 音频字节（Content-Type 上游给定，缺省 `audio/mpeg`）；SSML 模式输出 webm |
| `GET /api/metadata/search?q=` | q 缺省 400 VALIDATION | Open Library `search.json?limit=20` → `{"results":[{"title","authors":[],"cover":"https://covers.openlibrary.org/b/id/<cover_i>-L.jpg","publish_year","identifiers":{"isbn":[]}}]}` |

Edge TTS 依赖微软免费接口（websocket 协议）；Rust 实现可用等价 crate 或自实现，**响应形状不变**。

## 6. 书库层（`library` cargo feature，默认开）

书城 + 书目 + 管理页整体收在 `sync-server/src/library/`，只**读取**同步主流程的数据（books 表 JSON、files/ 目录），写仅限自己的三张表（`catalog_books`、`works`、blob 注册）。`cargo build --no-default-features` 时本节全部端点不存在（404），§0–§5 同步契约完全不受影响——解耦保证：未来可机械拆成独立二进制。

### 6.1 内容寻址存储（对 §4 契约透明）

`PUT /api/storage/blob/<key>` 落盘时对 body 算 SHA-256：权威体存 `DATA_DIR/blobs/<sha256>`（已存在**跳过写入**），`files/<uid>/...` 处改为硬链接；硬链接失败回退普通写。同字节文件全服务端只占一份磁盘；key/URL/配额记账/list/stats 均不变（硬链接 stat 报告全尺寸）。启动时 GC：`blobs/` 中链接数 < 2 的文件删除（也可经管理页手动触发）。

### 6.2 书目层（DB）

- `catalog_books(hash TEXT PK, title, author, language, work_id NULL, source_hash NULL, added_at, added_by)`：blob 上传命中 `Readest/Books/<hash>/` 键时自动登记（`added_by` = 上传 uid；元数据取自该用户 books 表 JSON 的 title/author/language，缺失退化用文件名去扩展名；后到的元数据只填空字段，不覆盖人工编辑）。
- `works(id TEXT PK, title, author)`：作品聚合层。`work_id` 相同的 catalog_books 视为同一作品的不同版本（译本/AI 翻译各自是独立 hash 行，靠 `source_hash` 指向原文形成衍生链）。

### 6.3 OPDS 1.2 书城 /opds

鉴权：HTTP Basic，**用户名 = 邮箱（账号），密码 = 账号密码**（argon2 复核；错凭据/禁用一律 401 + `WWW-Authenticate: Basic`）。与 `/api` 完全隔离，对同步数据只读。

| 端点 | 说明 |
|---|---|
| `GET /opds` | 根导航 feed：All Books（含书目总数）+ By Work |
| `GET /opds/books?page=N` | 全部书目，acquisition feed，每页 50，`rel="next"` 翻页；条目含 title/author/`dcterms:language`/封面/下载链 |
| `GET /opds/works` | 作品导航 feed：每作品一条目，链到 `/opds/work/<id>`，附版本数 |
| `GET /opds/work/<id>` | 单作品 feed：该 work_id 下全部版本（各版本独立下载链） |
| `GET /opds/search?q=` | title/author LIKE 检索（同 books 形态） |
| `GET /opds/opensearch.xml` | OpenSearch 描述（客户端目录内搜索用） |
| `GET /opds/download/<hash>` | 从任意持有者副本流式发送（跨 uid 权威出口），扩展名定 MIME（epub>pdf>mobi>…），`Content-Disposition: filename*=UTF-8''` |
| `GET /opds/cover/<hash>` | 该书任一持有者上传的 png/jpg/webp 封面 |

条目 `<id>` = `urn:readest:book:<hash>`；作品条目 `<id>` = `urn:readest:work:<id>`；已归组条目额外带 `rel="collection" href="/opds/work/<id>"`。`LIBRARY_SHARED=0` 时目录只含调用者自己上传的书（默认共享）。

### 6.4 管理页 /admin 与 /api/admin/*

鉴权**双轨**：主轨为 admin 用户登录 JWT（`role: "admin"` claim，客户端管理页 `/admin` 用它）；`ADMIN_TOKEN` env 保留为运维兜底轨（内嵌静态页/脚本）。两者任一通过即可。合法 JWT 但非 admin → 403；无/坏凭据 → 401（**未设 ADMIN_TOKEN 不再整体 404**——admin JWT 轨道始终存在，首个 admin 由 CLI 产生）。token 兜底轨比较为 SHA-256 后定长比较。

| 端点 | 说明 |
|---|---|
| `GET /admin` | 单文件静态管理页（内联 JS，token 存 localStorage；客户端管理页之外的兜底，作品级精细归组仍在此页） |
| `GET /api/admin/books` | 书目列表（含 workId/sourceHash/addedBy，及活跃持有数 `holders`——`books` 表未删除行计数） |
| `POST /api/admin/books/<hash>` | 编辑 title/author/language；`workId`（null=拆分不归组）；`sourceHash` 衍生链 |
| `GET /api/admin/books/<hash>/holders` | 持有关系：该 hash 的每条 per-user 在读书目行 `{userId, email, deleted, syncedAt}`（含已删除行、flag 标注；账号已删则 email 为 null；活跃行排前）；删书影响面视图；未知 hash 返回空数组，路径不安全 hash 400 |
| `DELETE /api/admin/books/<hash>` | 删除书目：目录行 + 全部持有者文件副本 + 封面 + 内容 blob 一并移除；hash 须过路径安全校验；未知 hash 404 |
| `GET /api/admin/works` / `POST /api/admin/works` | 作品列表（含书目数）/ 手工建作品（id=`manual-<hex4>`） |
| `POST /api/admin/enrich` | 对未归组书目逐本查 Open Library（title+author→work_id）自动归组；逐本容错，返回 `{enriched, failed[]}` |
| `POST /api/admin/gc` | 触发孤儿 blob GC，返回 `{removed}` |
| `GET /api/admin/users` | 用户列表（含 `role`、存储用量 `bytesUsed`——`files/<uid>/` 副本全尺寸累计，与配额口径一致；最后活跃 `lastSeenAt` unix 秒，登录与已鉴权同步/存储请求盖章，节流 ≤1 次/分钟，从未活跃为 null） |
| `POST /api/admin/users` | 建用户：`{"email","password"?,"role"?}`——`password` 缺省=随机生成**并在响应里仅此一次**返回，空串=免密；`role` 缺省 `user`；重复邮箱 409 |
| `DELETE /api/admin/users/<email>` | 删用户：账号 + 其全部同步数据行 + `files/<uid>/` 一并移除；随后清理无持有者的目录行；删自己的账号 400 |
| `POST /api/admin/users/<email>/password` | 重置密码：`{"password"}`（空串=免密）或缺省=随机生成仅返回一次；未知 404 |
| `POST /api/admin/users/<email>/role` | 改角色 `{"role":"admin"|"user"}`；改自己 400 |
| `POST /api/admin/users/<email>/state` | 停用·启用 `{"disabled":bool}` |
| `GET /api/admin/backup` | 完整重建备份：流式 tar（`readest.db` VACUUM INTO 快照 + `blobs/` 整目录 + `jwt_secret.key`；env 注入的 secret 无文件时以内存值入档），`Content-Disposition` 文件名 `readest-backup-<UTC时间戳>.tar`；`files/` 不打包（硬链接副本由 blobs 重物化），解包到全新 data_dir 即起、已发客户端 token 不失效；同一时刻仅允许一个备份，进行中再请求 409 |

### 6.5 元数据代理补强

`GET /api/metadata/search` 的 OL 映射额外保留 `workKey`/`editionKeys`/`language`（§5 表中形状不变，仅新增字段），供自动归组复用同一查询。

### 6.6 管理层指标快照 /api/admin/host-snapshot + 服务命令邮箱（待办⑦；⑩ 增补手动控制）

发布包里的 `readest-supervisor`（独立进程管理者，拉起并守护 sync-server + web 双服务）每 5 秒向本端点推送一份宿主/子进程指标快照（`Authorization: Bearer <ADMIN_TOKEN>`；**未设 ADMIN_TOKEN 即不推送**）。服务端原样存内存最新一份（不落库、重启即空），管理页「服务监控」标签页轮询读取。自 ⑩ 起，推送的**响应体**捎回管理页排队的 `service-command`（运维者意图，服务端只存转运、永不自生成命令）。

| 端点 | 说明 |
|---|---|
| `POST /api/admin/host-snapshot` | 提交快照（JSON 对象；非对象 400）。成功 **200**，响应体 `{"commands":[{"service":"web","command":"stop"},…]}`——待执行命令**读后清**（每条至多送达一次），无命令时为空数组。（⑩ 前的服务端返回 204 无体；⑩ 前的 supervisor 忽略响应体——双向向后兼容） |
| `GET /api/admin/host-snapshot` | `{"available":false}`（从未收到过）或 `{"available":true,"received_at":<unix秒>,"age_secs":<n>,"snapshot":{…原样}}` |
| `POST /api/admin/service-command` | 管理页按钮入队一条运维命令，body `{"service":"sync-server"\|"web","command":"start"\|"stop"\|"restart"}`；鉴权双轨同 §6.4；非法 service/command 400，成功 **204**。内存 `VecDeque`、TTL 60 秒（supervisor 重启后不重放陈旧意图）、由下一次快照推送读后清地带走 |

快照 JSON 契约（v1，字段对读取方全部可选、只增不改；⑩ 新增 `desired` 字段与 `halted` 状态）：

```json
{
  "schema": 1,
  "ts": 1760000000,
  "supervisor": { "version": "0.12.14", "pid": 1234, "uptime_secs": 600, "headless": false },
  "host": { "os": "windows", "cores": 8, "total_memory_bytes": 17000000000 },
  "services": [
    {
      "name": "sync-server",
      "state": "running",
      "desired": "run",
      "pid": 1234, "uptime_secs": 600, "restarts": 0, "consecutive_failures": 0,
      "last_exit": { "code": 1, "uptime_secs": 2 },
      "probe": { "ok": true, "status": 200, "rtt_ms": 3 },
      "memory_bytes": 45000000, "cpu_percent": 0.5,
      "circuit_reason": null
    }
  ]
}
```

`state` 取值：`starting|running|restarting|circuit open|halted|stopped`——`halted`=运维手动 Stop 压制（不自启、不计失败，`desired:"halt"`），`circuit open`=连续 5 次快速失败熔断**驻留**（等待人工 Start，supervisor 不再自动重试也不再需要重启整个 supervisor 才恢复）；`stopped`=supervisor 全局停机。`desired` 为运维意图（`run|halt`，内存态不落盘——supervisor 重启即全新意图，双服务自动回来）。`probe` 为 supervisor 对各服务**直连**（sync 打 `/api/health`、web 打 `/`）的探活结果。

**防环铁律**（supervisor 与本端点的边界，⑩ 的命令邮箱不破）：控制权只在 supervisor（服务端只存数、只转运运维者排队的命令、永不自生成、绝不自己执行）；探活直连各服务端口、绝不借道兄弟进程；推送/命令捎带失败只记日志、保留旧快照，永不参与重启决策。管理页在 `available:false` 时按「管理层未接入」降级展示，其余管理功能不受影响；管理页的 Stop 按钮对 sync-server 会先警示——停了它邮箱即熄火，「全家停」永远留在 OS 层（stop.bat/托盘退出/uninstall），这是设计而非缺陷。

### 6.7 管理页聚合统计 /api/admin/stats（管理页可观测性）

一个只读端点喂饱管理页「总览」标签页的卡片：账号/目录计数、磁盘占用、blob 去重节省、版本与运行时长。鉴权双轨同 §6.4；无状态实时统计、不缓存，客户端 60 秒轮询。

| 端点 | 说明 |
|---|---|
| `GET /api/admin/stats` | 聚合统计快照（只读；字段只增不改） |

响应形状：

```json
{
  "users": { "total": 3, "admins": 1, "disabled": 1 },
  "books": { "synced": 12, "catalog": 5, "works": 2 },
  "storage": {
    "files_bytes": 2500000,
    "blobs_bytes": 1500000,
    "blobs_count": 4,
    "db_bytes": 81920,
    "dedup_saved_bytes": 1000000
  },
  "server": { "version": "0.12.16", "uptime_secs": 3600 }
}
```

语义：`books.synced` = 各账号在读书目条目数（`books` 表未删除行）；`books.catalog` / `works` = 共享目录行数与作品数。`storage.files_bytes` 按每账号副本**全尺寸**累计（硬链接也计全尺寸，与配额口径一致）；`blobs_bytes` / `blobs_count` = 内容寻址 blob 库实际占用；**`dedup_saved_bytes` = `files_bytes` − `blobs_bytes`**，即去重省下的磁盘；`db_bytes` = SQLite 主库 + WAL/SHM 旁路文件。`server.version` = sync-server crate 版本，`uptime_secs` = 进程运行时长。

## 7. 服务端配置（本项目 env 集）

| env | 缺省 | 说明 |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | 裸 `":8080"` 自动归一化为 `0.0.0.0:8080`（裸端口经 getaddrinfo 在 Windows 会解析成单个本机地址，127.0.0.1/局域网全部拒绝连接） |
| `JWT_SECRET` | 自动生成落盘 | 未设时首启生成随机密钥写入 `<DATA_DIR>/jwt_secret.key` 并持久复用（token 跨重启不失效；删 data/ 即轮换）；显式设置须 ≥16 字符 |
| `DATA_DIR` | `<exe目录>/data` | SQLite 文件 `readest.db`、文件存储 `files/` 与去重 `blobs/` 的根（便携缺省：跟 exe 走，不占 C 盘） |
| `QUOTA_BYTES` | 2 GiB（每人） | 存储配额 |
| `MAX_BODY_BYTES` | 512 MiB | 单请求体上限（axum 默认仅 2 MiB，会拒掉整本书的 blob PUT；注意 blob 体整体进内存， audiobook 等超大文件按需调低/调高） |
| `LIBRARY_SHARED` | `1` | `0` = OPDS 目录只显示调用者自己的上传 |
| `ADMIN_TOKEN` | 空 | 运维兜底轨：设了才能用静态页 + Bearer token 直调 /api/admin；**不设也不影响** admin 用户 JWT 走客户端管理页；设了同时启用 supervisor→管理页的指标推送（§6.6） |
| `READEST_NODE` | 空 | readest-supervisor 专用：node 可执行文件路径（缺省依次找 exe 旁 `node`/`node.exe` → PATH；web 服务需要 Node ≥ 20） |
| `DEEPL_API_KEY` / `DEEPL_ENDPOINT` | 空 | 空 = mock 回显 |
| `RUST_LOG` | `info` | tracing |

ywz 的 `AUTH_CODE`/`METADATA_BACKEND`/`METADATA_DSN`/`STORAGE_KIND`/`LOCAL_ROOT`/`S3_*` 不保留；用户与密码管理走 CLI 或 admin API（§6.4）：`sync-server add-user <email> [--password <明文，缺省随机生成并打印；空串 = 免密账号>] [--admin]`、`set-role <email> <admin|user>`、`list-users`、`disable-user <email>`、`passwd <email> [--password <新密码>]`（`--code` 仍被接受为别名）。库表：`users(id TEXT PK, email TEXT UNIQUE, code_sha256 TEXT, code_argon2 TEXT, plan TEXT DEFAULT 'pro', disabled INTEGER DEFAULT 0, role TEXT DEFAULT 'user', created_at TEXT, last_seen_at INTEGER NULL)`（`code_sha256`/`code_argon2` 列名沿用登录码时代，现承载密码；`role`/`last_seen_at` 列对既有库由启动迁移自动补齐）。

## 8. 客户端依赖的隐含前提（回归清单）

1. 登录后所有 API 走 `getBaseUrl()` + `/api`，blob 相对 URL 由 `toFullUrl` 拼接且**非 http 开头必带 Bearer**。
2. JWT 过期 → `AuthContext.refresh` 用 localStorage `selfhosted_credentials`（旧 key `selfhosted_login_code` 只读回退）静默重登；服务端绝不能让 `/api/auth` 对同一设备的高频重登触发 429（成功会清零计数，天然满足）。
3. 书架进度依赖 config→book 的进度搭载；删除依赖 `deleted_at` 双通道传播（books 行级 + notes OR 条件）。
4. books 拉取游标是 `synced_at`（服务器时钟）；客户端把响应行尾 `synced_at` 转回 ms 作下一轮 `since`。
5. 副本 HLC 字符串比较即时间序，禁止解析成数值。
