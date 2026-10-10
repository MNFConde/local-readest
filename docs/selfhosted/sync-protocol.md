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
请求体：`{"code": "<登录码>"}`。

响应：
| 场景 | 状态 | 体/头 |
|---|---|---|
| 成功 | 200 | `{"access_token":"<JWT>","token_type":"bearer"}` |
| 码错误 | 401 | `{"error":"invalid code"}` |
| 设备/IP 锁定 | 429 | `{"error":"too many failures, device locked","code":"LOCKED"}` + `Retry-After: <秒>` |
| 体解析失败 | 400 | `{"error":"bad request"}` |
| 内部错误 | 500 | `{"error":"internal"}` |

限速：按 `ip:<host>` 与 `dev:<X-Device-Id>` 双键跟踪**连续**失败；3 次失败锁 15 分钟，1 小时无活动重置计数，成功登录清零。两键任一被锁即拒。

JWT（HS256）claims：`sub`（用户标识 = 数据隔离键）、`plan: "pro"`、`exp`、`iat`。客户端对 JWT 只做**不验签**的 payload 解码（`jwtToUser` 读 `sub`/`email`），服务端是唯一信任边界。

**本项目偏离（多用户）**：`code` 不再是全局共享码，而是每用户独立登录码（服务端随机生成的高熵码）。校验流程：SHA-256(code) 索引定位用户 → argon2id 复核（允许用户自设弱码）→ 签发 `sub=<user_id>`、`email=<用户邮箱>`、`plan=pro`。**TTL 缩短为 1 小时**：客户端 `AuthContext.refresh` 在过期后用持久化登录码静默重登（客户端补丁已实现），无需刷新端点。禁用用户返回 401 `{"error":"invalid code"}`。

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
| `progress/viewSettings`（对象） | 同名 snake（字符串） | JSON 序列化 |
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

## 6. 服务端配置（本项目 env 集）

| env | 缺省 | 说明 |
|---|---|---|
| `LISTEN_ADDR` | `:8080` | |
| `JWT_SECRET` | 无缺省（必填） | 必填，避免重启失效 |
| `DATA_DIR` | `./data` | SQLite 文件 `readest.db` 与文件存储 `files/` 的根 |
| `QUOTA_BYTES` | 2 GiB（每人） | 存储配额 |
| `DEEPL_API_KEY` / `DEEPL_ENDPOINT` | 空 | 空 = mock 回显 |
| `RUST_LOG` | `info` | tracing |

ywz 的 `AUTH_CODE`/`METADATA_BACKEND`/`METADATA_DSN`/`STORAGE_KIND`/`LOCAL_ROOT`/`S3_*` 不保留；用户与登录码管理走 CLI：`sync-server add-user <email> [--code <明文，缺省随机生成并打印>]`、`list-users`、`disable-user <email>`、`passwd <email>`。库表：`users(id TEXT PK, email TEXT UNIQUE, code_sha256 TEXT, code_argon2 TEXT, plan TEXT DEFAULT 'pro', disabled INTEGER DEFAULT 0, created_at TEXT)`。

## 7. 客户端依赖的隐含前提（回归清单）

1. 登录后所有 API 走 `getBaseUrl()` + `/api`，blob 相对 URL 由 `toFullUrl` 拼接且**非 http 开头必带 Bearer**。
2. JWT 过期 → `AuthContext.refresh` 用 localStorage `selfhosted_login_code` 静默重登；服务端绝不能让 `/api/auth` 对同一设备的高频重登触发 429（成功会清零计数，天然满足）。
3. 书架进度依赖 config→book 的进度搭载；删除依赖 `deleted_at` 双通道传播（books 行级 + notes OR 条件）。
4. books 拉取游标是 `synced_at`（服务器时钟）；客户端把响应行尾 `synced_at` 转回 ms 作下一轮 `since`。
5. 副本 HLC 字符串比较即时间序，禁止解析成数值。
