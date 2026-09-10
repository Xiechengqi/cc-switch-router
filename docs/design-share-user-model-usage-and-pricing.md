# 设计：Share 授权用户的按模型用量与官方价格等价

> 状态：**阶段一已落地**（schema 0039、派生价目、owner/公开 API、owner 展开表、公开价目卡、审计脚本）。阶段二（Settings 价目管理）与阶段三（按等价美元卡配额）未实施。§11 已知精度缺口仍然有效。
> 范围：仅 Router（`cc-switch-router`）。**不改动 Share Contract，不需要 Server 配合，不需要 v7。**
> 最后更新：2026-09-10。
>
> 相关文档：[ARCHITECTURE.md](../ARCHITECTURE.md) §5 数据层 / §9 前端、[PROTOCOL.md](../PROTOCOL.md) §8 探针。

---

## 0. 一句话

在每个 Share 的「编辑/查看」弹窗的**授权用户与配额**表格里，为每个授权用户增加**按模型的 input/output/cache 明细**和**按官方价格换算的等价美元**，作为**只读展示层**；不改变任何配额执行、账务或结算行为。

---

## 1. 背景与问题

### 1.1 数据底座是全的，缺的是聚合

`share_request_logs`（`schema/0001_baseline.sql:914`）逐请求存有：

| 维度 | 字段 |
|---|---|
| 归属 | `share_id`、`user_email`、`app_type`、`request_agent`、`installation_id` |
| 模型 | `model`、`request_model`、`requested_model`、`actual_model`、`actual_model_source` |
| Token 四分 | `input_tokens`、`output_tokens`、`cache_read_tokens`、`cache_creation_tokens` |
| 归一化 | `quota_tokens`、`usage_state`、`usage_revision` |
| 其他 | `status_code`、`latency_ms`、`first_token_ms`、`is_health_check`、`created_at` |

全仓 `src/store.rs` 中**没有任何一处按 `model` 聚合**。明细在库里，从未被切过这一刀。

### 1.2 现有聚合链路在哪里压扁

配额表走 `GET /v1/shares/:share_id/user-limit-status` → `store.share_user_limit_status()`（`src/store.rs:8568`）：

```
shares.user_grants_json
  └─► 每个 grant 按 token_period 求窗口 (start, end)
        └─► query_share_email_token_totals_for_windows()      src/store.rs:19538
              direct_usage: SUM(COALESCE(quota_tokens,
                                 input+output+cache_read+cache_creation))
                            GROUP BY window_key, email        ← 只有 email 一维
              └─► effective_share_user_tokens()               src/store.rs:19474
                    叠加 usageRebase 标量偏移
                    └─► ShareUserLimitStatusRow { tokensUsed: u64, percent }
```

压扁点是 `GROUP BY window_key, email` —— `model` 与四类 token 在此被 `SUM` 掉。

### 1.3 `quota_tokens` 不是加权配额

Server 侧 `domain/usage/store.rs:398`：

```rust
pub fn quota_tokens(&self) -> u64 {
    let processed = self.processed_tokens();  // input+output+cache_read+cache_creation
    if processed > 0 { processed } else { self.total_tokens.unwrap_or(0) }
}
```

即**四项无权重直和**。整条链路自始至终没有权重概念。

### 1.4 平权求和造成的错配（问题的严重性）

以 Claude Sonnet 量级的官方价位（input \$3 / cache-write \$3.75 / cache-read \$0.30 / output \$15，每 1M）：

| | 用户 A（长上下文复读） | 用户 B（纯生成） |
|---|---:|---:|
| input | 2,000 | 2,000 |
| cache_read | 150,000 | 0 |
| cache_write | 20,000 | 0 |
| output | 1,500 | 30,000 |
| **当前配额计数** | **173,500** | **32,000** |
| **官方等价** | **\$0.1485** | **\$0.4560** |

用户 B 消耗了 A 的 **3 倍**真实价值，配额上却只显示 A 的 **1/5.4** —— 错配约 **16 倍**。

根因：`cache_read` 单价是 `output` 的 **1/50**，却在 `SUM` 里 1:1 等权。而 Claude Code 流量中 cache_read 常占总 token 的 80–90%，**这不是边角情况，是主流情况**。

### 1.5 顺带发现：`capacity_usage` 分支当前恒空

migration 21 将 `capacity_request_observations` 重建为对 `gateway_request_observations` 的 VIEW，硬编码 `NULL AS user_email`（`schema/0021_physically_retire_legacy_token_market.sql:193`），注释明确 "Gateway is an observation principal, not a terminal user, and must not enter user quota/identity aggregation"。而配额 SQL 过滤 `WHERE ml.user_email IS NOT NULL AND trim(ml.user_email) != ''` → 该 CTE 恒空。

**设计意图正确、实现冗余。** 本设计的新查询**只打 `share_request_logs`**，不复制这套 `LEFT JOIN` + `CASE WHEN` 复杂度。既有分支保持不动（不在本设计范围内清理）。

---

## 2. 红线约束

以下三条来自 `ARCHITECTURE.md` §2 与 Server 的 `AGENTS.md`，**违反任何一条都会破坏既有边界**。

### R1 — 等价金额只能是展示层参考值，不得进入账务

`market_accrual_entries` / `market_service_intervals` / `market_invoices` 是**按天费率 × 健康服务时长**结算的，与 token 无关。等价金额：

- **不得**写入上述任何表
- **不得**成为出账、预警阈值或逾期判定的依据
- **不得**进入账单冻结快照或争议证据
- **不得**出现在 `market_*` 任何 API 的响应里

### R2 — 不得复活 `officialPricePercent`

该字段已退役，Server 侧 `domain/sharing/retired_fields.rs` 对 camelCase 与 snake_case **双向 fail-closed**。本设计的所有新标识符必须与之无关，且**不得进入 Share descriptor**。

### R3 — Server 不参与定价

Server README 明确：「**只统计 Token / 状态 / 延迟，不计算成本或 USD 金额**」。价目表与计算**全部留在 Router 本地**。Share Contract 保持 v6 不变。

### R4 — 公开面与 owner 面是两个不同形状的产物

等价金额**明确面向买家与未登录用户开放**（产品决策）。但公开面不是 owner 面的子集，是**另一个东西**：

| | owner 面（编辑/查看弹窗） | 公开面（市场目录） |
|---|---|---|
| 粒度 | 每个授权用户一行（`email` 为键） | **listing 级聚合，无 email** |
| 主体内容 | 该用户实际用量折算的等价美元 | **可用模型的官方价目本身**（静态） |
| 用量派生数字 | 金额 + 四分拆 | **只给构成占比**，不给折算总额（§9.2 论证） |
| 可见门槛 | Share owner | 匿名可读 |
| 计算方式 | 读时聚合 | **预物化**（§8.4） |

`src/share_market.rs:4845` 的 `retain_public_catalog` / `redact_public_subscription` 是既有的公开面收窄层——
现行公开契约里 `subscription.contacts` 是被 `clear()` 的。**按 email 的用量明细比 contacts 泄露得更多**
（不仅是谁，还有此人做了多少活、在什么时段做的），因此它绝不能走公开面。

**k-匿名下限：** listing 级聚合在活跃授权用户数 `< 3` 时不输出用量派生字段——
只有一个买家时，「聚合」就是那个买家本人的数据。价目部分不受此限（它与用量无关）。

> 定位小结：这是一个 **Router 本地的、只读的、供人理解的换算层**。owner 面回答「这个人花了我多少」，
> 公开面回答「这里能用什么模型、官方标价多少」——**后者不是前者的脱敏版本**。

---

## 3. 术语

| 术语 | 含义 |
|---|---|
| `price_key` | 规范化的定价键，如 `claude-sonnet-4-5`。多个 `actual_model` 原文可映射到同一个 key |
| `rates` | 某 `price_key` 在某生效区间内的四类单价（微美元 / 1M token） |
| `micros` | 微美元定点整数。`1 USD = 1_000_000 micros`。全链路禁用 `f64` |
| `priced` | 该聚合行是否命中价目表。未命中时**不显示金额**，标注为未定价 |
| `pricing_revision` | 生效价目集合的 SHA-256，用于审计与前端缓存失效 |
| `equivalent` | 「等价」——按官方 API 价格换算的参考金额，非应付金额 |

---

## 4. 总体设计

