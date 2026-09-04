# 十轮架构会诊结论

> 目的：按用户要求，四个模型 + 当前 agent 对需求文档与后端架构进行 10 轮交叉评审。本文记录最终裁决，用于更新 `BACKEND_ARCHITECTURE.md` 与后续实现门禁。

---

## Round 1：总体架构

结论：端口/适配器方向成立，但需补强恢复一致性、LocalApi、取消、降级、资源维度。

---

## Round 2：Domain / Manifest

P0 修正：

- StageRecord 必须有 `stage_id` / `node_key`。
- artifact 不应内联混乱，StageRecord 引用 `artifact_id`。
- hash 必须带 `hash_algo_version`。
- `dependency_hash` 需拆出 `input_hash`、`param_hash`、`engine_version`、`stage_schema_version`。
- fallback 不能只写字符串，需结构化 `FallbackRecord`。
- 需要 `AttemptRecord` 记录失败链和重试历史。
- artifacts 需记录：content_hash、uri/type/schema_version、producer_stage_id、status、retention。

---

## Round 3：Scheduler / Resource Model

P0/P1 修正：

- 不用 EngineKind 硬编码准入逻辑，改成 ResourcePool / Cost 声明。
- Cost 需要细化：CPU、RAM、GPU/VRAM、process_slots、disk_bytes、io_bw、duration_hint。
- RemoteApi 用 token bucket；LocalApi 同时持有 API lease + local lease。
- lease 必须 RAII，取消/异常必须释放。
- 需要获取顺序/超时/回滚，防止 deadlock。
- 本地服务需要独立并发锁/连接上限。

---

## Round 4：ApiClient

P0/P1 修正：

- ApiClient 不应变成巨型类，拆成 Transport + middleware/decorator 链：RateLimit、Retry、Logging、Redaction、Timeout。
- 重试仅适用于 5xx/429/网络瞬断；4xx 直接失败。
- 遵守 `Retry-After`，使用指数退避 + jitter。
- 全链路 deadline，避免无限重试。
- 日志采用结构化 trace_id；body 只记录白名单字段和截断片段。
- 敏感字段用 Secret 类型/声明式 redaction，脱敏前置。
- 连通性测试应是独立 health probe，不污染业务重试统计。

---

## Round 5：ArtifactStore / 文件系统

P0/P1 修正：

- relative_path 必须防 `..`、绝对路径、junction/symlink/reparse 越权。
- Windows 长路径使用 `\\?\` 绝对路径处理，内部仍存 UTF-8 相对路径。
- staging 必须与目标同卷；跨卷 rename 非原子，禁止。
- 多文件产物需要事务目录整体提交。
- 启动时清理孤儿 `.tmp` 与 staging。
- 用户手动编辑/删除后，读前做懒验证：size/mtime，不符则 stale 或重算 hash。
- 文件锁定时重试/只读降级，不能死锁。
- 需要 retention/reference 计数，避免清理正在被 manifest 引用的大文件。

---

## Round 6：PipelineRunner / DAG / Resume

P0 修正：

- 必须显式产物级 DAG，不只是阶段名顺序。
- optional 节点用 `Skipped` 或 `Disabled` 状态表达。
- final 依赖 mix + srt；separation 可选但必须进入 dependency key。
- run_or_reuse 必须按 node_key 进行 single-flight，防止并发击穿同一节点。
- resume 依赖 commit record；未 commit 节点不复用。
- 失效传播基于 DAG 拓扑排序，而不是手写全量 invalidation。
- 父子上下文隔离，子任务只通过不可变产物引用回传。

---

## Round 7：UI / Task State Model

P0/P1 修正：

- 父状态必须区分共享阶段和子任务阶段。
- 部分失败应显示 `3/5 成功`、失败摘要与可进入详情。
- 空间显示分三类：可恢复缓存 / 成品 / 临时。
- 父级共享产物空间单独显示。
- 历史恢复需要状态：可复用、运行中、取消、失败、已降级、需重跑。
- 清理策略不能让用户误删恢复依赖。
- 子任务取消需要明确引用计数；全部子任务取消后父任务如何处理要有规则。

---

## Round 8：视频合成 / 音频对齐 / 字幕

P0/P1 修正：

- 0.85x~1.25x 不是万能。超限时不能静默截断。
- 对齐策略：先调整句间静音，再变速；仍超限则记录 overflow/warning，使用保守策略。
- SRT 时间轴必须和最终时间映射一致；需要 `TimelineMap`：原始时间 → 输出时间。
- 混音需默认 ducking：dub 优先，BGM 在配音期间衰减；总线限幅 -1dBFS。
- mix 与 final 可以物理上合并，逻辑上保留两个 stage record，减少重复 IO/编码。

---

## Round 9：多语言 / 方言配置

P0/P1 修正：

- 方言使用稳定 ID，建议靠近 BCP47 思路：`zh-CN`、`zh-yue`、`zh-minnan`。
- dialect JSON 需要 schema、version、alias、source、min_engine_version。
- 内置 + 应用同级配置合并策略：用户配置覆盖内置，同 ID 合并，坏 JSON 降级内置并写日志。
- 方言能力必须拆分：translation / tts / stt。
- 某方言可翻译但不可 TTS 时 UI/后端要明确降级或禁用。
- 前端不要写死支持列表，通过 descriptor/capability 判断齿轮可用。

---

## Round 10：迁移路径与测试策略

Go 条件：有条件 GO。

启动迁移前必须满足：

1. 旧 `process.rs` 保持可用，作为 Strangler 旧路径。
2. 每个迁移步骤有 feature flag / route flag，可回滚。
3. 新旧 pipeline 可 shadow 对比输出；差异归零后再切主。
4. Ports 有 contract tests，fake 与真实 adapter 共用契约测试。
5. Fake pipeline 必须覆盖成功、失败、取消、并发、路径异常、staging 崩溃。
6. 每迁移一个 command 必须独立验证、独立部署、独立回滚。

---

## 最终裁决

架构方向可行，但必须按以下 P0/P1 修改后再进入代码实现。

### P0 必须修改

- Manifest 增强：stage_id/node_key/hash_algo_version/attempts/fallback/artifact_id。
- ArtifactStore 路径安全、同卷事务、staging 恢复。
- Scheduler 资源池 + RAII + token bucket + LocalApi 双准入。
- ApiClient 中间件化，重试策略区分 4xx/5xx/429。
- PipelineRunner 产物级 DAG、commit record、single-flight、拓扑失效。
- 取消语义：CancelToken + 子进程 kill + 保留已完成产物。

### P1 强烈建议

- TimelineMap 与音频对齐策略。
- BGM ducking/响度/限幅。
- UI 状态细分：Degraded / PartialFailed / Invalidated / Reusable。
- 方言 capability 矩阵与配置 schema。
- LocalServiceManager：本地 API 服务启动/探活/端口/崩溃恢复。

### P2 后续增强

- LRU/TTL 缓存清理。
- 硬字幕 feature flag。
- 变速后音频质量检测。
- Segment 级细粒度失效（当前可先 stage 级）。
