# 0.149.0-rc10 本地安装记录

日期：2026-11-16。用户已授权安装当前工作树版本。

状态：release 构建、CLI 安装和 Runtime 安全交接均成功；默认命令与健康的 Runtime 均为 `0.149.0-rc10`。结构化结果见 [local-install.json](local-install.json)。

## Runtime 交接与健康核对

使用新安装程序执行 `daemon upgrade --timeout 300`，退出码为 `0`。程序先排空旧 Runtime，等待活动任务结束后完成交接；未使用强制停止。

| 项目 | 安装前 | 安装后 |
| --- | --- | --- |
| 版本 | `0.149.0-rc3` | `0.149.0-rc10` |
| PID | `36399` | `39783` |
| 地址 | `127.0.0.1:65080` | `127.0.0.1:59447` |
| 状态 | `running` | `running`，CLI 健康核对通过 |

旧版备份实际执行 `--version` 返回 `0.149.0-rc3`，再次核对 SHA256 与备份时记录一致。回滚命令已记录，未实际执行回滚。

## CLI 安装核对

- Cargo 安装退出码为 `0`，release 构建耗时 `9m 06s`。
- 绝对路径和 PATH 默认命令均报告 `willdeep 0.149.0-rc10`。
- `target/release/willdeep` 与 `/Users/rocky/.cargo/bin/willdeep` 的 SHA256 一致，均为 `7e5eb8e12fc981776e8304d0013e98ee6029b3a8d5adc5b2d065b3e328a7c4b0`。
- 已在安装程序内核对到当前 Web 入口 `assets/index-CI09gyn_.js`，资源指纹与前端构建记录一致。
- 安装过程有两处既有未使用方法告警；Cargo 同时提示锁定的 `chacha20 0.10.1`、`clipboard-win 5.4.1` 已被 registry 撤回。本次保留 `--locked` 构建，未调整依赖，安装成功。

## 安装来源与目标

- 来源：`fix/tui-queue-during-compression` 工作树中的 rc10，包含恢复鲸鱼、窄屏适配与用户消息显示修复。
- 目标：`/Users/rocky/.cargo/bin/willdeep`，本机 PATH 中优先使用的命令。
- 安装前：Cargo 命令为 `0.149.0-rc3`；Runtime 为 `0.149.0-rc3`，PID 为 `36399`。
- Homebrew 命令为 `0.149.0-rc1`，保持已发布版本；本次本地安装不更新 tap 或发布 Release。
- 构建前已执行 `yarn --cwd web build`，成功生成内嵌 Web 资源。Vite 的大 chunk 提示为构建警告，构建结果成功。
- 新入口引用 `/assets/index-CI09gyn_.js`。`web/dist/index.html` SHA256 为 `b328dc53af7b5702a1437c618bdbe7e1fbe033d67c03a59dda3ea7781547d8bd`；入口 JS SHA256 为 `d80da913528b2959d6a9e0bbae148ea13cfc856bd7239bcce7b31301550fb82a`。

安装命令：

```bash
CARGO_INCREMENTAL=0 cargo install --path crates/willdeep-cli --locked --force
```

## 旧程序备份

备份路径：`/Users/rocky/.local/share/willdeep/local-install-backups/0.149.0-rc3-20261116-192826/willdeep`。

旧程序与备份 SHA256 均为 `8d28eaeb6463ac0367c1264084026bb4bcd6e376d340c11bc74abedf5ef859d9`。未清理已有备份。

## 回滚命令

复制到临时文件后再原子替换，避免原地覆盖运行中的可执行文件；随后使用程序自带的排空交接流程恢复旧 Runtime。若有待审批或待回答任务，先处理该任务，不能强制停止。

```bash
cp -p /Users/rocky/.local/share/willdeep/local-install-backups/0.149.0-rc3-20261116-192826/willdeep /Users/rocky/.cargo/bin/willdeep.rollback
```

```bash
mv -f /Users/rocky/.cargo/bin/willdeep.rollback /Users/rocky/.cargo/bin/willdeep
```

```bash
/Users/rocky/.cargo/bin/willdeep daemon upgrade --timeout 300
```

本轮未主动执行数据库迁移、数据回滚或数据清理；未提交代码、推送、创建 tag 或对外发布。
