# CI 构建性能与发布边界

## 为什么之前越来越慢

原工作流将前端、Rust 测试、release 编译、发布完全串行执行；Rust 依赖与 Next.js 构建中间结果没有缓存。市场测试数量增长后，每个业务测试又重复初始化完整数据库、重放 84 个迁移，其中大量表重建即使没有业务数据也有明显成本。

2026-10-24 检查的三次公开运行总耗时分别为 12 分 36 秒、24 分 25 秒、22 分 05 秒，发布时间仅约 18–30 秒。主要瓶颈是构建与重复迁移，不是上传 release 文件。

## 任务依赖

```text
frontend ── 同次运行、同一 SHA 的 export ── build ─┐
    └───────────────────────────────────────────┤
rust-tests ─────────────────────────────────────┴── release
```

- `frontend`：来源审计、既有前端单测和契约审计、静态导出、钱包/C2C/导航余额浏览器契约测试。浏览器测试只使用本地页面与模拟 API，不截图、不创建真实订单。
- `rust-tests`：与前端独立执行；编译测试二进制、数据库适配器与模板隔离测试、全部 `market_` 测试、全部真实 `schema::tests`。
- `build`：等待前端通过，下载当前运行中以完整 SHA 命名的 export，再编译 Ubuntu AMD64 release。
- `release`：必须等待以上所有任务成功；只有该任务拥有仓库写权限。旧构建任务可取消，发布任务单独串行、不在上传中途取消。

`market_` 筛选会额外命中 3 个迁移测试，它们只在该步骤排除，再由完整 `schema::tests` 步骤运行一次；没有删除测试或断言。测试线程数仍为 4，release 优化参数保持不变。

独立测试 job 不构建前端，但先创建稳定的空 `frontend/out` 目录：`build.rs` 会监视这个路径，如果路径一直不存在，Cargo 会在后续每次 `cargo test` 时重跑构建脚本，构建时间戳变化又会导致重复编译。该空目录仅用于 debug 测试；release job 仍必须下载真实静态导出，并通过嵌入内容校验。

使用真实 `build.rs` 的隔离 Cargo 探针已复现这一行为：目录缺失时连续两次构建都触发编译，创建空目录后的第二次构建直接复用产物。

## 缓存边界

- npm 仍只缓存下载目录，依赖安装使用 `npm ci`。
- Twemoji 源文件单独缓存；生成的 flags 按生成脚本、国家列表、lockfile 的哈希缓存。
- Next.js 仅缓存 `.next/cache`，按系统、Node 主版本、依赖与配置隔离；每个 SHA 保存新缓存，并从同作用域最近一次成功缓存恢复。CI 显式启用当前 Next.js 版本的 Turbopack 生产构建磁盘缓存。
- Rust 测试与 release 使用独立的 `Swatinem/rust-cache` 键，工具链、依赖锁文件与 profile 的变化不会混用产物。
- 不缓存整个 `.next` 或 `frontend/out`，不直接发布缓存中的业务二进制。导出目录只经当前 workflow 的 artifact 传递。

首次启用、工具链/lockfile 更新或缓存淘汰时仍会有冷构建成本。并行可以缩短等待时间，不保证减少 GitHub runner 总分钟数。

## 测试数据库模板

`src/test_support.rs` 仅在 `cfg(test)` 下编译。每个测试进程第一次使用时，仍调用真实 `schema::apply` 完成全量迁移，再通过 SQLite `VACUUM INTO` 生成物理快照。模板只保存在当前进程内存中，没有第二份 schema SQL，也没有跨版本缓存的数据库文件。

每个业务测试复制一份独立数据库，保留种子数据、迁移记录、触发器、索引和视图，不共享业务数据。原来的非持久内存 fixture 使用临时文件、MEMORY journal 与同步关闭；外键开关仍由原 fixture 设置。测试连接、statement 和 transaction 派生连接持有目录生命周期，最后一个持有者释放后清理。

涉及重启/多连接竞争的 fixture 通过 `database_at` 只创建新文件，保留默认持久化设置，绝不覆盖已有文件；重新打开仍使用真实 `Connection::open`。迁移与启动测试继续直接初始化数据库；生产数据库、历史 SQL、迁移 checksum 均不改动。

模板回归验证完整 schema 一致、种子数据与财务约束、跨测试/并行隔离、statement/transaction 生命周期、文件重开与禁止覆盖。

本地相同机器、4 线程、同一组 46 项 C2C 测试从 **111.55 秒降为 6.47 秒**；完整 462 项 market 测试用时 49.69 秒，54 项真实迁移测试用时 136.18 秒。以上均为测试执行时间，不含首次依赖编译，不能直接当成 GitHub runner 的整体耗时预测。

另外执行了完整 Rust 回归，1650 项全部通过；前端既有单测/审计、三组无截图浏览器契约测试、5 项 CI 契约测试和 release 构建均通过。实际 release 二进制的构建身份与本次导出的 86 个 HTML/JS/CSS/logo 资源已核对一致。

## 防止旧产物发布

release 构建通过 `CC_SWITCH_ROUTER_BUILD_COMMIT` 显式传入完整 workflow SHA，构建脚本检查格式并将短 SHA 写入版本信息。缺省时本地构建仍从 Git 读取版本。

`scripts/ci/verify-release.mjs` 不启动服务，直接读取 `build-info`，校验 commit、版本和时间，再检查 ELF 二进制确实包含当前 export 的关键 HTML、logo 以及全部 JS/CSS 的精确字节。身份不符、漏嵌入或缓存残留都会阻止发布。

发布脚本在串行发布槽内检查 `master`，已被更新提交取代的运行直接跳过，无法查询远端则失败。更新 `latest` 使用 GitHub API，不删除已有 release/tag。新建 release 先作为 draft；只有文件上传成功后才移动 tag、更新说明并正式发布。上传错误向上传递，不能显示为成功。

GitHub 同名 asset 的 `--clobber` 替换并非原子操作，仍可能存在短暂下载窗口；本实现不承诺零中断，也不会用删除整个 release 的方式扩大窗口。

## 验证与后续观察

本地可执行：

```bash
node --test scripts/ci/ci.test.mjs
bash -n scripts/ci/timed.sh scripts/ci/publish-release.sh
RUST_TEST_THREADS=4 cargo test --locked test_support::tests
RUST_TEST_THREADS=4 cargo test --locked db::tests
RUST_TEST_THREADS=4 cargo test --locked market_ -- --skip schema::tests
RUST_TEST_THREADS=4 cargo test --locked schema::tests
```

CI 每个主要阶段通过 `scripts/ci/timed.sh` 在日志和 Job Summary 记录耗时及退出码。合并后至少比较一次冷缓存、两次正常热缓存运行，分别关注依赖编译、业务测试、真实迁移、前端构建、release 编译及缓存恢复/保存时间，再决定是否需要进一步分片。

当前本地 Google Fonts 网络不可达，前端验证使用 `/tmp` 中临时 font mock 完成 Turbopack 导出与浏览器测试，不修改或提交生产字体配置。真实字体下载与 GitHub 缓存传输耗时仍需由实际 Actions 运行验证。
