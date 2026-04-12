# portr-rs

面向 `cc-switch` 的最小 Rust tunnel server。

## 快速开始

### 源码运行

当前实现包含：

- `POST /v1/installations/register`
- `POST /v1/tunnels/lease`
- `GET /v1/healthz`
- `GET /v1/dashboard`
- `POST /v1/shares/sync`
- `POST /v1/shares/batch-sync`
- `POST /v1/shares/delete`
- `GET /admin`
- `GET /admin/login`
- `POST /v1/admin/login`
- `POST /v1/admin/logout`
- 基于 `russh` 的一次性 SSH 密码认证
- 基于 Host subdomain 的最小 HTTP 反向代理

默认监听：

- HTTP API: `0.0.0.0:8787`
- SSH: `0.0.0.0:2222`
- Tunnel domain: `0.0.0.0:8787`

可通过环境变量覆盖：

- `PORTR_RS_API_ADDR`
- `PORTR_RS_SSH_ADDR`
- `PORTR_RS_TUNNEL_DOMAIN`
- `PORTR_RS_USE_LOCALHOST`
- `PORTR_RS_LEASE_TTL_SECS`
- `PORTR_RS_DB_PATH`，默认 `$HOME/.config/portr-rs/portr-rs.db`
- `PORTR_RS_ADMIN_TOKEN`
- `PORTR_RS_CLEANUP_INTERVAL_SECS`
- `PORTR_RS_LEASE_RETENTION_SECS`

默认配置文件路径：

- `$HOME/.config/portr-rs/.env`

启动时如果这个文件不存在，`portr-rs` 会自动生成默认 `.env`，然后按该文件加载配置。进程环境变量优先级更高，会覆盖 `.env` 里的同名配置。

启动：

```bash
cargo run
```

查看帮助：

```bash
cargo run -- help
```

默认日志级别是 `info`。如需查看更多细节，可显式指定：

```bash
RUST_LOG=debug cargo run
```

### 一键构建

项目根目录已提供 [build.sh](/data/projects/portr-rs/build.sh)：

```bash
chmod +x ./build.sh
./build.sh
```

默认行为：

- 执行 `cargo build --release`
- 将二进制和部署文件整理到 `dist/`
- 生成 `.tar.gz` 发布包
- 生成 `sha256` 校验文件（系统支持时）

可选环境变量：

```bash
TARGET_TRIPLE=x86_64-unknown-linux-gnu ./build.sh
BUILD_MODE=debug ./build.sh
```

## 二进制部署

### 1. 准备发布包

执行：

```bash
./build.sh
```

产物会出现在：

- `dist/portr-rs_<version>.tar.gz`
- 或 `dist/portr-rs_<version>_<target>.tar.gz`

发布包内包含：

- `portr-rs` 二进制
- `README.md`
- `ARCHITECTURE.md`
- `portr-rs.env.example`

### GitHub Release

GitHub Actions 会在 `main` 分支自动构建 Ubuntu AMD64 二进制，并更新 `latest` Release。Release 只附带单个二进制文件：

- `portr-rs-linux-amd64`

### 2. 服务器解压

示例：

```bash
mkdir -p /opt/portr-rs
tar -C /opt/portr-rs -xzf portr-rs_0.1.0.tar.gz --strip-components=1
```

### 3. 准备运行目录

建议目录结构：

```text
/opt/portr-rs/
  portr-rs
  README.md
  ARCHITECTURE.md
  portr-rs.env.example
  data/
```

然后创建数据目录：

```bash
mkdir -p /opt/portr-rs/data
```

### 4. 配置环境变量

首次启动前不必手动创建配置文件。默认情况下，服务会自动生成：

```text
$HOME/.config/portr-rs/.env
```

如需预先编辑，可直接创建或修改这个文件。

最小生产示例：

```bash
cat > "$HOME/.config/portr-rs/.env" <<'EOF'
PORTR_RS_API_ADDR=0.0.0.0:8787
PORTR_RS_SSH_ADDR=0.0.0.0:2222
PORTR_RS_TUNNEL_DOMAIN=your-domain.example.com
PORTR_RS_USE_LOCALHOST=false
PORTR_RS_DB_PATH=$HOME/.config/portr-rs/portr-rs.db
PORTR_RS_ADMIN_TOKEN=change-me-admin-token
EOF
```

如果需要长期保留更少历史数据，可额外配置：

```bash
export PORTR_RS_CLEANUP_INTERVAL_SECS=300
export PORTR_RS_LEASE_RETENTION_SECS=604800
```

### 5. 启动二进制

```bash
cd /opt/portr-rs
./portr-rs
```

查看帮助：

```bash
./portr-rs help
```

默认启动会打印 `info` 级别日志。生产环境如需调整日志详细程度，可追加：

```bash
RUST_LOG=debug ./portr-rs
```

### 6. 验证部署

健康检查：

```bash
curl http://127.0.0.1:8787/v1/healthz
```

预期返回：

```json
{"ok":true}
```

控制台登录页：

```text
http://127.0.0.1:8787/admin/login
```

### 7. systemd 部署示例

示例服务文件：

```ini
[Unit]
Description=portr-rs
After=network.target

[Service]
Type=simple
WorkingDirectory=/opt/portr-rs
Environment=HOME=/root
EnvironmentFile=%h/.config/portr-rs/.env
ExecStart=/opt/portr-rs/portr-rs
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
```

应用：

```bash
sudo systemctl daemon-reload
sudo systemctl enable portr-rs
sudo systemctl start portr-rs
sudo systemctl status portr-rs
```

打开控制台：

```text
http://127.0.0.1:8787/admin
```

默认需要 admin token 才能访问 `/admin` 和 `/v1/dashboard`：

```bash
export PORTR_RS_ADMIN_TOKEN=your-admin-token
```

当前限制：

- 仅实现 HTTP tunnel
- 设备私钥仍由 `cc-switch` 以本地文件方式保存，未接入系统安全存储
- share 用量同步为“事件驱动最终一致”，由 `cc-switch` 在创建、状态变更、用量变更、删除时异步上报
- `cc-switch` 端 share 同步已做短延迟批量聚合，降低高频请求噪音
- `portr-rs` 会定时清理超过保留期的历史 lease，以及状态为 `expired` / `deleted` 的陈旧 share 记录
