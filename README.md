# ApiTest

ApiTest 是一个使用 Rust 与 egui 构建的本地优先 API 桌面工具。它以低内存占用、流式处理和清晰的模块边界为设计重点，提供接近 Apifox 的请求调试工作流，同时不依赖浏览器运行时。

## 当前能力

- 紧凑的深色/浅色桌面工作台，支持简体中文与英文，并持久化外观设置。
- HTTP 请求编辑、参数、请求头、完整请求体、环境变量、Basic/Bearer/API Key、发送/取消和流式响应查看。
- GraphQL 与 SSE 复用 HTTP 流式执行管线。
- WebSocket 双向会话、消息时间线与主动关闭。
- gRPC proto/descriptor/reflection 动态发现，支持 unary、服务端流、客户端流和双向流。
- Basic、Bearer、API Key 认证；敏感值通过系统钥匙串引用，不写入项目文档。
- 环境变量嵌套解析、作用域覆盖、缺失变量提示和循环检测。
- SQLite WAL 持久化、FTS5 搜索、外置大响应体和滚动备份。
- QuickJS 沙箱脚本、响应断言、场景串行执行和变量传递。
- 本地 Mock 服务、OpenAPI 导入、项目 JSON 交换及多语言请求代码片段。

桌面编辑器当前聚焦 HTTP 调试与环境管理闭环。WebSocket、gRPC、Mock、脚本和场景执行已经在运行时层提供稳定接口，但尚未暴露为桌面入口。OAuth2、Digest、AWS SigV4 的领域模型已经预留，执行器会明确返回“尚未支持”，不会静默发送错误认证信息。

## 运行

需要 Rust 1.97.0。仓库中的 `rust-toolchain.toml` 会选择正确工具链。

```bash
cargo run -p apitest-app --release
```

Linux 桌面环境若缺少窗口系统开发库，请安装发行版对应的 X11/Wayland、`libxkbcommon` 和 `pkg-config` 包。

## 质量检查

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
cargo check -p apitest-app --release
```

## 工作区

| crate | 职责 |
| --- | --- |
| `apitest-core` | 协议无关领域模型、变量系统、执行事件契约 |
| `apitest-storage` | SQLite、响应体文件、钥匙串与备份 |
| `apitest-runtime` | HTTP、SSE、WebSocket、gRPC、Mock、脚本与自动化 |
| `apitest-interop` | OpenAPI/项目导入导出与代码片段 |
| `apitest-app` | egui/eframe 桌面交互和运行时编排 |

详细设计见 [架构说明](docs/architecture.md)。

## 本地数据

项目元数据保存在操作系统应用数据目录的 `apitest.sqlite3`。响应体存储和备份由存储层使用独立文件管理；密钥只保存引用，真实值交给操作系统凭据存储。项目交换格式不会导出真实密钥。
