# Terminal-Bench（Harbor × willdeep）归档

在 [Terminal-Bench](https://www.tbench.ai/) 数据集上，用 Harbor 沙箱 + [`integrations/harbor-willdeep`](../../integrations/harbor-willdeep) 跑 `willdeep run`。与 [`bench/model-eval`](../model-eval/) 的区别：这里是**全容器终端任务 + 官方 verifier**，不是本仓库内置小 fixture。

## 目录

| 路径 | 含义 |
|---|---|
| `history.jsonl` | 每次 job 一行摘要（append-only） |
| `reports/<日期>/<model>.<时刻>.json` | Harbor `result.json` 与逐题结论的归档副本（脱敏） |
| `reports/<日期>/<model>.<时刻>.md` | 同轮人类可读摘要 |

原始 Harbor 输出默认在仓库根目录 `jobs/`（已 `.gitignore`），只把**脱敏摘要**放进本目录。

## 口径

- **passed**：Harbor verifier 给分 / reward（`n_trials` 计入分子）。
- **infra_error**：沙箱、镜像、`docker compose`、容器内下载 willdeep、未连上模型等——**不算模型分母**。
- **model_invoked**：容器内 `willdeep run` 至少发起过一次 API；否则记 `model_invoked: false`。

## 本机前置（macOS + Colima）

1. Docker 上下文：`export DOCKER_HOST=unix://$HOME/.colima/default/docker.sock`
2. Harbor 调用 `docker compose` 子命令；Homebrew 需把 Compose v2 挂到 CLI 插件：
   `ln -sf $(which docker-compose) ~/.docker/cli-plugins/docker-compose`
3. Harbor agent 参数用 **`max_turns`**（下划线），不是 `max-turns`。
4. 容器内从 GitHub Release 拉 `willdeep-linux-amd64.tar.gz`；**标签必须已发布**（见 `gh release list -R niuwoai/willdeep-cli`）。仅 bump 仓库 `Cargo.toml` 而未发版时，用 `--agent-kwarg install_url=...` 或 `version=v0.154.0-rc11` 等已存在标签。

## 示例：qwen3.8-27b（some.im）

```bash
export DOCKER_HOST=unix://$HOME/.colima/default/docker.sock
export SOMEIM_API_KEY="$(python3 -c "import tomllib, pathlib; print(tomllib.loads(pathlib.Path('~/.willdeep/config.toml').expanduser().read_text())['providers']['some-im']['api_key'])")"
export PYTHONPATH="$PWD/integrations/harbor-willdeep:${PYTHONPATH:-}"

harbor run \
  --agent willdeep_harbor.agent:WilldeepAgent \
  --model qwen3.8-27b \
  --base-url https://api.niuwoai.com/v1 \
  --api-key-name SOMEIM_API_KEY \
  --api-format openai_chat_completions \
  --agent-kwarg version=v0.154.0-rc11 \
  --agent-kwarg provider=some-im \
  --agent-kwarg max_turns=60 \
  --dataset terminal-bench@2.0 \
  -n 1 -k 1 -l 3 \
  -o jobs --job-name "tb-smoke"

ruby scripts/archive_terminal_bench_job.rb jobs/tb-smoke
```

归档脚本会把 `result.json`、摘录的 `job.log`（去掉凭据行）写入 `bench/terminal-bench/reports/` 并追加 `history.jsonl`。