```
  LiteLLM model_prices_and_context_window.json   ← 上游，不入库、不 vendor（§5.3）
                  │ scripts/pricing/derive-model-prices.mjs（人工执行 + 审阅 diff）
                  ▼
                       ┌───────────────────────────────────────────┐
                       │  pricing/model-prices.json (include_str!)  │
                       │  仓库自有派生产物，随二进制编译，只读       │
                       └───────────────┬───────────────────────────┘
                                       │ 启动时 upsert source='derived'
                                       ▼
  ┌────────────────────────────────────────────────────────────┐
  │  model_price_catalog  模型级元数据（阈值 / 方向 / 生效区间）  │
  │  model_price_rates    (service_tier × context_tier) → 五类价 │
  │  model_price_aliases  (actual_model 原文 → price_key)        │
  │  管理员覆盖写 source='admin'，升级不被 derived 覆盖           │
  └───────────────┬────────────────────────────────────────────┘
                  │
                  ▼
  ┌────────────────────────────────────────────────────────────┐
  │  src/model_pricing.rs   纯函数计算内核，无 IO，可单测        │
  │  price_request(rates, token_split) -> PricedUsage           │
  └───────────────┬────────────────────────────────────────────┘
                  │
                  ▼
  ┌────────────────────────────────────────────────────────────┐
  │  store.share_user_usage_breakdown()                         │
  │  share_request_logs                                         │
  │    GROUP BY window, email, model, service_tier,             │
  │             context_tier, app_type                          │
  └───────────────┬────────────────────────────────────────────┘
                  │
                  ▼
                  ├──────────────────────────┐
                  │ owner 面（读时计算）      │ 公开面（读 rollup）
                  ▼                          ▼
  GET /v1/shares/:id/user-usage-breakdown    GET /v1/share-market/listings/:id/pricing
      鉴权同 user-limit-status，懒加载            匿名可读，listing 级，无 email
                  │                          │
                  ▼                          ▼
  ShareUserLimitsTable 可展开行          市场目录价目卡（§12.6）
      （编辑态 / 只读态复用）                 价目 + 用量构成占比，k-匿名 ≥ 3
                                             ▲
  ┌──────────────────────────────────────────┴─────────────────┐
  │  share_listing_usage_rollup   日界桶，只存 token 不存金额    │
  │  缓存语义：raw 是真相，可随时 DELETE 重建（§8.5）            │
  └────────────────────────────────────────────────────────────┘
```

**关键取舍：读时计算，不落地每请求金额。**（rollup 物化的是 token 聚合，不是金额——见 §8.5）

| | 读时计算（选用） | 写时冻结（未选） |
|---|---|---|
| Schema 改动 | 只加目录表（3 张，全新） | 需 `ALTER TABLE share_request_logs` |
| 历史数据 | 全部可算 | 既有行恒 NULL，历史永远空白 |
| 价目纠错 | 改目录即全局生效 | 需回填，且旧行已错 |
| 成本 | 每次查询一次窗口扫描 | 一次性 |

窗口扫描复用既有索引 `idx_share_request_logs_share_app_user_created`，与配额查询同量级。

**但这个论证只对 owner 面成立。** owner 面是展开时懒加载、单 Share、低 QPS；公开面（R4）是匿名可读的
市场目录页，每张卡一次窗口扫描，且是零成本可刷的入口。因此：

- **owner 面：读时计算，不物化**（前提成立）
- **公开面：阶段一即物化**（前提不成立，见 §8.5）

---

## 5. 数据层

新增 `schema/0039_model_price_catalog.sql`（baseline 已冻结，只能新增；见 `ARCHITECTURE.md` §5）。

### 5.0 为什么是两张表而不是一堆列

官方价目不是「四个单价」，而是一张**受两个维度调制的费率表**：

- **服务层级（service tier）**：standard / priority(fast) / flex。priority 约 2×、flex 约 0.5×，
  且并非统一倍率——LiteLLM 目录里 `input_cost_per_token_priority`（43 个模型）、
  `cache_read_input_token_cost_priority`（41 个）是**独立标价**，不是从标准价乘出来的。
- **上下文档位（context tier）**：超过阈值后 `input` / `output` / `cache_read` / `cache_write`
  **四类各有独立的超阈值价**（`*_cost_above_200k_tokens`），不是「输入乘 2、输出乘 1.5」两个倍率。

若把这些铺成列，是 4 类 × 3 层级 × 2 档位 = 24 列且不可扩展。因此拆成
**目录头（模型级元数据）+ 费率行（每个 `(层级, 档位)` 组合一行，固定 5 列单价）**。

> 这一节的形态是从 `proxy/TokenRouter` 的 `ModelPricing`（`internal/service/billing_service.go:89`）
> 与 LiteLLM 目录的实际字段反推出来的。早期版本只有「两个倍率」的模型来自 done-hub，
> **该模型不完整**：它漏掉了长上下文对 `cache_read` 的影响，而 cache_read 恰是 Claude Code 流量的大头。

### 5.1 表结构

```sql
CREATE TABLE model_price_catalog (
    price_key                TEXT    NOT NULL,
    effective_from           INTEGER NOT NULL,          -- unix 秒，含
    effective_to             INTEGER,                   -- unix 秒，不含；NULL = 至今
    display_name             TEXT    NOT NULL DEFAULT '',
    currency                 TEXT    NOT NULL DEFAULT 'USD',
    long_context_threshold   INTEGER,                   -- 输入 token 阈值；NULL = 无长上下文分档
    long_context_inclusive   INTEGER NOT NULL DEFAULT 0,-- 0: input > 阈值；1: input >= 阈值
    supports_cache_breakdown INTEGER NOT NULL DEFAULT 0,-- 是否存在 1h 缓存写独立价
    source                   TEXT    NOT NULL,          -- 'derived' | 'admin'
    source_note              TEXT    NOT NULL DEFAULT '',
    updated_at               INTEGER NOT NULL,
    PRIMARY KEY (price_key, effective_from),
    CHECK (currency = 'USD'),
    CHECK (source IN ('derived', 'admin')),
    CHECK (effective_to IS NULL OR effective_to > effective_from),
    CHECK (long_context_inclusive IN (0, 1)),
    CHECK (supports_cache_breakdown IN (0, 1)),
    CHECK (long_context_threshold IS NULL OR long_context_threshold > 0)
);

-- 每个 (服务层级, 上下文档位) 一行。单价单位统一为微美元 / 1M token。
CREATE TABLE model_price_rates (
    price_key                    TEXT    NOT NULL,
    effective_from               INTEGER NOT NULL,
    service_tier                 TEXT    NOT NULL,   -- 'standard' | 'priority' | 'flex'
    context_tier                 TEXT    NOT NULL,   -- 'base' | 'long'
    input_micros_per_1m          INTEGER NOT NULL,
    output_micros_per_1m         INTEGER NOT NULL,
    cache_read_micros_per_1m     INTEGER NOT NULL,
    cache_write_5m_micros_per_1m INTEGER NOT NULL,
    cache_write_1h_micros_per_1m INTEGER,            -- NULL = 上游未标 1h 价
    PRIMARY KEY (price_key, effective_from, service_tier, context_tier),
    FOREIGN KEY (price_key, effective_from)
        REFERENCES model_price_catalog(price_key, effective_from) ON DELETE CASCADE,
    CHECK (service_tier IN ('standard', 'priority', 'flex')),
    CHECK (context_tier IN ('base', 'long')),
    CHECK (input_micros_per_1m          >= 0),
    CHECK (output_micros_per_1m         >= 0),
    CHECK (cache_read_micros_per_1m     >= 0),
    CHECK (cache_write_5m_micros_per_1m >= 0),
    CHECK (cache_write_1h_micros_per_1m IS NULL OR cache_write_1h_micros_per_1m >= 0)
);

CREATE INDEX idx_model_price_catalog_lookup
    ON model_price_catalog(price_key, effective_from DESC);

CREATE TABLE model_price_aliases (
    pattern     TEXT    NOT NULL,          -- 已 lower + trim
    match_kind  TEXT    NOT NULL,          -- 'exact' | 'prefix'
    price_key   TEXT    NOT NULL,
    priority    INTEGER NOT NULL DEFAULT 0,-- 大者先匹配
    source      TEXT    NOT NULL,
    updated_at  INTEGER NOT NULL,
    PRIMARY KEY (pattern, match_kind),
    CHECK (match_kind IN ('exact', 'prefix')),
    CHECK (source IN ('derived', 'admin')),
    CHECK (pattern = lower(trim(pattern))),
    CHECK (pattern != '')
);

CREATE INDEX idx_model_price_aliases_priority
    ON model_price_aliases(priority DESC, match_kind, pattern);
```

`cache_write_1h_micros_per_1m` 当前**不参与主计算**（token 侧无 5m/1h 拆分，见 §11.1），保留它有两个用途：

1. 前向兼容——若将来走通 v7 携带拆分，目录已就绪，无需再迁移
2. **计算等价金额的上界**（§7.5）：把全部 cache write 按 1h 计价得到 `upperBound`，
   与按 5m 计价的主值一起返回，让 UI 能显示「$9.80 ~ $11.30」这样的区间，
   把估算不确定性**量化并可见**，而不是藏在一条脚注里

### 5.2 费率解析的回退链

理想情况下派生脚本已把上游有的组合全部物化，运行时总是精确命中。但管理员覆盖与上游缺档会造成空洞，
因此运行时按固定顺序回退，**并记录实际命中的是哪一档**：

| 顺序 | 命中 | note |
|---:|---|---|
| 1 | `(tier, ctx)` | — |
| 2 | `(tier, base)` | `contextTierFellBack`（该层级无长上下文档，视为不分档） |
| 3 | `('standard', ctx)` | `serviceTierFellBack`（该层级无独立价，按标准价） |
| 4 | `('standard', 'base')` | 两者都回退 |

**层级回退优先于档位回退**（2 先于 3）：某模型有 priority 价但无 priority 长上下文价，
通常意味着 priority 定价本身不分档，此时用 `(priority, base)` 比用 `(standard, long)` 更接近真实。

回退命中时**照常出金额**，但在该行 `notes` 里标出，UI 以次级样式提示。这与 §6.4「未定价必须可见」
是同一姿态的不同强度：完全无价 → 不出金额；有价但回退 → 出金额 + 标注。

### 5.3 目录来源：由 LiteLLM 派生，而非 vendor

价目的权威来源是 LiteLLM 社区维护的 `model_prices_and_context_window.json`（MIT，上游千余条）。
它已经覆盖了本设计需要的全部维度——四类单价、priority/flex 层级价、`above_200k_tokens` 档位价、
`above_1hr` 缓存写价——手工逐条核对官网（约 200 模型 × 12 字段）不现实。

