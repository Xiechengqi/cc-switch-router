-- 模型中枢：为每条路由引入有序候选目标列表。
--
-- 现有 `user_model_routes.target_share_id` 保留为"主候选"（priority=0）的物化列，
-- 本表存放同一条路由的其余候选（priority >= 1）。请求时按 priority 升序依次尝试：
-- 当前候选连接失败/超时/5xx 时自动切换到下一个；候选耗尽仍 fail-closed 报错。
--
-- 设计约束（PROTOCOL.md §9.2 修订版）：
--   - 仅在用户显式配置的候选列表内切换，绝不臆造目标。
--   - 响应字节流出后不再切换（流式首 chunk 后锁死），避免客户端收到拼接响应。
--   - 熔断为进程内存态，由真实转发流量驱动，与 share_model_health 探测解耦。
--
-- 兼容性：老客户端 PUT 单目标时只写 routes 表主候选，candidates 表对应为空，
-- 读取时主候选即完整列表。UNIQUE(route_id, target_share_id) 防止重复候选，
-- UNIQUE(route_id, priority) 保证优先级唯一。

CREATE TABLE user_model_route_candidates (
    id              TEXT PRIMARY KEY,
    route_id        TEXT NOT NULL REFERENCES user_model_routes(id) ON DELETE CASCADE,
    target_share_id TEXT NOT NULL CHECK (target_share_id != ''),
    priority        INTEGER NOT NULL CHECK (priority >= 1),
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    UNIQUE (route_id, target_share_id),
    UNIQUE (route_id, priority)
);

CREATE INDEX idx_user_model_route_candidates_route
    ON user_model_route_candidates(route_id, priority);
