# stdin 断管 panic 清理测试的退出同步

日期：2026-11-23。候选版本：`0.154.0-rc10`。本次仅修改测试同步及版本记录，不修改生产清理逻辑。本机安装仍为已验收的 `0.154.0-rc8`。

## 失败证据与原因

PR 172 的提交 `3ed52507692f7b608600665a859bf8940cd7e821` 在[自托管 Linux worker](https://github.com/niuwoai/willdeep-cli/actions/runs/45100220685/job/137449107944) 中出现：

- `process_cleanup::tests::cleanup_reaps_client_after_stdin_write_panics` 失败；该轮 headless_runtime 为 62 通过、1 失败。
- 先打印第 602 行注入的 BrokenPipe panic，随后第 631 行断言为 `left: None, right: Some(BrokenPipe)`。
- 同一提交的托管 Linux 运行中，该测试和此前 MCP 缺失进程测试均实际通过；本机未修改前的单项测试也通过。因此这是时序相关失败，不能将日志解释为生产 ChildGuard 丢失错误或没有回收。

测试分为普通 Child 负对照和 ChildGuard 两轮，每轮重新记录写入错误。第一轮写入断管后，catch_unwind 捕获 panic，但 panic hook 仍输出日志；第二轮可能在子进程完全退出之前写入成功，错误种类于是为 None，随后触发最终断言。

原同步条件只要求 stdout EOF。进程关闭 stdout 与关闭 stdin、完成退出并不等价，短写入可能在 stdin 读端关闭前完成。

## 修复与证明边界

在读完 stdout 后、写入 stdin 前，使用 `waitid(P_PID, pid, ..., WEXITED | WNOWAIT | WNOHANG)` 观察本轮直接子进程完整退出：

- 仅观察目标 PID，不查询或终止其他进程。
- `WNOWAIT` 保留待回收状态，避免提前代替被测 ChildGuard 回收子进程。
- 遵循既有 CLEANUP_TIMEOUT / CLEANUP_POLL；处理 EINTR，超时和系统调用失败明确报错。
- 保留写入必须 BrokenPipe、panic 内容及 waitpid 验证：普通 Child 负对照必须仍可回收，ChildGuard 必须已在展开时回收并得到 ECHILD。

没有使用固定延迟替代退出条件，没有在写入前调用 Child::wait / try_wait / 普通 waitpid，也没有放宽断管和负对照断言。现有超时清理兜底保持不变。

## 验证

本地验证通过：清理模块 17 项测试通过、0 失败、0 忽略；断管清理单项额外独立重复 10 次全部通过，每次均包含普通 Child 负对照和 ChildGuard 对照。内部版本钉测试 1 项通过；headless_runtime 严格 Clippy、rustfmt、发布版本元数据及 git diff --check 通过。结果摘要保存为 [JSON](stdin-cleanup-ci.json)。本轮未重跑无关的完整 Runtime/TUI 套件；完整 Linux 和其他平台结果由新提交的 CI 确认。

独立真实子进程实验已确定性复现原同步假设无效：独立夹具先关闭 stdout、再等待读取 stdin；父进程观察 stdout EOF 后仍成功写入 stdin。实验只使用自有 `/bin/sh` 子进程、清空子进程环境，退出后回收子进程；源码保留在本地 `.build/stdout-eof-reproducer.rs`，不纳入生产代码。该实验验证 EOF 与 stdin 关闭不等价，并非声称重演了原 Linux runner 的完整调度时间线。

```bash
cargo test -p willdeep --test headless_runtime process_cleanup:: --locked --offline --target-dir .build/cargo-target --no-fail-fast
```

```bash
cargo test -p willdeep --test headless_runtime process_cleanup::tests::cleanup_reaps_client_after_stdin_write_panics --locked --offline --target-dir .build/cargo-target -- --exact
```

```bash
cargo test -p willdeep --test workspace_versions --locked --offline --target-dir .build/cargo-target
```

```bash
cargo clippy -p willdeep --test headless_runtime --locked --offline --target-dir .build/cargo-target -- -D warnings
```

```bash
ruby scripts/check_release_version.rb 0.154.0-rc10
```

本机为 macOS、Rust 1.94.1，与失败 worker 的 Rust 版本一致。Linux 的修复后执行结果须由新提交的 CI 确认；不将本机结果当作 Linux 运行证据。未公开发行或创建发布 tag，未替换本机已安装的 rc8。