**但本仓政策禁止直接 vendor 该文件。** `SOURCE_PROVENANCE.json` 的 policy 为
`runtimeAndBuildInputsMustBeRepositoryOwned: true`，且 `scripts/audit/audit-source-provenance.mjs:108`
硬性要求每个 vendored source 声明 `technicalInput: false`：

```js
if (entry.technicalInput !== false) {
  violations.push(`SOURCE_PROVENANCE.json: ${entry.id} must declare technicalInput=false`);
}
```

驱动计算的价目表显然是 technical input；provider 图标能通过是因为它们是惰性展示资产。

**因此采用「派生而非搬运」**，与 `cc-switch-server/src/proxy/cache_injector.rs` 文件头那套
「repository-owned code with in-module tests + Historical adaptation attribution」是同一做法：

```
scripts/pricing/derive-model-prices.mjs
  输入：本地临时下载的上游 JSON（不入库、不提交）
  裁剪：只保留本仓实际服务的模型（按 app_type allowlist）
  转换：LiteLLM 字段 → 本仓 schema 字段，USD/token 浮点 → 微美元/1M 整数
  输出：pricing/model-prices.json  ← 仓库自有产物，人工 review diff 后提交
```

产物经 `include_str!("../pricing/model-prices.json")` 编译进二进制
（同 `regions`（`src/api.rs:124`）、`schema/*.sql`（`src/schema.rs:12`）的既有约定）。
`SOURCE_PROVENANCE.json` 增加一条 historical attribution 条目记录出处与上游 commit，
但产物本身是仓库自有代码，不计为 vendored technical input。

**字段映射：**

| LiteLLM 字段 | → `(service_tier, context_tier)` | 目标列 |
|---|---|---|
| `input_cost_per_token` | `(standard, base)` | `input` |
| `output_cost_per_token` | `(standard, base)` | `output` |
| `cache_read_input_token_cost` | `(standard, base)` | `cache_read` |
| `cache_creation_input_token_cost` | `(standard, base)` | `cache_write_5m` |
| `cache_creation_input_token_cost_above_1hr` | `(standard, base)` | `cache_write_1h` |
| `input_cost_per_token_priority` | `(priority, base)` | `input` |
| `output_cost_per_token_priority` | `(priority, base)` | `output` |
| `cache_read_input_token_cost_priority` | `(priority, base)` | `cache_read` |
| `*_cost_per_token_flex` | `(flex, base)` | 同上 |
| `input_cost_per_token_above_<N>k_tokens` | `(standard, long)` | `input` |
| `output_cost_per_token_above_<N>k_tokens` | `(standard, long)` | `output` |
| `cache_read_input_token_cost_above_<N>k_tokens` | `(standard, long)` | `cache_read` |
| `cache_creation_input_token_cost_above_<N>k_tokens` | `(standard, long)` | `cache_write_5m` |
| 上述 key 中的 `<N>` | 目录头 | `long_context_threshold = N * 1000` |

**单位换算必须精确。** LiteLLM 存的是 USD/token 浮点（如 `3e-06`），本仓存微美元/1M 整数：

```
micros_per_1m = round_half_up(Decimal(usd_per_token_string) * 10^12)
```

脚本**用十进制字符串解析，不经 JS `Number`**（`3e-06 * 1e12` 在二进制浮点下不保证得到整数 3000000）。
审计脚本断言反向换算落在容差内。

**阈值方向按厂商而异：** 多数厂商是「严格大于」，xAI 系为「达到即适用」
（对应 TokenRouter 的 `LongContextThresholdInclusive`）。派生脚本按 `litellm_provider`
设置 `long_context_inclusive`，未知 provider 默认 0（严格大于）。

### 5.4 启动期装载

在 schema 迁移完成后、后台任务拉起前：

1. 解析 `pricing/model-prices.json`，失败则**启动失败**（与 baseline checksum 一致的 fail-closed 姿态）
2. 在单个 `BEGIN IMMEDIATE` 事务内 upsert 所有 `source='derived'` 的目录头、费率行与别名
3. **不触碰任何 `source='admin'` 行** —— 管理员覆盖跨版本保留
4. 计算 `pricing_revision = SHA-256(规范化后的生效价目集合)`，存进程内原子快照

## 6. 模型身份解析

### 6.1 按哪个字段定价

**`actual_model` 优先，`model` 兜底。**

「等价」问的是「若按官方价支付**实际执行**的这次推理，需要多少钱」。跨协议 adapter 下（Claude Code → Codex/Gemini/Cursor/Kiro 上游），`requested_model` 是 `claude-sonnet-4-5`，`actual_model` 才是真正跑的 `gpt-5-codex` —— 应按后者计价。

解析顺序：

```
lower(trim(actual_model))  非空 → 用之
  否则 lower(trim(model))  非空 → 用之
  否则                          → unpriced
```

### 6.2 别名匹配

1. `exact` 命中优先于 `prefix`
2. 同类中 `priority` 大者优先
3. 同 priority 的 `prefix` 中**最长 pattern 优先**（避免 `claude-` 抢走 `claude-sonnet-4-5`）
4. 全部未命中 → `priced = false`

> 明确**不做** new-api 式的 `strings.Contains` 模糊猜族（`setting/ratio_setting/model_ratio.go:483,525`）。猜错的价格比没有价格更有害。

### 6.3 `actual_model_source` 的作用

该字段（Server 侧取值见 `proxy/provider_ops.rs`、`proxy/usage.rs`、`proxy/cursor/mod.rs`）**不参与选键**，只用于两件事：

| source | 含义 | 用途 |
|---|---|---|
| `response` | 上游响应回填 | 可信度最高 |
| `request` | 请求原样 | 可信 |
| `claude_context_1m_suffix` | 1M 上下文变体 | **提示长上下文分档**（实际判定仍按 §7.2 的 token 阈值） |
| `runtime_plan_single_model`、`kimi_model_allowlist`、`grok_model_normalization`、`cursor_model_resolution`、`cursor_explicit_selector` | 运行时改写 | 需目录显式覆盖，否则易 unpriced |
| 空 | 未知 | 照常按 §6.1 解析 |

### 6.4 未定价必须可见

未命中价目表的模型：

- **绝不静默按 \$0 计**
- 在明细中单独成行，金额列显示 `—` 并挂警示 Chip
- 行级汇总暴露 `pricedCoveragePercent`（已定价 token 数 / 总 token 数）

> 一个偏低但看起来确定的数字，比没有数字更糟。

---

### 6.5 服务层级解析

`share_request_logs` 已有三列服务层级信息，且**确实在产生数据**：
`cc-switch-server/src/proxy/codex_request_policy.rs:97` 在 fast mode 开启时写入
`effective_service_tier = "priority"`，经 `state.rs:22140` → `store.rs:15627` 落库。

priority 层级在官方价目里约为标准价的 2×（flex 约 0.5×）。**忽略这一维会对 priority 流量系统性低估约一半。**

解析顺序：

```
lower(trim(effective_service_tier))  非空 → 用之
  否则 lower(trim(client_service_tier)) 非空 → 用之
  否则                                      → 'standard'
```

归一化映射：

| 原始值 | → | 说明 |
|---|---|---|
| `priority`、`fast` | `priority` | 两种叫法指同一层级 |
| `flex` | `flex` | |
| `standard`、`default`、`auto`、空 | `standard` | |
| 其他未知值 | `standard` | 附 `unknownServiceTier` note，不猜 |

> 用 `effective_*` 而非 `client_*`：与 §6.1 选 `actual_model` 同理——计价要对齐**实际执行**的层级，
> 而非客户端请求的层级。`service_tier_decision` 记录了改写原因，不参与选价，仅供排查。

## 7. 计算内核

新增 `src/model_pricing.rs`。纯函数、无 IO、可单测 —— 对齐
`proxy/TokenHub/backend/internal/metering/pricing.go` 的分包哲学
（"implements exact prices independently of transport and storage"）。

### 7.1 类型

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ServiceTier { Standard, Priority, Flex }

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ContextTier { Base, Long }

/// 单个 (service_tier, context_tier) 组合的完整费率，微美元 / 1M token
pub struct RateSet {
    pub input_micros_per_1m: i64,
    pub output_micros_per_1m: i64,
    pub cache_read_micros_per_1m: i64,
    pub cache_write_5m_micros_per_1m: i64,
    pub cache_write_1h_micros_per_1m: Option<i64>,
}

/// 一个模型在某生效区间内的全部费率与元数据
pub struct ModelPrice {
    pub price_key: String,
    pub display_name: String,
    pub long_context_threshold: Option<u64>,
    pub long_context_inclusive: bool,
    pub rates: BTreeMap<(ServiceTier, ContextTier), RateSet>,
}

pub struct TokenSplit {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,   // 合并值；5m/1h 不可分，见 §11.1
}

pub enum PriceLineKind { Input, Output, CacheRead, CacheWrite }

pub struct PriceLine {
    pub kind: PriceLineKind,
    pub tokens: u64,
    pub rate_micros_per_1m: i64,
    pub amount_micros: i128,
}

pub struct PricedUsage {
    pub lines: Vec<PriceLine>,
    pub total_micros: i128,
    /// 全部 cache write 按 1h 计价的上界；无 1h 价时等于 total_micros
    pub upper_bound_micros: i128,
    pub service_tier: ServiceTier,
    pub context_tier: ContextTier,
    pub notes: Vec<PricingNote>,
}

