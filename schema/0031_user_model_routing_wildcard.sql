-- 统一模型入口：引入用户显式配置的 `*` 全量路由。
--
-- `*` 是保留的 requested_model 取值，表示「该 app_type 下任何未被精确映射命中的模型
-- 都转发到此 Share」。它由用户显式写入，可见、可审计、可删除，因此不属于
-- PROTOCOL.md §9.2 所禁止的「系统推断的默认 Share」。
--
-- 选路优先级固定为「精确 > `*`」，一次查询定终局，不是 fallback。
-- 现有 UNIQUE(user_id, app_type, requested_model) 天然保证每个 (user, app) 至多一条 `*`。
--
-- 本迁移唯一的实质变更是收紧 requested_model 的 CHECK：只有恰好等于 `*` 时才允许出现
-- `*` 字符，杜绝将来被误用为前缀/后缀/正则模糊匹配。SQLite 无法原地追加 CHECK，故重建表。

CREATE TABLE user_model_routes_new (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    app_type TEXT NOT NULL CHECK (app_type IN ('claude','codex','gemini')),
    requested_model TEXT NOT NULL CHECK (
        requested_model = trim(requested_model)
        AND length(requested_model) BETWEEN 1 AND 200
        AND (requested_model = '*' OR instr(requested_model, '*') = 0)
    ),
    -- Intentionally not a foreign key. A removed Share must leave a visible,
    -- unavailable route instead of silently changing into "not configured".
    target_share_id TEXT NOT NULL CHECK (target_share_id != ''),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE (user_id, app_type, requested_model)
);

-- 旧 schema 从不接受含 `*` 的模型名（归一化层无此路径），过滤是防御式的：
-- 宁可丢弃一条不可能存在的畸形记录，也不让迁移在 CHECK 上整体失败。
INSERT INTO user_model_routes_new (
    id, user_id, app_type, requested_model, target_share_id, created_at, updated_at
)
SELECT id, user_id, app_type, requested_model, target_share_id, created_at, updated_at
FROM user_model_routes
WHERE instr(requested_model, '*') = 0;

DROP TABLE user_model_routes;

ALTER TABLE user_model_routes_new RENAME TO user_model_routes;

CREATE INDEX idx_user_model_routes_target
    ON user_model_routes(target_share_id, app_type);
