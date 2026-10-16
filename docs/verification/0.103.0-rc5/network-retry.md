# 网络重试修复验证

版本：0.103.0-rc5。工作区基于 develop 的 `03d0a7e638381bf88e951780eb193d1f3666f3e3`，分支 `codex/network-retry-five`。

## 行为

Provider 在成功响应头之前最多重试 5 次，即首次请求加 5 次重试。默认间隔为 2、4、8、16、32 秒，合计最多增加 62 秒等待。服务端有效 Retry-After 优先；等待和全部尝试共用原请求期限，可取消，不增加整轮工具执行次数。重试等待与开始事件逐次上报，TUI 使用“等待后重试”，不把默认网络退避误说成服务端要求。

证书验证、无证书、协议或密钥配置等已知永久 TLS 错误通过 rustls 类型链识别，不使用凭据或错误正文字符串匹配。握手 EOF 等既有可恢复连接错误保留重试资格。直接复用锁定的 rustls 0.23.43，无新下载或依赖版本漂移。

成功响应头以后，正文或流式读取失败仍直接返回；已生成的部分文本与用量保留，不自动重新发起生成，不重跑已发生的工具。

## 实测

- 核心 common 回归：20/20 通过，覆盖 6 次总尝试和完整 10 条重试事件、退避表、Retry-After、取消、共享截止、401、TLS EOF、永久证书类型链、不可重放请求体，以及成功头后正文断流仅 1 次请求。
- 已有三种方言流式中断回归：3/3 通过。Chat Completions、Anthropic、Responses 都保留部分结果并仅发送 1 次请求，不暴露未完成工具。
- core 全 targets Clippy 严格检查通过；Rustfmt 和 git diff --check 通过。
- 全部为本地模拟或纯逻辑测试，真实模型与账号请求均为 0。没有运行整个仓库测试或新增付费评测。

首轮 20 例中有 1 个新测试失败：它假定 reqwest 的残缺正文错误必定为 is_body，而此版本标记为 is_decode。修正后要求 body/decode 错误，并继续保留明确的 HTTP200 脚本与只发送一次请求断言；生产重放边界未因此放宽。最终 20/20 通过，首轮失败日志保留在隔离证据目录。

## 隔离与资源

只用 clonefile 克隆原 target 的 deps、build、.fingerprint 到 `/private/tmp/willdeep-network-retry-cache/target`，无普通复制回退、无硬链接、不复制 incremental。克隆 42,522 个文件，逻辑约 36GB，耗时 10.6 秒；这不是构建提速比，未做同场景冷构建对照。

所有构建 jobs=1、关闭本次增量写入并监视至少 6GiB 空闲；未触发停止线。原 clone 源 Rust 二进制每个流程 SHA 均保持不变。旧 RSI 目录、付费账本、原冻结源与 Native/App 没有被本次实现写入；本次没有重新计算其他冻结 Native 的全部产物摘要。各命令、退出码、耗时、缓存模式、资源余量和源码摘要见同目录 JSON。

## 复跑与边界

使用独立 CARGO_TARGET_DIR 运行：

```text
cargo test --offline -p willdeep-core provider::common -- --nocapture
cargo clippy --offline -p willdeep-core --all-targets -- -D warnings
cargo fmt --all -- --check
```

三个现有流中断测试按完整名称定向执行，名称记录在 JSON 的 stream-no-replay 命令中。

成功响应头之前断线，服务端仍可能已经处理请求；增加重试不能证明服务端没有重复消费。较短的原请求总期限也可能在用满 5 次重试前结束。截图中的“网络或 TLS 中断”来自宽泛的错误分类，单凭该提示无法判断故障在连接阶段还是已开始的正文/流阶段。

本次修复未提交、推送、发版或安装；没有运行实际安装界面，也没有执行 Windows/Linux CI 或真实 Provider 验证。代码与已安装版本是两个独立事实。