/// 按回退链（§5.2）取费率，并记录实际命中档位
pub fn resolve_rates(
    price: &ModelPrice,
    tier: ServiceTier,
    ctx: ContextTier,
) -> (&RateSet, Vec<PricingNote>);

pub fn price_usage(
    price: &ModelPrice,
    tier: ServiceTier,
    split: &TokenSplit,
    representative_input_tokens: u64,   // 用于长上下文判定，见 §7.3
) -> PricedUsage;
```

### 7.2 规则

1. **全程 `i128` 微美元定点，禁用 `f64`。** 金额只在序列化边界转为字符串
   > 这是与 `proxy/TokenRouter` 的有意分歧：它用 `float64` 计算、靠 Postgres `decimal(20,10)` 存储兜底。
   > libSQL 无 decimal 类型，浮点误差无处可藏，故走定点。
2. 逐项：`amount_micros = rate_micros_per_1m * tokens / 1_000_000`，向零截断
3. **长上下文不是倍率，是整套替代费率。** 判定命中后，`input`/`output`/`cache_read`/`cache_write`
   四类**全部**切到 `context_tier = Long` 的 `RateSet`，而不是只对 input/output 乘系数
4. 阈值方向由 `long_context_inclusive` 决定：`false` → `input > threshold`；`true` → `input >= threshold`
5. `tokens == 0` 的类别不产生 line
6. 保留逐项 `lines`，前端 tooltip 直接消费，**不在前端二次计算**

### 7.3 长上下文标记：看结果，不看条件

长上下文命中不等于「更贵」。缓存命中率高时，切到 Long 费率后总额可能反而下降或持平
（`cache_read` 的 Long 价虽高于其 Base 价，但仍远低于 input 价）。

因此 `longContextApplied` 的判定**算两遍再比较**，而非「输入超阈值即标记」：

```rust
let base = price_with(rates_at(tier, ContextTier::Base), split);
let long = price_with(rates_at(tier, ContextTier::Long), split);
let applied = long.total_micros > base.total_micros;
```

> 直接借鉴 `proxy/TokenRouter` 的
> `bd.LongContextBillingApplied = baselineCost != nil && bd.ActualCost > baselineCost.ActualCost`
> （`internal/service/billing_service.go:1463`）。命中档位仍按规则 4 选定，
> 该标志只影响 UI 是否提示「本行含长上下文计价」。

### 7.4 聚合顺序与代表性输入

**先按请求定价，再按模型求和。** 不可先把 input 求和再判阈值——否则一组小请求的输入总和会被误判为长上下文。

`representative_input_tokens` 指**单条请求的 input**。§8.2 的分档预聚合保证同一组内所有请求的档位判定一致，
故组内先求和再按该组档位定价与逐请求定价结果等价。

### 7.5 估算上界

`cache_write_5m` 与 `cache_write_1h` 的 token 无法拆分（§11.1），主值按 5m 计。同时计算上界：

```
upper_bound_micros = total_micros
                   - cache_write_tokens * rate_5m / 1_000_000
                   + cache_write_tokens * rate_1h / 1_000_000
```

无 1h 价时 `upper_bound_micros == total_micros`。以 Sonnet 4.5 为例
（5m = 3.75e-6、1h = 6e-6 USD/token）1h 为 5m 的 **1.6 倍**，故上界仅在 cache write 一项上浮 60%，
落到总额通常是个位数百分比。UI 据此显示区间而非单点，把不确定性量化。

### 7.6 明细与配额列的口径对账

`quota_tokens()`（Server 侧 `domain/usage/store.rs:398`）在四项之和为 0 时**回落到 `total_tokens`**：

```rust
let processed = self.processed_tokens();
if processed > 0 { processed } else { self.total_tokens.unwrap_or(0) }
```

因此存在一类行：`quota_tokens > 0` 但 `input/output/cache_read/cache_creation` 全为 0
（上游只报了一个总数、未给分项）。这些 token **进了配额列，却无法进入任何模型明细**。

处理：单独归入 `unattributed` 桶，**计入 token 合计、不计入等价金额**，并在 UI 明示：

```
未归因 1.2M —— 上游仅返回总量、未提供分项，无法计价
```

> 借鉴 `proxy/TokenRouter` 的 `normalizeCacheCreationBreakdown`（`billing_service.go:1485`）
> 所体现的原则——**明细与聚合冲突时必须显式对账，不能让差额悄悄消失**。
> 它面对的是「拆分之和 > 聚合值」而按比例封顶；我们面对的是「拆分全为 0 而聚合值 > 0」，
> 结论同样是：让差额可见，而不是让两个数字对不上却无人解释。

## 8. 聚合查询

在 `src/store.rs` 新增 `share_user_usage_breakdown()`。

### 8.1 窗口对齐

**必须与配额表使用完全相同的窗口**（`token_period_window()` + `quota_window_key()`，`src/store.rs:8613-8622`），否则两个数字对不上，会立刻被质疑为 bug。

### 8.2 分档预聚合：三维分组

§7.4 要求逐请求定价，但逐请求拉取在长窗口下行数过大。解法是把**定价档位提升为分组维度**，
使同组内所有请求适用同一 `RateSet`，组内先求和再定价与逐请求定价结果等价。

分组键为 `(model_key, service_tier, context_tier, app_type)`：

```sql
SELECT
  lower(COALESCE(NULLIF(trim(actual_model), ''), model))          AS model_key,
  lower(COALESCE(NULLIF(trim(effective_service_tier), ''),
                 NULLIF(trim(client_service_tier), ''), 'standard')) AS tier_raw,
  CASE
    WHEN :threshold IS NULL THEN 0
    WHEN :inclusive = 1 AND input_tokens >= :threshold THEN 1
    WHEN :inclusive = 0 AND input_tokens >  :threshold THEN 1
    ELSE 0
  END                                                            AS long_ctx,
  app_type,
  SUM(input_tokens), SUM(output_tokens),
  SUM(cache_read_tokens), SUM(cache_creation_tokens),
  SUM(CASE WHEN COALESCE(input_tokens,0) + COALESCE(output_tokens,0)
              + COALESCE(cache_read_tokens,0) + COALESCE(cache_creation_tokens,0) = 0
           THEN COALESCE(quota_tokens, 0) ELSE 0 END)             AS unattributed_tokens,
  COUNT(*)                                                        AS request_count,
  SUM(CASE WHEN usage_state != 'observed' THEN 1 ELSE 0 END)       AS estimated_count
FROM share_request_logs
WHERE share_id = :share_id
  AND is_health_check = 0
  AND user_email IS NOT NULL AND trim(user_email) != ''
  AND lower(trim(user_email)) = :email
  AND (:start IS NULL OR created_at >= :start)
  AND (:end   IS NULL OR created_at <  :end)
