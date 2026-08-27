# 发布流程

发布由 tag 驱动：推送 `v*` 标签后，`.github/workflows/release.yml` 在五个平台
（Linux x86_64/arm64、macOS Intel/Apple Silicon、Windows x86_64）原生构建
`apitest`，打包 tar.gz/zip、生成 `SHA256SUMS`，并创建 GitHub Release。
含 `-` 的标签（如 `v0.2.0-rc.1`）自动标记为 prerelease。

## 步骤

1. 本地质量门全部通过：

   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace --locked
   cargo check -p apitest-app --release --locked
   ```

   界面有改动时建议离屏渲染核对：

   ```bash
   APITEST_SHOT_DIR=/tmp/apitest-shots \
     cargo test -p apitest-app --lib tests::shots -- --ignored --nocapture
   ```

2. 更新 `[workspace.package].version`（`Cargo.toml`），提交。
   workflow 的 validate 任务会校验标签与该版本一致（忽略 `-rc.N` 后缀），
   不一致会在构建前失败。

3. 打标签并推送：

   ```bash
   git tag v0.1.0
   git push origin main v0.1.0
   ```

4. 在 Actions 页面观察 Release workflow；完成后到 Releases 页核对五个产物
   与 `SHA256SUMS`。

5. 至少在一个平台实测安装脚本：

   ```bash
   curl -fsSL https://raw.githubusercontent.com/zzhtl/apitest/main/install.sh | sh
   ```

## 首次发布建议

先推 `v0.1.0-rc.1` 走通整条流水线并实测 `install.sh`
（`APITEST_VERSION=v0.1.0-rc.1` 指定 rc），确认无误后删除 rc 的 release 与
标签，再正式发布 `v0.1.0`。

## 回滚

发布产物有问题时：删除该 GitHub Release 与对应标签，修复后重新打标签发布。
已下载的用户以 `SHA256SUMS` 为准辨别产物。

## 约束与已知事项

- 跨平台一律用原生 runner 构建：`aws-lc-sys`、`rquickjs-sys` 与 bundled
  SQLite 都编译 C 代码，交叉编译是最容易失败的路径。
- `macos-15-intel` 是 x86_64 macOS 的 runner 标签（macos-13 已于 2025-12
  退役）；若该标签下线，可改用 `macos-26-intel`，或在 `macos-15` 上
  `rustup target add x86_64-apple-darwin` 交叉构建。
- macOS 产物未签名、未公证；README 与 install.sh 已提示 `xattr` 解除隔离。
