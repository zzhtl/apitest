# ApiTest 架构说明

## 设计原则

1. UI 不直接依赖协议实现：界面只提交 `ExecutionRequest` 并消费 `ExecutionEvent` 流。
2. 小元数据与大响应体分离：SQLite 管理可检索文档，大响应体通过临时文件原子提交。
3. 密钥与项目数据分离：项目只持有 `SecretRef`，真实值在请求执行前从系统钥匙串物化。
4. 持续流式处理：HTTP、SSE、WebSocket 与 gRPC 都逐块产生事件，并响应取消令牌。
5. 静态模块化优先：crate 边界稳定，可扩展协议执行器，但不引入不必要的动态插件 ABI。

## 依赖方向

```text
apitest-app
  ├── apitest-runtime ──┐
  ├── apitest-storage ──┼── apitest-core
  └── apitest-interop ──┘
```

`apitest-core` 不引用 UI、网络或数据库。`apitest-runtime` 只通过 `SecretStore` trait 获取密钥；`apitest-app` 负责选择具体实现并将后台事件送回 egui。

## 请求生命周期

```text
编辑草稿
  → 构造 ProtocolSpec / ExecutionRequest
  → 解析环境变量与密钥引用
  → ProtocolExecutor 连接并发送
  → Started / ResponseHead / Data|Message / Metrics / Completed
  → UI 增量显示或 ScenarioRunner 汇总断言
```

每次运行拥有独立 `CancellationToken`。界面发起新请求或点击停止时会取消旧运行；旧运行的事件还会通过运行编号隔离，避免污染当前响应。

## 存储

- SQLite 开启 WAL、外键和 `synchronous=NORMAL`。
- API 文档以 JSON 保存，同时维护结构化索引和 FTS5 搜索表。
- 大响应体先写临时文件，提交时原子重命名；读取支持范围访问。
- 备份使用 SQLite backup API，并按保留数量清理旧快照。
- 项目交换格式带显式 `schema_version`，便于后续兼容迁移。

## 扩展新协议

1. 在 `apitest-core::ProtocolSpec` 增加纯数据配置和 `ProtocolKind`。
2. 在 `apitest-runtime` 实现 `ProtocolExecutor`，按统一事件顺序流式输出。
3. 为解析、取消、错误映射和本地集成服务编写测试。
4. 在桌面层增加草稿类型和编辑视图；不要把网络逻辑放进 egui 绘制函数。

## 性能边界

- UI 响应预览限制为 10 MiB，超出部分明确标记截断；完整数据可交由响应体存储层持久化。
- 网络响应按块处理，不要求一次性载入内存。
- QuickJS 默认限制 16 MiB 内存、512 KiB 栈和 2 秒执行时间。
- Tokio 后台运行时与 egui 绘制线程分离，后台事件只触发必要重绘。
