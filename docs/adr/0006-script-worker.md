# ADR-0006：常驻脚本线程与执行期缓存

## 状态

已接受。

## 背景

每次脚本求值都会新建整个 QuickJS `Runtime` 与 `Context`，并把变量和响应体
JSON 经 `format!` 内联进脚本源码，再被 JS 解析器重新解析一遍。一次发送里的
每条脚本断言、Mock 服务的每个动态路由请求都要付一次完整的运行时构建成本；
5 MB 响应配 10 条断言意味着约 200 MB 的瞬时拷贝。此外求值是同步阻塞的，却
直接跑在 2 线程的 tokio runtime 上。`rquickjs 0.12` 的 `Runtime` 仅在实验性
`parallel` feature 下实现 `Send`，无法简单地在多线程间复用。

同类问题也出现在其他执行路径上：`reqwest::Client`、gRPC 连接与 descriptor、
OAuth token、keyring 密钥都在每次发送时重建或重取。

## 决策

1. 名为 `apitest-script` 的专用 OS 线程持有唯一常驻 `Runtime`；每个求值仍
   新建 `Context::full`，脚本之间不共享任何 JS 状态。求值失败（超时、内存
   超限）后防御性重建运行时。数据通过 `ctx.globals()` 注入 JSON 字符串、由
   固定前奏 `JSON.parse`，不再拼接进源码。
2. `ScriptEngine::run` 保持同步签名（通道往返），新增 `run_async` 供场景
   执行等 async 调用方等待 oneshot，不再阻塞 tokio 工作线程；桌面端的断言
   求值包在 `spawn_blocking` 中。
3. 执行期缓存统一放在各执行器内部：HTTP Client 按连接相关配置缓存、OAuth
   token 按授权要素缓存并用 `refresh_token` 续期、gRPC 按端点缓存 Channel、
   按文件 mtime 缓存 descriptor、keyring 读取加 30 秒 TTL 装饰器。

## 取舍

所有脚本在一个线程上串行：Mock 高并发下动态路由会互相排队，但每次求值受
2 秒中断上限约束，且场景本身就是串行执行，实测中串行化不构成瓶颈；换来的
是运行时构建从每断言一次降为进程一次。密钥 TTL 缓存让明文在内存中多驻留
最多 30 秒——与执行器已物化的变量属同一暴露等级，换来每次发送少 N 次
D-Bus 往返。OAuth 缓存对没有 `expires_in` 的响应不缓存，保持旧行为，避免
猜测 token 生命周期。
