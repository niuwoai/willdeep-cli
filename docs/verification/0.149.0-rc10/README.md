# TUI 鲸鱼恢复与窄屏适配验收

日期：2026-11-16。版本：0.149.0-rc10。

## 实现与结果

首屏恢复彩色鲸鱼、产品名和版本号。聊天区内宽至少 24 列时保持原宽版，12–23 列保持原小号，更窄时从小号整幅网格按列中心采样到可用宽度；真彩与单色共用字形和采样位置，始终保留 7 个终端行。普通问候正文继续正常换行，历史文字首屏记录仍可识别。

修复前新增的 Rust 边界回归实际失败：`mascot exceeds the 1-column viewport`。原因是小号鲸鱼固定输出 12 列，在不足 12 列的聊天区被通用正文折行拆散。

用户消息保留右侧蓝灰背景块、正文和时间，不显示生成的 `You:` 前缀与模型名；助手仍显示该条回复实际使用的模型。复制、引用和原文空白边界继续沿用既有修复。

## 检查结果

| 检查 | 结果 | 范围与限制 |
| --- | --- | --- |
| `CARGO_INCREMENTAL=0 cargo test -p willdeep tui:: --no-fail-fast` | PASS | 最终源码 536 通过、0 失败、2 项原有手动性能测试忽略；包含模型元数据模块移出后的完整复跑 |
| `CARGO_INCREMENTAL=0 cargo build -p willdeep` | PASS | debug CLI；实际 `--version` 为 0.149.0-rc10 |
| `NO_COLOR=1 CARGO_INCREMENTAL=0 cargo test -p willdeep tui::banner::tests::` | PASS | 8 通过、0 失败、0 忽略 |
| 实际 TUI + Runtime，Ruby PTY 隔离夹具 | PASS | 120×45 简洁、120×45 完整、60×45 窄屏；仅 2 次本机固定模型回复 |
| 实际欢迎会话的宽度边界 | PASS | 两种布局各 11 项，共 22 项；包含缩窄后恢复宽屏 |
| 采图校验器的缺陷注入检查 | PASS | 接受完整图案，拒绝每行仅留一个像素及扩大视口后仍残留小号的画面 |
| 图片几何与实际尺寸 | PASS | 每张主图 99 个鲸鱼块单元格；矩形宽 10，整块高 20、半块高 10；PNG 尺寸与素材索引一致 |
| 图片人工查看 | PASS | 已逐张查看三套聊天图，以及 25 列简洁与 13 列完整模式的边界图 |
| `cargo fmt --all -- --check` | PASS | Rust 格式 |
| `ruby scripts/check_release_version.rb 0.149.0-rc10` | PASS | Cargo、锁文件、两个 runtime 版本钉、Web/npm、安装清单、产品与架构版本头、CHANGELOG |
| `ruby -c scripts/verify_tui_user_messages.rb` | PASS | Ruby 语法 |
| `git diff --check` | PASS | 差异空白 |
| `CARGO_INCREMENTAL=0 cargo clippy -p willdeep --all-targets -- -D warnings` | FAIL | 最终源码仍有 3 处既有告警，exit 101；新测试的多余克隆告警已消除 |
| 代码规模与模型归属提取 | PASS | 相关 Rust 文件均不超过 3000 行；`app_state.rs` 为 2991 行，移入 `message_model.rs` 的函数体与基线逐字一致 |

最终严格 Clippy 的实际错误摘要：

```text
error: method `wait_until_idle` is never used
  crates/willdeep-cli/src/daemon/task_manager.rs:879
error: method `handle_question_text` is never used
  crates/willdeep-cli/src/tui/app_state.rs:1855
error: call to `std::mem::drop` with a value that does not implement `Drop`
  crates/willdeep-cli/src/tui/human_gate_ui.rs:130
error: could not compile `willdeep` (bin "willdeep") due to 3 previous errors
```

这三个告警对应的函数未被本轮修改；严格静态检查仍为失败，不能宣称全部工程检查通过。首轮新增测试的 `cloned_ref_to_slice_refs` 已用 `std::slice::from_ref` 修正，随后完整 TUI 回归和严格 Clippy 均已复跑。

为遵守 3000 行上限，只将与本次角色/模型归属有关的方法移入 `message_model.rs`；比较基线确认函数体未改写，没有改变模型选择或消息记录行为。

## 宽度与绘制证据

Rust 完整消息渲染覆盖内宽 1–26 列；两种布局在同一个 App 上依次使用内宽 `24→23→12→11→8→1→11→24`，每个尺寸绘制两次，核对实际 TestBackend 单元格、缓存复用、原始 transcript 不变，以及恢复到原宽图案。

实际 CLI 欢迎会话使用 80 行高度，简洁与完整布局分别遍历终端列数：

```text
120 → 60 → 40 → 26 → 25 → 24 → 23 → 14 → 13 → 10 → 120
```

逐格比对独立固定的宽版、小号图案，进一步采样时检查对应列；拒绝像素丢失、越过左右边界、终端软折行和额外图形行。尺寸变化后必须收到新的绘制输出，且整个画面连续稳定至少 0.5 秒。两种布局都实际验证了缩窄后恢复 120 列。

主聊天图使用 45 行，保留完整鲸鱼。边界验收另用空欢迎会话，避免将对话增多导致的顶部滚出视口与图形折行混淆。真实终端没有覆盖窄屏冷启动；它从 120 列启动后逐次缩放。极窄宽度的普通文字、状态栏截断按现有终端布局行为保留。

## 图片与原始证据

[素材说明、尺寸与来源索引](../../assets/tui-user-messages/0.149.0-rc10/README.md)。

- [简洁模式 PNG](../../assets/tui-user-messages/0.149.0-rc10/compact.png) · [SVG 原稿](user-messages/compact.svg)
- [完整模式 PNG](../../assets/tui-user-messages/0.149.0-rc10/full.png) · [SVG 原稿](user-messages/full.svg)
- [窄屏 PNG](../../assets/tui-user-messages/0.149.0-rc10/narrow.png) · [SVG 原稿](user-messages/narrow.svg)
- [25 列简洁边界图](user-messages/whale-compact-4-25.png) · [13 列完整边界图](user-messages/whale-full-8-13.png)
- [实际验收 JSON](user-messages/integration.json) · [Markdown](user-messages/integration.md) · [汇总检查 JSON](checks.json)
- [聊天原始 ANSI](user-messages/terminal.ansi) · [尺寸事件](user-messages/terminal-sizes.json)
- [宽度边界原始 ANSI](user-messages/whale-terminal.ansi) · [尺寸事件](user-messages/whale-terminal-sizes.json)

图片按实际 TUI 输出的 ANSI 单元格还原为 SVG，再转换成 2 倍尺寸 PNG，不是操作系统窗口截图。`▀`、`▄`、`█` 使用单元格矩形还原，半格为 10×10 方形像素；前景和背景分别绘制，避免字体替换、块字符字形缝隙与双色半块背景丢失。普通文字字体与终端调色板可能和使用者的环境不同。

## 隔离与交付范围

使用独立临时 home、工作区和本机回环固定回复服务，无真实会话、凭据读取或外部模型请求。`fixture-chat-model` 仅为测试模型标识。

未执行全工作区测试、原有忽略的手动性能测试或窄屏冷启动验收。修复验收后，用户另行授权本地安装；已完成 rc10 release 安装、内嵌 Web 入口核对与 Runtime 安全交接，详见 [本地安装和回滚记录](local-install.md) 及 [JSON 结果](local-install.json)。未修改官网页面、更新 Homebrew tap、对外发布、推送、创建 tag 或由本会话提交。
