# Router 时钟偏差导致 Client Console 401

## 故障指纹

同时出现以下现象时,优先检查 Router 主机时钟:

- Client Console 的 `/` 和 `/assets/*` 正常返回 `200`,但 `/web-api/auth/methods` 等动态接口返回空正文 `401`。
- Router 发起的 `/_share-router/share-runtime`、`/_share-router/request-logs` 等请求也返回 `401`。
- 多个 Client 同时发生,且校时后动态接口立即恢复。

Router 为每个转发请求签发短期 IngressContext。Server 接受“最多过期 30 秒、最多超前 5 秒”的时间戳；边界值本身有效。Router 慢超过 30 秒会得到 `expired`,快超过 5 秒会得到 `future_timestamp`。

## 五分钟定位

在 Router 主机执行:

```bash
timedatectl status
timedatectl show -p NTPSynchronized --value
date --utc --iso-8601=ns
curl --silent --show-error --head https://www.cloudflare.com/cdn-cgi/trace | sed -n '/^[Dd]ate:/p'
journalctl -u cc-switch-router --since '-15 minutes' | grep -E 'clock|ingress'
journalctl -u cc-switch-http-timesync --since '-15 minutes'
sudo env CC_SWITCH_HTTP_TIMESYNC_DRY_RUN=1 /opt/cc-switch-router/scripts/ops/http-timesync.sh
```

Metrics 的 Host 页同时显示 Router 时钟偏差、NTP 状态、HTTPS 时间源仲裁和三类 ingress 拒绝计数。告警阈值按协议窗口非对称设置:

| Router 偏差 | warning | critical | Server 拒绝 |
|---|---:|---:|---:|
| 慢于真实时间 | 15 秒 | 25 秒 | 大于 30 秒 |
| 快于真实时间 | 2 秒 | 4 秒 | 大于 5 秒 |

## 区分其他 401

| 响应 | 含义 |
|---|---|
| Router 返回 `503 ingress-clock-skew`,带 `Retry-After: 5` | Server 明确拒绝 ingress 新鲜度；检查主机时钟 |
| Router 返回 `502 ingress-contract-rejected` | ingress 签名、身份、epoch 或字段契约错误；检查 Router/Server 配置与发布版本 |
| `401 login-required` | Router Client Web 登录态缺失或失效 |
| 普通 JSON/文本 `401` | Server 或上游的业务认证失败；不会被推断为时钟故障 |
| `503 tunnel-reconnecting` | Client tunnel/lease 独立故障；校时后仍需单独恢复隧道 |

Server 的 `x-cc-switch-internal-*` 诊断头仅供 Router 读取。Router 必须剥离这些头后才响应公网请求；不得用“空正文 401”猜测时钟故障。

## 恢复

1. 确认 UDP/123 出站和 DNS 可用,优先恢复 `systemd-timesyncd`。
2. 安装本仓库 `deploy/timesyncd/60-cc-switch-router.conf`,重启 `systemd-timesyncd`,直到 `NTPSynchronized=yes`。
3. NTP 明确未同步时,手动运行 `systemctl start cc-switch-http-timesync.service`。该服务只有在三路 HTTPS Date 至少两路仲裁成功且偏差大于 2 秒时才校正。
4. 验证 Metrics 时钟偏差回到 2 秒以内,然后验证 `/web-api/auth/methods` 恢复 `200`。
5. 对仍返回 `tunnel-reconnecting` 的 Client 单独检查 lease 和 Client 重连。

若 `NTPSynchronized=yes` 但 HTTPS 仲裁仍显示明显偏差,兜底服务只告警、不改时间。先检查 NTP 源、虚拟机宿主时钟和网络劫持,不要让两个校时器相互争用。

## 部署

```bash
sudo install -D -m 0644 deploy/timesyncd/60-cc-switch-router.conf /etc/systemd/timesyncd.conf.d/60-cc-switch-router.conf
sudo install -D -m 0755 scripts/ops/http-timesync.sh /opt/cc-switch-router/scripts/ops/http-timesync.sh
sudo install -D -m 0644 deploy/systemd/cc-switch-http-timesync.service /etc/systemd/system/cc-switch-http-timesync.service
sudo install -D -m 0644 deploy/systemd/cc-switch-http-timesync.timer /etc/systemd/system/cc-switch-http-timesync.timer
sudo install -D -m 0644 deploy/systemd/cc-switch-router.service /etc/systemd/system/cc-switch-router.service
sudo systemctl daemon-reload
sudo systemctl restart systemd-timesyncd
sudo systemctl enable --now cc-switch-http-timesync.timer
sudo systemctl enable --now cc-switch-router
```

`cc-switch-router.service` 只有 `CAP_NET_BIND_SERVICE`,不具备 `CAP_SYS_TIME`。只有短生命周期的 `cc-switch-http-timesync.service` 可以改系统时间。校正成功后,脚本仅在非容器且存在 RTC 设备时执行 `hwclock --systohc`。

发布顺序固定为:

1. 主机 NTP 与 HTTPS 校时兜底。
2. 先发布能够识别并剥离内部诊断头的 Router。
3. 再发布会发送 typed ingress 拒绝头的 Server。

回滚时顺序相反:先回滚 Server,再回滚 Router。

## 演练

只在隔离的预发布 Router 主机执行拨慢 35 秒的演练。先停止 NTP,记录基线,拨慢时钟,确认 Metrics/告警与 `503 ingress-clock-skew`,再启动一次 HTTPS 兜底校时并恢复 NTP。生产主机不得直接进行拨钟演练。
