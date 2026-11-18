# 当前请求与验证门禁边界验收

日期：2026-11-18。版本：0.153.0-rc5（develop 已使用 0.153.0-rc2 至 rc4，本次修复由 0.152.0-rc6 顺延）。当前状态：已合入 develop 并发布。此前 1343 项通过的全量回归只覆盖合入当时 rc1 的树，不作为与 rc4 合并后的结果。

## 行为

- 独立问答、文件查询和 shell 查询不因模型响应期间或工具之间的外部工作区变化而被要求跑项目测试。
- 本任务文件写入仍要求当前验证；shell、MCP、委派、Hook 窗口内的变化无法确认归属时保留 `Unverified` 并停止自动续跑，不催跑无关测试。
- `monitor` 与显式后台命令返回后仍可能改动工作区，保留同一暂停。暂停写入检查点的 `completion_scope_uncertain`；后台结果唤醒的宿主续轮沿用它，不因续轮本身变成催测，也不把旧计划重新装成当前计划。
- 普通新输入中的旧计划标记为历史背景，并在持久化中保留；未获同意不恢复为当前计划。
- 显式检查、完成契约、活动 Goal、同任务恢复和未知副作用保护继续有效。完成契约已经满足也不能绕过必要验证。
- 动态任务边界、完成门禁和时间预算收尾提示均要求只推进当前请求；扩大范围须先询问并得到明确同意。

## 夹具与失败记录

原生 Rust 单测使用 UUID 私有临时目录、合成 Provider、可注入验证快照及真实本地 shell。没有读取真实会话、配置、凭据，没有请求公网或真实模型，没有写用户工作区文件。

首次复现 4 项失败：3 项查询在外部变化后实际返回 `Unverified`，预期 `Finished`；另 1 项 shell 写入夹具因未配置审批回调而未执行，不能计为产品缺陷。补齐夹具的一次性审批后，真实写入得到执行并保留验证要求。修复后的首轮输入边界回归 12 项全部通过；扩充后的边界回归 17 项全部通过，包含失败写入、后台命令、历史计划保留和完成契约已满足后的验证要求。

首轮 Core 全量回归（1346 项）发现 `restarting_an_unverified_run_preserves_its_original_baseline` 失败：原始同任务/host 检查点入口的首次执行还没有保存基线，被误归为独立输入。修复为由显式 new-input 包装开启独立作用域，原始 sink 即使尚未保存基线也保留严格门禁；保持该恢复测试原有断言不变。该轮全量结果为 1338 通过、1 失败、7 忽略，运行的是这项修复之前的二进制，不能计为最终结果。合入 develop 前的最终代码重跑为 1343 通过、0 失败、7 忽略，退出码 0，耗时 420.70 秒。

独立复核另外确认两项：`monitor` 返回后仍在运行，后续工作区变化会被漏过；后台结果唤醒的宿主续轮使用原始 sink，把上一轮的无归属暂停重建为严格门禁并重新催测。两者都已补回归并修复：`monitor` 与显式后台命令同等对待，暂停写入检查点后由宿主续轮恢复。复核提出的另两项未采纳：命令通过 `start_new_session` 脱离原进程组后，现有执行层只清理自己的进程组，快照层无法区分这种脱离与其他会话的写入，保留为已知限制；历史消息中的未知工具回执仍使独立查询走严格门禁，这是既有未知副作用保护，本轮不放宽。

源码行数检查初次因 Ruby 默认 US-ASCII 无法解析已有中文路径而失败；使用 `ruby -E UTF-8` 重跑通过，未修改无关脚本。

## 验证结果

构建使用独立 `CARGO_TARGET_DIR=build/verification/task-scope-20261118/cargo-target`，dev/test debug 信息均设为 0；执行使用 `--locked --offline`。

- 与 develop 的 0.153.0-rc4 合并后，冲突只在版本元数据；验证代码两边均保留。`checkpoint::` 62 项为 61 通过、0 失败、1 忽略，`verification::` 59 项全部通过，严格 Clippy 通过。该结果不替代 Core 全量回归或 CI。
- 合入当时 develop rc1 后的 Core 全量回归（`cargo test --locked --offline -p willdeep-core --lib --no-fail-fast`，0.153.0-rc2）：1343 通过、0 失败、7 忽略，退出码 0，耗时 420.62 秒。超过 60 秒的两项（`actual_goal_loop_escalates_repeated_errors_and_unchanged_successes`、`recovery_inventory_beyond_4096_paginates_within_parent_and_marks_reports`）也通过。合入前同一套测试为 1343 通过、0 失败、7 忽略，耗时 420.70 秒。
- `checkpoint::` 定向回归：61 通过、0 失败、1 忽略（`execution_lock_probe` 为跨进程锁测试的子进程入口，按原设计忽略）。其中包含新增的 `monitor` 后续变化与宿主续轮无归属变化两项，以及修复后的原始基线保留测试。
- 严格 Clippy（`-p willdeep-core --lib --tests -D warnings`）、`cargo fmt --all -- --check` 与 `git diff --check` 在合入前通过。合入后 `ruby scripts/check_release_version.rb 0.153.0-rc2`、`cargo fmt --all -- --check` 与 `git diff --check` 通过。
- 源码行数检查通过：1732 个文件、无超过 3000 行的违例。

## 覆盖限制

快照不是进程级写入审计；无法区分恰好处于 opaque 工具窗口内的外部写入，也不能证明写后还原、仓库外或远端副作用。保守保留未验证状态，不把正常查询标为工程验证通过。

命令通过 `start_new_session` 等方式脱离原进程组后，执行层的进程组清理追不到，快照层也无法把它和其他会话的写入分开。这种情况保留为已知限制，不在本轮用命令名白名单或自建 shell 解析器去猜。

本轮不替代真实模型行为或三端完整运行时；没有为不可由浏览器证明的 Core 修复启动无关预览服务器。没有声称已从真实会话取证确认截图的具体触发条件。

## 发布与安装

2026-11-18，PR #159 的 10 项检查全部通过后，合并提交 `7331a6fc` 进入 `develop` 与 `main`，annotated tag `v0.153.0-rc5` 指向它。Release 工作流 `44296286520` 全部成功，产物为 `willdeep-macos-universal.tar.gz`、`willdeep-linux-amd64.tar.gz`、`willdeep-linux-arm64.tar.gz`。

- 本机 `~/.cargo/bin/willdeep` 与 `/opt/homebrew/bin/willdeep` 均为 `0.153.0-rc5`；Homebrew formula 提交为 `willdeep-cli 0.153.0-rc5`。daemon 从 `0.153.0-rc3` 交接到 `0.153.0-rc5`（pid 11906）。
- q4 经 `q4-pub` 替换用户 `willdeep` 与 root 两份二进制，旧版 `0.149.0-rc2` 分别留在对应目录的 `willdeep.bak-0.149.0-rc2`。升级前没有等待事件；daemon 交接到 `0.153.0-rc5`（pid 3207080）。
- iMac 内网直连被断开，经 `imac-pub` 替换 `/Users/yuanli/.cargo/bin/willdeep`，旧版 `0.153.0-rc3` 留在 `willdeep.bak-0.153.0-rc3`。升级前没有等待事件；daemon 交接到 `0.153.0-rc5`（pid 75344）。
- `~/Sites/willdeep-react-static` 的 `src` 与 `content` 没有写死 CLI 版本或 macOS 下载包名，未改站点。