GROUP BY model_key, tier_raw, long_ctx, app_type
```

`tier_raw` 的归一化（`fast` → `priority`、未知 → `standard`）在 Rust 侧做，不写进 SQL——
映射表会演进，放在可单测的代码里比嵌在 SQL 字符串里安全。

`:threshold` / `:inclusive` 依赖具体模型的目录条目，因此分两趟：

1. 轻量 `GROUP BY model_key` 取出该窗口出现过的模型集合
2. 解析 price_key、取阈值与方向；**有阈值的模型**跑上面的三维聚合，
   **无阈值的模型**（多数）`:threshold` 传 NULL，`long_ctx` 恒 0，退化为两维

`unattributed_tokens` 对应 §7.6 —— 只在四项全零时才把 `quota_tokens` 计入，避免与正常行重复计数。

### 8.3 时间版本化价格

窗口可能跨越价格生效边界。**阶段一简化：按窗口结束时刻（或 `now`，取较早者）的生效价格统一计算**，并在响应中回传 `pricingRevision`。

理由：配额窗口最长为「自然月 / 每 30 天」，价格在单个窗口内变动是罕见事件；而 §8.2 的分组键已有三维，再切一维会让组数继续膨胀。若后续出现真实的窗口内调价，再引入按 `effective_from` 分桶的第四维。**此简化必须在 API 文档与 UI tooltip 中明示。**

### 8.4 只打 `share_request_logs`

见 §1.5。不引入 `capacity_request_observations` 分支。

### 8.5 公开面必须物化（阶段一）

原设计「阶段一不做物化缓存」的理由是 **只有 owner 会打开这个弹窗**。R4 之后该前提消失：
市场目录对未登录用户渲染，列表页每张卡都需要该 listing 的用量构成。**实时聚合不可接受**——
既是 N+1 次窗口扫描，也是一个无需认证即可放大的查询入口。

新增 `share_listing_usage_rollup`（`schema/0039_*.sql` 同一迁移内）：

```sql
CREATE TABLE share_listing_usage_rollup (
    share_id            TEXT    NOT NULL,
    bucket_start        INTEGER NOT NULL,     -- UTC 日界
    model_key           TEXT    NOT NULL,
    service_tier        TEXT    NOT NULL,
    context_tier        TEXT    NOT NULL,
    input_tokens        INTEGER NOT NULL DEFAULT 0,
    output_tokens       INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens   INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens  INTEGER NOT NULL DEFAULT 0,
    unattributed_tokens INTEGER NOT NULL DEFAULT 0,
    distinct_users      INTEGER NOT NULL DEFAULT 0,   -- k-匿名判定用
    request_count       INTEGER NOT NULL DEFAULT 0,
    computed_at         INTEGER NOT NULL,
    PRIMARY KEY (share_id, bucket_start, model_key, service_tier, context_tier)
);
```

**注意存的是 token,不是金额。** 价目可能被管理员改,金额必须保持读时计算——
物化的只是聚合代价高的那一半。这样「改一次价目、历史全局纠错」的性质不受影响。

**降级契约（照搬 TokenRouter migration `232`）：**

1. **rollup 是缓存,`share_request_logs` 是真相。** 任何时候可 `DELETE` 全表并重建
2. 重建是幂等的纯函数:`(share_id, bucket)` → 一组行,不依赖 rollup 现值
3. rollup 缺失某桶时,公开面**降级为不展示该字段**,不回退到实时扫描
   (回退实时扫描等于把被防住的负载又放回来了)
4. owner 面**永不读 rollup**,始终走 raw——owner 要的是精确值和当前窗口

由既有后台任务节律驱动增量重算,日界桶,不追求秒级新鲜度。

---

## 9. API 契约

两个端点，形状不同，依据 R4。

### 9.1 owner 面：按用户明细

**不改动 `user-limit-status` 的任何现有字段** —— 它被配额执行链路依赖，改动风险高。新增独立端点。

```
GET /v1/shares/:share_id/user-usage-breakdown[?email=<addr>]
```

- 鉴权与 `user-limit-status` 完全一致（`src/api.rs:446`）
- 省略 `email` 时返回该 Share 全部 active grant；带 `email` 时只返回一行（展开时懒加载走这条）

```jsonc
{
  "shareId": "shr_...",
  "pricingRevision": "sha256:a3f2c1...",
  "pricedAt": 1757462400,
  "rows": [{
    "email": "zhang@example.com",
    "windowStartsAt": "2026-09-01T00:00:00Z",
    "resetsAt": "2026-10-01T00:00:00Z",
    "rebaseApplied": true,                       // ← 见 §10
    "observedTotals": {
      "input": 2100000, "output": 1800000,
      "cacheRead": 98200000, "cacheWrite": 12100000,
      "unattributed": 1200000,                   // ← §7.6，计入 total 不计入金额
      "total": 115400000
    },
    "equivalentUsdMicros": "12400000",           // 主值：cache write 按 5m
    "equivalentUsdMicrosUpperBound": "13120000", // 上界：cache write 全按 1h（§7.5）
    "pricedCoveragePercent": 97.3,
    "estimatedRequestPercent": 1.2,
    "byModel": [{
      "modelKey": "claude-sonnet-4-5-20250929",
      "priceKey": "claude-sonnet-4-5",
      "displayName": "Claude Sonnet 4.5",
      "appType": "claude",
      "serviceTier": "standard",                 // standard | priority | flex
      "contextTier": "base",                     // base | long
      "input": 2100000, "output": 1800000,
      "cacheRead": 98200000, "cacheWrite": 12100000,
      "total": 114200000,
      "requestCount": 4821,
      "priced": true,
      "equivalentUsdMicros": "9800000",
      "equivalentUsdMicrosUpperBound": "10520000",
      "lines": [
        { "kind": "input",     "tokens": 2100000,  "rateMicrosPer1M": 3000000, "amountMicros": "6300000" },
        { "kind": "cacheRead", "tokens": 98200000, "rateMicrosPer1M": 300000,  "amountMicros": "29460000" }
      ],
      "notes": ["cacheWriteAssumed5m"]
    }, {
      "modelKey": "gpt-5-codex",
      "priceKey": "gpt-5-codex",
      "appType": "codex",
      "serviceTier": "priority",                 // ← fast mode 归一化后
      "contextTier": "base",
      "input": 1200000, "output": 900000,
      "cacheRead": 0, "cacheWrite": 0,
      "total": 2100000,
      "requestCount": 133,
      "priced": true,
      "equivalentUsdMicros": "4260000",
      "equivalentUsdMicrosUpperBound": "4260000",
      "lines": [ /* ... */ ],
      "notes": []
    }, {
      "modelKey": "some-unknown-model",
      "priceKey": null,
      "appType": "claude",
      "serviceTier": "standard",
      "contextTier": "base",
      "input": 400000, "output": 120000,
      "cacheRead": 0, "cacheWrite": 0,
      "total": 520000,
      "requestCount": 12,
      "priced": false,
      "equivalentUsdMicros": null,
      "equivalentUsdMicrosUpperBound": null,
      "lines": [],
      "notes": ["priceKeyNotFound"]
    }]
  }]
}
```

约定：

- **所有金额为字符串**，单位微美元。前端只做展示格式化，不做算术
- `priced: false` 时两个金额字段均为 `null`，**绝不为 `"0"`**
- `byModel` 按 `total` 降序。同一 `modelKey` 在不同 `(serviceTier, contextTier)` 下**是不同的行**——
  这是刻意的：priority 与 standard 单价差约 2×，合并展示会掩盖成本来源
- `notes` 为稳定枚举串，前端映射 i18n；未知 note 静默忽略（前向兼容）

**`notes` 枚举：**

| note | 含义 | UI 强度 |
|---|---|---|
| `priceKeyNotFound` | 未命中价目表 | 警示 Chip，不出金额 |
| `cacheWriteAssumed5m` | 缓存写入按 5m 估算（§11.1） | 脚注 |
| `longContextApplied` | 长上下文费率实际抬高了成本（§7.3） | 行内标记 |
| `serviceTierFellBack` | 该层级无独立价，按标准价（§5.2） | 次级提示 |
| `contextTierFellBack` | 该层级无长上下文档，按不分档处理（§5.2） | 次级提示 |
| `unknownServiceTier` | 层级取值未知，按 standard 处理（§6.5） | 次级提示 |
| `usageStateNotFullyObserved` | 非 observed 行占比 > 5%（§11.3） | 行级脚注 |

### 9.2 公开面：listing 级，价目优先于折算额

```
GET /v1/share-market/listings/:id/pricing        # 匿名可读
```

```jsonc
{
  "listingId": "lst_...",
  "catalogRevision": "a3f2c1",

  // ① 主体：该 listing 可用模型的官方价目。静态，与用量无关，可长缓存
  "models": [
    {
      "modelKey": "claude-sonnet-4-5",
      "displayName": "Claude Sonnet 4.5",
      "rates": {                                 // 微美元 / 1M token，字符串
        "input": "3000000", "output": "15000000",
        "cacheRead": "300000", "cacheWrite5m": "3750000"
      },
      "longContextThreshold": 200000
    }
  ],

  // ② 次要：近 30 天用量构成占比。k-匿名不足时整块为 null
  "usageMix": {
    "windowDays": 30,
    "composition": { "input": 0.09, "output": 0.04,
                     "cacheRead": 0.83, "cacheWrite": 0.04 },
    "modelShare": [ { "modelKey": "claude-sonnet-4-5", "share": 0.91 } ],
    "staleAfter": 1757462400
  }
}
```

**不返回等价美元总额、不返回「每 1M 配额值多少美元」的密度。** 理由：

同一上游模型在不同 listing 上**官方价目是同一份**。因此 listing 之间折算密度的差异，
几乎全部来自**过去那批买家的用量结构**，而非 listing 自身属性。而按 §1 的配额口径，
`output / cacheRead = 50×`，实测两种典型负载相差可达 **21×**——把一个 21 倍散布的分布
压成一个点估计发布出去，买家横向比较的是别人的工作负载，不是自己能拿到什么。

**能公开的真属性是价目本身**：只放 Haiku 的 listing 与放 Opus 的 listing，价值差异是真的、
稳定的、可比的。`usageMix` 保留，因为它能证明「这个号是活的、真在被使用」——但它是
**构成**不是**结论**，且必须配文案说明结果取决于买家自身用法。

约束：

- `usageMix` 读 `share_listing_usage_rollup`（§8.5）；rollup 缺失则整块 `null`，**不回退实时扫描**
- `distinct_users < 3` 时 `usageMix` 为 `null`（R4 的 k-匿名下限）
- 响应中**不含任何 email、用户数、请求数绝对值**——只有占比
- 该端点必须经 `retain_public_catalog` 同源的 `publicly_listed` 判定；未公开 listing 返回 404 而非 403
- 可 `Cache-Control` 长缓存；`catalogRevision` + `staleAfter` 用于失效

## 10. usageRebase 口径（口径 A）

`effective_share_user_tokens()`（`src/store.rs:19474`）允许 owner 将某用户已用量**重基线**到任意值。该偏移是**纯标量、无模型归属**，因此：

- `byModel` 之和 **≠** 配额列的 `tokensUsed`（存在 rebase 时）
- 等价金额**无法**按 rebase 后的值计算（不知该扣哪个模型的哪类 token）

**采用口径 A：明细与等价一律基于实测 observed 值；存在 rebase 时在 UI 显式标注。**

```
配额列（不变）  = effective_share_user_tokens(...)   ← 含 rebase
明细 / 等价     = observed 实测值                     ← 不含 rebase
```

响应中 `rebaseApplied: true` 时，UI 在展开区顶部渲染一条提示：

> 该用户的配额已被重基线，下方明细与等价为**实测值**，与上方配额列不一致属预期。

**未采用的方案与理由**：

| 方案 | 做法 | 否决理由 |
|---|---|---|
| B | 按各模型 token 占比等比缩放 rebase 差额 | 数字对得上，但**编造归因** —— 等价金额会失真，且失真不可见 |
| C | 有 rebase 时不显示等价 | rebase 使用频率若不低，功能大面积失效 |

口径 A 与仓库既有姿态一致：`OperationVerification` 区分「API 提交成功」与「Dashboard 已观察到生效」、Gateway 在 grant contract 未成形时整体 fail-closed —— 一贯选择**诚实暴露不确定性**而非凑一个好看的数。

---

## 11. 已知精度缺口

必须在 API `notes` 与 UI 中明示，不得隐藏。

### 11.1 `cache_creation_tokens` 无 5m/1h 拆分

Claude 的 1h cache write 单价为 5m 的 **1.6 倍**（1h = 2.0× 基础输入价，5m = 1.25× 基础输入价；
以 Sonnet 4.5 为例 6e-6 vs 3.75e-6），但全链路只有一个合并标量。信息在三处丢失/受阻：

**(a) Server 解析时未读嵌套对象。** `domain/usage/store.rs:1275-1305` 的 `first_u64` 候选列表只含标量键名
（`cache_creation_input_tokens` 等）与三条 `*_details/*` 指针回落，**没有一条指向
`/cache_creation/ephemeral_5m_input_tokens` 或 `/ephemeral_1h_input_tokens`**。原始数据存在，只是未被解析。

> 该嵌套对象的存在已由第三方实现佐证：`proxy/TokenRouter` 的 SSE 夹具
> `internal/service/antigravity_gateway_service_test.go:1657` 为
> `{"usage":{"input_tokens":100,"cache_creation":{"ephemeral_5m_input_tokens":30,"ephemeral_1h_input_tokens":70}}}`，
> 且其 `usage_logs` 表落有 `cache_creation_5m_tokens` / `cache_creation_1h_tokens` 两列
> （`backend/ent/schema/usage_log.go`）。

**(b) 契约无槽位且 `deny_unknown_fields`。** `src/models.rs:1129` 为单标量 `cache_creation_tokens: u32`；
Router 全仓对 `ephemeral_*` / `ttl` 匹配数为 0。`models.rs` 与 Server `router_contract.rs`、`usage/store.rs`
**全线 `deny_unknown_fields`** —— 新增字段会被旧端直接拒收，**必须走 v7 + 版本协商门控发送**。

**(c) 单请求内 TTL 可合法混合 —— 配置反推路径被此条否决。** 曾考虑从 provider 的
`cacheInjection.ttl`（`proxy/adapters.rs:3337`，且 `cache_injector.rs:136` 的 `upgrade_existing_ttl`
会改写全部 breakpoint）反推 TTL。但 `proxy/anthropic_cache_control.rs::normalize_cache_control_ttls`
显式允许「**1h 仅可作为前缀；一旦出现 5m，其后 1h 全部降级为 5m**」：

```rust
Some("1h") if !seen_five_minute => {}                            // 前缀 1h 保留
Some("1h") => { object.remove("ttl"); seen_five_minute = true; }  // 其后降级
```

即**一次请求内 1h 与 5m breakpoint 共存是被支持并规范化的常态**，`cache_creation_input_tokens`
是两个价档的求和，标量不可拆。此外该配置在 `profile_id.is_some()` 时 disabled（`adapters.rs:3338`），
且存于 Client 的 `StoredProvider.settings_config`，**Router 无从读取** —— 配置反推同样需要新契约字段，
成本与直接携带拆分相同。

| 路径 | 可行性 |
|---|---|
| 从现有 `share_request_logs` 拆 | ❌ 信息已不可逆丢失 |
| 从 provider 配置反推 | ❌ 混合 TTL 合法存在 + Router 读不到配置 |
| Server 补解析嵌套对象 → v7 携带 2 字段 → 加 2 列 | ✅ 唯一正确路径 |

**本设计的处理：按 5m 计价，并给出上界。** 含 cache write 的模型行附 `cacheWriteAssumed5m` note，
同时返回 `equivalentUsdMicrosUpperBound`（全部按 1h 计价，§7.5），UI 以区间展示。

目录已备好 `cache_write_1h_micros_per_1m`（§5.1），因此：
- 今天用它算上界，把估算不确定性量化为可见区间
- 将来若走通 v7 携带 token 拆分，目录无需再迁移，只需改计算入口

> 参考 TokenHub 的 `Rates` 已区分 `cache_write_5m` / `cache_write_1h`（`internal/metering/pricing.go:11-18`）。
> 补齐虽只需 Server 多解析两个 token 计数（不算钱，严格说不违反 R3），但需 v7 + 双端版本协商 + fail-closed 处理，
> 风险等级与本设计「纯只读叠加」不同档。**明确不在阶段一范围内**，如有真实需求应单独立项。

### 11.2 窗口内调价

见 §8.3。按窗口结束时刻的价格统一计算，`pricingRevision` 可审计。

### 11.3 `usage_state` 非 `observed` 的请求

`share_request_logs.usage_state` 默认 `'observed'`。非 `observed`（估算/缺失）的行 token 数可能不准。

**处理：照常计入，但在行级统计 `estimatedRequestCount`**，若占比超过 5% 则附 `usageStateNotFullyObserved` note。**不排除这些行** —— 排除会让明细之和小于配额列，制造新的对不上。

---

### 11.4 服务层级的残余不确定性

§6.5 把 `effective_service_tier` 纳入了选价，消除了 priority 流量约 2× 的系统性低估。剩余不确定性有两处：

1. **并非所有模型都有 priority/flex 独立价。** LiteLLM 目录中 `input_cost_per_token_priority`
   仅覆盖 43 个模型、`_flex` 24 个。缺档时按 §5.2 回退到标准价并附 `serviceTierFellBack`——
   此时若上游实际按 priority 收费，等价金额偏低。
2. **该列可能为空。** 非 Codex 路径（Claude/Gemini/Cursor 等）当前未写入
   `effective_service_tier`，一律按 standard 计。若这些上游存在层级差价，同样偏低。

两处都是**方向确定的低估**（不会高估），且都通过 note 暴露。不额外做推断。

## 12. 前端

两个消费面（R4）：owner 面 §12.1–12.5，公开面 §12.6。**共用 `usd-micros.ts` 格式化，不共用组件**
——形状不同，硬合会把 owner 面的 email 维度漏进公开面。

### 12.1 不加列，改用可展开行

`ShareUserLimitsTable`（`frontend/components/dashboard/drawer-panels.tsx:1007`）当前 5 列 + `table-fixed` + `text-[11px]`，已相当紧凑。再加两列在窄屏必然溢出。

两个调用点必须同时受益：
- `frontend/components/dashboard/share-edit/share-user-grants-editor.tsx:720`（编辑态）
- `frontend/components/dashboard/share-edit/share-edit-read-view.tsx:181`（只读态）

### 12.2 交互

```
┌──────────────────────────────────────────────────────────────────────┐
│ ▸ zhang@x.com [shareto]   3   ████░░ 120.5M / 200M      2d 4h        │ ← 现有行不变
│     ≈ $12.40 ~ $13.28  ⓘ                                             │ ← email 单元格下方副行
├──────────────────────────────────────────────────────────────────────┤
│ ▾ 展开                                                                │
│   ⚠ 该用户配额已重基线，下方为实测值                                   │ ← 仅 rebaseApplied 时
│   模型                       Input  Output  CacheR  CacheW       等价 │
│   Claude Sonnet 4.5           2.1M    1.8M   98.2M   12.1M      $9.80 │
│   Claude Sonnet 4.5  [长上下文] 0.6M   0.4M    2.0M    0.3M      $2.41 │
│   gpt-5-codex        [Priority] 0.8M   0.6M    3.1M       —      $0.79 │
│   Claude Haiku 4.5            0.4M    0.3M    4.2M    1.1M      $0.32 │
│   gemini-x-preview   [未定价]  1.2M    0.9M      —       —          — │
│   未归属                         —       —       —       —          — │ ← 0.4M tokens
│   ─────────────────────────────────────────────────────────────────  │
│   已定价覆盖 97.3% · 缓存写入按 5m 估算 · 12% 请求为估算 · 目录 a3f2c1 │
└──────────────────────────────────────────────────────────────────────┘
```

三点关键：

1. **同一模型可能出现多行。** §8.2 按 `(model, serviceTier, contextTier)` 三维分组，因此
   「Claude Sonnet 4.5」标准档与长上下文档是两行。非 `standard` / 非 `base` 的行**必须**带
   Chip 区分，否则用户会以为是重复数据。
2. **等价金额是区间。** `equivalentUsdMicros ~ equivalentUsdMicrosUpperBound`（§7.5）。
   两者相等时（无 cache write，或该模型无 1h 价）退化为单值，不显示 `~`。
3. **`unattributed` 单独成行。** 它有 token 数但按定义无金额（§7.6），四个分拆列均为 `—`。
   这一行的存在就是「明细之和 = 配额列」的解释，不能省略。

### 12.3 组件改动

`ShareUserLimitsTable` 增加**两个可选 props**：

```ts
breakdown?: ShareUserUsageBreakdownMap;   // email(lower) -> row
onExpand?: (email: string) => void;       // 懒加载回调
```

**不传时行为与现状逐字节一致** —— 零回归风险。展开区作为 `<tr>` 后插的整宽 `<td colSpan>` 渲染，不影响 `colgroup` 布局。

### 12.4 展示细则

- 等价金额 `font-mono`，`≈` 前缀强调估算性质；区间用 `~` 连接，两端同宽对齐
- `ⓘ` tooltip 展开 `lines` 逐项（token 数 × 单价 = 金额），并列出该行命中的所有 `notes`
- 未定价模型：金额列 `—` + 警示 Chip，**不显示 `$0`**
- Chip 语义分三类，视觉上必须可区分：
  - **信息**（`serviceTier` / `contextTier` 非缺省）：中性色，说明这行按不同费率计
  - **降级**（`serviceTierFellBack` / `contextTierFellBack` / `unknownServiceTier`）：警告色，说明费率是回退取的
  - **估算**（`cacheWriteAssumed5m` / `usageStateNotFullyObserved`）：弱化色，说明金额是估的
- Token 数复用 `formatTokenMillions()`（`frontend/lib/token-units.ts`），受 `audit:web-token-units` 约束
- **懒加载**：仅在首次展开时请求 `?email=`，弹窗打开不触发
- 金额格式化新增 `frontend/lib/usd-micros.ts`，从字符串微美元格式化，**全程不经 `Number` 算术**

### 12.5 i18n

新增三组键，中英各一份（`frontend/lib/i18n.ts` 单文件双语，键必须成对出现）：

```
dashboard.userLimit.byModel.*        title / model / input / output / cacheRead / cacheWrite
                                     / equivalent / unpriced / unattributed / coverage
                                     / expand / collapse
dashboard.userLimit.equivalent.*     label / hint / range / rebaseNotice / catalogRevision
dashboard.userLimit.note.*           §9 notes 枚举逐值一个键，值为一句人话解释：
                                     priceKeyNotFound / cacheWriteAssumed5m
                                     / longContextApplied / serviceTierFellBack
                                     / contextTierFellBack / unknownServiceTier
                                     / usageStateNotFullyObserved
```

`note.*` 与 §9 的枚举**一一对应**，任何一侧新增值都必须同时改另一侧——这条由 §14.2 的审计断言守住。

---

### 12.6 公开面：市场目录的价目卡

匿名可见，落在既有市场目录 listing 卡的详情区。

```
┌──────────────────────────────────────────────────────┐
│  可用模型与官方标价                    目录 a3f2c1    │
│  Claude Sonnet 4.5                                    │
│    输入 $3.00 · 输出 $15.00 · 缓存读 $0.30            │
│    缓存写 $3.75      （每 1M token · 长上下文 200K）  │
│  ─────────────────────────────────────────────────   │
│  近 30 天用量构成                                     │
│    缓存读 83% ████████▏  输出 4% ▍                    │
│    输入   9% ▉           缓存写 4% ▍                  │
│    ⓘ 实际折算取决于你自己的用法，差异可达数十倍        │
└──────────────────────────────────────────────────────┘
```

细则：

- **价目区永远渲染**（静态、不依赖用量、无 k-匿名限制）
- **构成区在 `usageMix === null` 时整块不渲染**，不显示「暂无数据」——
  k-匿名不足是常态而非异常，空状态反而会引导用户去追问是谁
- 那句 `ⓘ` 文案是**必需的**，不是可选提示：它是 §9.2 那个 21× 论证在 UI 上的唯一体现
- 构成条用占比宽度，**不标注绝对 token 数**（绝对值可反推单个买家的量级）
- 不做任何「按你的用法估算」计算器——那会重新引入被 §9.2 否掉的点估计

新增 i18n 组：

```
shareMarket.pricing.*    title / perMillion / input / output / cacheRead / cacheWrite
                         / longContext / usageMix / mixDisclaimer / catalogRevision
```

---

## 13. 分阶段

| 阶段 | 内容 | 触碰配额执行？ |
|---|---|---|
| **一（本设计）** | 价目表 schema（3 表）+ `share_listing_usage_rollup` + 派生价目 + `model_pricing.rs` + owner 面 `user-usage-breakdown` + 公开面 `listings/:id/pricing` + 两套 UI | ❌ 纯只读叠加 |
| **二** | `/settings` 价目管理界面：管理员覆盖、生效时间、未定价模型巡检、覆盖率看板 | ❌ |
| **三（明确排除，需单独立项）** | 限额口径可切换：按 token 数 或 按等价美元，逐 listing 选择 | ✅ 改 Share Contract 与 Server 执行 |

> 阶段一因 R4 比原计划重：多一张 rollup 表、多一个公开端点、多一套 UI。这是「公开可见」这个
> 产品决策的直接成本，不是范围蔓延。

### 13.1 阶段三的架构障碍（现在不解，但别堵死）

限额**执行**在 Server 侧，价目表与计算**全部**在 Router 侧（R3）。要按等价美元卡额，
Server 必须知道美元——而它按定义算不出来。三条出路：

| 出路 | 做法 | 代价 |
|---|---|---|
| A | Router 周期性下推「每 Share 的等价权重」，Server 用权重乘 token | 权重滞后于实际用量结构；但 R3 不破 |
| B | 限额执行上移到 Router | 改动最大；Server 断连时的 fail-closed 语义要重做 |
| C | Share Contract 携带价目表 | R3 直接破；Server 变成会算钱的组件 |

**现在不选。** 记录在此是因为：三条路今天都不花钱，等阶段三真要做时再选，任何一条都是大手术。
阶段一唯一要做的是**不把「配额单位恒为 token」这个假设焊进新增结构**——
`share_listing_usage_rollup` 存 token 明细而非折算额（§8.5），已经满足这一点。

**阶段三不在本设计范围内。** 它会改动 Share Contract 与 Server 侧执行路径，风险等级与阶段一完全不同。应在阶段一、二落地并跑过一段真实流量、用实测模型分布校验价目准确性之后，再单独立项。**在价目表未经真实流量校验前就用它卡配额，会直接影响卖家收入。**

---

## 14. 测试与审计

### 14.1 Rust

| 范围 | 用例 |
|---|---|
| `model_pricing` | 四类单价逐项计算；零 token 不产生 line；`i128` 不溢出；上界计算（有/无 1h 价） |
| 费率解析 | 回退链四步顺序（§5.2）；层级回退优先于档位回退；回退时发出对应 note |
| 服务层级 | `fast` → `priority` 归一化；`effective_*` 优先于 `client_*`；未知值 → standard + note |
| 长上下文 | `inclusive` 真/假两种阈值方向的边界（`==` 分别命中/不命中）；四类费率**全部**切换；`longContextApplied` 仅在总额实际上升时为真（含「命中档位但总额未升」的用例） |
| 口径对账 | 四项全零且 `quota_tokens > 0` 时进入 `unattributed`，且不计入金额；四项非零时不重复计数 |
| 别名解析 | exact 优先 prefix；同 priority 最长 prefix 优先；未命中返回 unpriced |
| 目录装载 | derived upsert 不覆盖 admin 行；catalog / rates 级联一致；JSON 非法则启动失败；`pricing_revision` 对同一集合稳定 |
| 聚合 | 与配额窗口逐字段对齐；分档聚合与逐请求定价结果等价；`is_health_check=1` 被排除；rebase 存在时 `byModel` 之和等于 observed |
| 公开面收窄 | 响应中**不含** email / 用户数 / 请求数绝对值（结构化断言，非字符串 grep）；`distinct_users < 3` 时 `usageMix` 为 `null`；非 `publicly_listed` 返回 404 |
| rollup | 重建幂等：同一 `(share_id, bucket)` 重算结果逐字段相等；`DELETE` 全表后重建与原值一致；缺桶时公开面降级为 `null` 而**不**触发 raw 扫描（以查询计数断言） |
| 面隔离 | owner 面查询路径**不读** rollup；公开面查询路径**不读** `share_request_logs` |
| Schema | `installs_and_reopens_fresh_baseline` 的表数断言从 `137` 改为 `141`（新增 catalog / rates / aliases / listing_usage_rollup 四张表，`src/schema.rs:671`）；新表 CHECK 约束生效 |

> 注意 `src/schema.rs` 的 `baseline_checksum_stays_frozen` 钉死了 `0001_baseline.sql`，本设计**只新增** `0039_*.sql`，不触碰 baseline。

### 14.2 前端

- `frontend/lib/usd-micros.test.ts`：字符串微美元格式化、大数不失精度、区间两端相等时退化为单值
- 扩展 `audit:web-token-units`：断言等价金额路径**不出现** `Number(` / `parseFloat(` 算术
- `audit-model-prices.mjs` 顺带断言 §9 的 `notes` 枚举与 `frontend/lib/i18n.ts` 的
  `dashboard.userLimit.note.*` 键集合完全相等，且每个键 `en` / `zh-CN` 双份齐备

### 14.3 新增审计脚本

`scripts/audit/audit-model-prices.mjs`（对齐既有 `audit-settings-contract.mjs` 等），`--check` 模式断言：

1. `pricing/model-prices.json` 每条含非空 `sourceNote`（记录上游出处与派生日期）
2. 所有单价为**非负整数**（微美元），无浮点字面量
3. `priceKey` 唯一；别名 pattern 已 lower+trim 且无重复
4. 每个 `priceKey` 的生效区间不重叠
5. 每个 `priceKey` 至少存在 `(standard, base)` 一档（回退链的兜底必须有底）
6. 声明了 `longContextThreshold` 的条目，必须同时存在 `(standard, long)` 费率行
7. 声明了 `cacheWrite1h` 的条目，`supportsCacheBreakdown` 必须为 true
8. 单位换算可逆：`micros_per_1m / 1e12` 与派生记录里的上游原值在容差内一致
9. JSON 字段集合与 Rust 侧结构体精确一致（防止单侧漂移）

`SOURCE_PROVENANCE.json` 由既有 `audit:source-provenance` 覆盖——派生条目登记为
historical attribution，**不得**登记为 `vendoredSources`（否则会触发
`technicalInput=false` 断言，见 §5.3）。

接入 `scripts/static-checks.sh` 与 CI。

### 14.4 UI_TEST_PLAN

在 `UI_TEST_PLAN.md` §11 Share 相关(S) 新增用例（现有最大编号 S-46，故新增自 S-47 起），并同步 §18 反查索引与 §19 API 覆盖核对：

| 用例 | 前置 | 操作 | 期望 |
|---|---|---|---|
| S-47 | 有多模型流量的 Share | 打开编辑弹窗，展开某用户 | 按模型列出四分拆与等价；金额 `font-mono` 带 `≈` |
| S-48 | 承上 | 观察未定价模型行 | 金额显示 `—` 与未定价 Chip，**不显示 $0** |
| S-49 | 该用户存在 usageRebase | 展开 | 顶部显示重基线提示；明细之和 ≠ 配额列属预期 |
| S-50 | — | 打开弹窗但不展开 | 不发起 breakdown 请求（Network 面板核对） |
| S-51 | 只读态弹窗 | 展开 | 与编辑态展示一致，无编辑控件 |
| S-52 | 存在 priority 或长上下文流量的 Share | 展开 | 同一模型出现多行且带层级/档位 Chip，视觉可区分；无 Chip 行即标准档 |
| S-53 | 存在 `quota_tokens > 0` 但四项全零的请求 | 展开 | 出现「未归属」行，token 数非零、四个分拆列与金额列均为 `—` |
| S-54 | **未登录**，公开市场目录 | 打开某公开 listing 详情 | 渲染可用模型与官方标价；无任何 email、无等价美元总额 |
| S-55 | 承上，该 listing 活跃用户 < 3 | 同上 | 用量构成区**整块不渲染**，且不出现空状态占位 |
| S-56 | 承上，活跃用户 ≥ 3 | 同上 | 渲染四类占比条与免责文案；条上不含绝对 token 数 |
| S-57 | 非 `publicly_listed` 的 listing | 直接访问 `/pricing` | 404（非 403，不泄露存在性） |

---

## 15. 非目标

明确**不做**：

1. 阶段三的加权配额（§13）
2. 扩展 Share Contract 携带 cache write 5m/1h 拆分（§11.1）
3. 让等价金额进入 `market_*` 任何账务表、API 或账单快照（R1）
4. 复活 `officialPricePercent` 或任何等价语义的 Share descriptor 字段（R2）
5. Server 侧计算成本或 USD（R3）
6. 从外部源在线拉取价目（引入外部依赖与 fail-open 风险；改价走发版 + 管理员覆盖）
7. 清理 §1.5 中恒空的 `capacity_usage` 分支（独立改动，不搭车）
8. 按模型的图表/趋势（`share_usage_by_email` 已有分桶范式，未来可另议）
9. 在 `effective_service_tier` 缺失时**推断**服务层级（§11.4）——宁可确定性低估并标注，不做猜测
10. 写入期冻结成本（TokenRouter 式 `usage_logs.cost` 列）。本设计是读时计算，理由见 §16
11. 公开面的等价美元总额、「每 1M 配额值多少美元」密度、或任何形式的「按你的用法估算」计算器（§9.2）
12. 在公开面暴露 email、用户数、请求数等绝对量（R4）
13. 现在就在 A/B/C 三条路里选定阶段三的限额执行架构（§13.1）

---

## 16. 参考实现

来自 `/data/projects/proxy`：

| 项目 | 文件 | 借鉴 | 未采纳部分 |
|---|---|---|---|
| **TokenHub** | `backend/internal/metering/pricing.go` | 计算内核分包哲学；六类费率；逐项 `Lines`；精确数值而非浮点；`model_catalog_data.go` 的 `pricing_periods` 时间版本化 | `big.Rat`（Rust 侧用 `i128` 定点，不引新依赖） |
| **done-hub** | `model/price.go` | 绝对单价而非倍率；长上下文分档的存在性；`GetExtraRatio` 的缺省回落（`:136-150`） | `LongContextTier{Threshold, InputRatio, OutputRatio}` 只有两个倍率——缺 cache 两类，被 LiteLLM 数据证伪（§5.1 改为四类各一个绝对价）；`datatypes.JSONType` 存储方式（我们用规范化列） |
| **new-api** | `setting/ratio_setting/model_ratio.go` | — | 倍率制链条长、可读性差；自定义 quota 单位（`USD = 500`）服务于其钱包体系；`HasPrefix/Contains` 猜模型族（`:483,525`）维护性差 |
| **TokenRouter** | `internal/service/billing_service.go`、`ent/schema/usage_log.go`、`migrations/229`,`232` | 见下 §16.1 | 见下 §16.1 |
| **sub2api** | `ent/schema/usage_log.go`、`internal/handler/admin/dashboard_snapshot_v2_handler.go:322` | 见下 §16.2 | 见下 §16.2 |

### 16.1 TokenRouter：本设计三处修正的直接来源

TokenRouter 是这批参考里唯一把「服务层级 × 上下文档位 × 缓存 TTL」三个维度同时做进计费的实现，本设计 §5/§7 的三处修正都来自比对它：

**采纳：**

| 它的做法 | 我们的对应 |
|---|---|
| `ModelPricing` 携带 `*Priority` / `*Flex` 独立价而非统一倍率（`:89`） | §5.1 `model_price_rates.service_tier` 维度。倍率制会在缺档模型上算错 |
| `CacheCreation5mPrice` / `CacheCreation1hPrice` 两个独立字段 + `SupportsCacheBreakdown` 开关 | §5.1 同名两列 + `supports_cache_breakdown`。今天只用 5m 价算值、1h 价算上界（§7.5） |
| `LongContextThresholdInclusive` —— 阈值比较方向按厂商而异 | §5.1 `long_context_inclusive`，§8.2 SQL 里的 `:inclusive` 分支 |
| `LongContextBillingApplied = baselineCost != nil && bd.ActualCost > baselineCost.ActualCost`（`:1463`）——**看结果，不看条件** | §7.3 同款。避免「命中阈值但费率相同」时打出误导性标记 |
| `normalizeCacheCreationBreakdown`（`:1485`）——拆分之和与标量对账，不一致时以标量为准 | §7.6 的 `unattributed` 桶。同一条原则：分项与总额冲突时，总额是真相，差额显式化而非抹平 |
| `:1472` 注释「API 未返回 ephemeral 明细，回退到全部按 5m 单价计费」 | §11.1。独立实现遇到同一个数据缺口、选了同一个回退，佐证这不是我们的疏漏 |
| `antigravity_gateway_service_test.go:1657` 的 SSE fixture 含 `cache_creation.ephemeral_5m_input_tokens` / `_1h_` | §11.1 的证据基础——嵌套对象**确实存在于上游响应**，是 Server 侧 `store.rs:1275` 未解析 |
| LiteLLM `model_prices_and_context_window.json` 作为价目来源 | §5.3。但我们**派生**而非 vendor，理由见 §5.3 的 provenance 政策约束 |
| migration `232` 的降级契约：rollup 是缓存，raw 是真相，rollup 可随时重建 | **§8.5 直接采用。** owner 面不物化（低 QPS），公开面（R4）匿名可刷，阶段一即需 rollup。差别是我们的 rollup **只存 token 不存金额**，以保住「改一次价目、历史全局纠错」 |

**不采纳：**

- **写入期冻结成本**（`usage_log` 上的 `decimal(20,10)` 成本列）。TokenRouter 要对最终用户出账，成本必须在请求时刻定死。我们是**只读展示叠加**（R1），读时计算换来的是「改一次价目，历史全部重算」，这对「官方价等价」这个语义反而更正确。代价是 §8 的聚合成本——已用 §8.2 的分档预聚合压住。
- **`float64` 中间态 + Postgres `decimal`**。libSQL 无 decimal 类型；`i128` 微美元定点是我们唯一能同时保证精确与可存储的选择。
- **`billing_allocations`（多主体成本分摊）**。与 R1 直接冲突——本设计的等价金额不得进入任何账务路径。
- **Redis 计费缓存、ent 代码生成**。栈不匹配。

### 16.2 sub2api：一个被识别但不纳入本设计的产品机会

sub2api 在 `usage_log` 上加了 `upstream_response_model` 与三态 `upstream_model_mismatch`，并在管理端做成可筛选维度（`dashboard_snapshot_v2_handler.go:322`）——即记录「客户端请求的模型」与「上游实际回答的模型」是否一致。

对 Share Market 而言这是个有价值的信号：**卖家把请求静默转发到更便宜的模型**，买家按贵模型的配额被扣、按贵模型看等价金额，而实际拿到的是廉价模型的输出。本设计的按模型明细恰好会让这类偏差第一次变得可见，但**要真正判定 mismatch，需要 Server 侧记录上游响应中的 model 字段并经 Share Contract 上报——那是 v7 的事**（R3：Server 不算钱，但可以多报一个事实字段）。

**明确不纳入本设计**，记录在此作为独立立项的输入。R4 之后这条的优先级上升：
公开面既然展示「该 listing 可用模型的官方标价」，买家就有了明确预期，而
静默转发到廉价模型正是对该预期的违反——公开标价把一个原本模糊的问题变成了可主张的差异。

**不采纳：** sub2api 其余部分与 TokenRouter 同源（ent/Postgres/Redis），无额外可借鉴项。
