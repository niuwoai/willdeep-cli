# Terminal-Bench：willdeep + qwen3.8-27b

| 字段 | 值 |
|---|---|
| 时间 (UTC) | 2026-11-24T05:27:53Z |
| Harbor job | `willdeep-qwen38-27b-tb-2` |
| 数据集 | terminal-bench@2.0，限 2 题 |
| 模型 | qwen3.8-27b @ `https://api.niuwoai.com/v1` |
| Agent | `willdeep_harbor.agent:WilldeepAgent` |
| 仓库 commit | `1ee01af8`（工作区含未提交项） |
| 总耗时 | ~3 分钟 |

## 结果

| 指标 | 数量 |
|---|---|
| 计划题数 | 2 |
| Harbor 完成（含异常结束） | 2 |
| **Verifier 通过** | **0** |
| 基础设施错误 | 2 |
| 模型是否被调用 | 否 |

本轮**不能**用来评价 qwen3.8-27b 做题能力：两题都在容器内安装 willdeep 时失败，未进入 `willdeep run`。

## 逐题

1. **adaptive-rejection-sampler** — `NetworkConnectionError`：容器里 `curl` 拉取 `.../releases/download/v0.155.0-rc9/willdeep-linux-amd64.tar.gz` 返回 **404**（该 tag 尚未在 GitHub 发布；当时 Latest 为 `v0.154.0-rc11`）。
2. **bn-fit-modify** — 同上。

## 宿主机侧已处理的问题

- 首次 3 题 job（`willdeep-qwen38-27b-tb-3`）：本机无 `docker compose` 插件 → `unknown flag: --project-name`。
- 本轮前：已配置 Colima `DOCKER_HOST` 与 `~/.docker/cli-plugins/docker-compose` 符号链接。

## 复跑建议

- 使用**已发布** Release：`--agent-kwarg version=v0.154.0-rc11`，或发版 `v0.155.0-rc9` 后再用默认 tag。
- 归档：`ruby scripts/archive_terminal_bench_job.rb jobs/<job-name>`

完整结构化记录见同目录 `qwen3.8-27b.132753.json`。
