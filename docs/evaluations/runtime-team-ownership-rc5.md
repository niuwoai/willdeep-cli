# Runtime 团队执行权发布验证

日期：2026-10-14；最终版本：0.99.0-rc5。

团队锁修复沿用 rc4 验收记录：界面初始化不恢复或占用团队；取得会话执行权后首次恢复，后续轮次不误中断当前 Worker。rc5 修正了之前版本已有的六处 Clippy 问题，使用具名派工参数结构与等价条件简化，没有放宽静态检查。

## 发布前结果

- `cargo test --workspace --locked --offline`：1493 项通过，7 项按设计忽略；包含 30 项真实 Runtime 集成测试。
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`：通过。
- `cargo publish --dry-run --locked --allow-dirty -p willdeep-runtime-protocol -p willdeep-runtime-client`：两个 SDK 均打包并验证通过，未上传到 crates.io。
- Ruby 评测、指标、RSI、执行与反馈契约自检：67 项测试通过，20 个本地模型评测任务均合格。
- `yarn --cwd web build`：通过。
- 版本一致性、`cargo fmt --all --check`、`git diff --check`、源文件大小检查：通过。

## 发布范围

rc4 标签保留原提交，最终交付使用 rc5。GitHub 发行包、本地 Cargo / Homebrew / Runtime 和 h2 systemd 实例的升级结果在发布完成后独立回查。本记录是发布前验证结果，不代替安装或服务器验收。
