# willdeep × Harbor（Terminal-Bench）

在 Harbor 任务容器里安装官方 `willdeep-linux-amd64` 二进制，用 **`willdeep run --local --full-auto`** 完成 Terminal-Bench 指令。评分仍由 Harbor 的任务 verifier 负责，本适配器只负责「装 CLI → 写临时 config → 跑一轮」。

## 前置

- [Harbor](https://harborframework.com/) CLI（`uv tool install harbor`）
- Docker（Terminal-Bench 默认本地沙箱）
- 模型凭据，例如 `export SOMEIM_API_KEY=...`

## 安装适配器

Harbor 自带独立 Python，请把包装进 **harbor 解释器**（或每次用 `PYTHONPATH`）：

```bash
# 推荐：装进 harbor 的 uv 环境
uv pip install -e integrations/harbor-willdeep \
  --python "$(dirname $(command -v harbor))/python"

# 若上面路径不对，常见位置：
#   ~/.local/share/uv/tools/harbor/bin/python

# 临时：不安装，只加 PYTHONPATH
export PYTHONPATH="/path/to/willdeep-rs/integrations/harbor-willdeep:${PYTHONPATH:-}"
```

验证：

```bash
PYTHONPATH=integrations/harbor-willdeep python3 -c \
  "from willdeep_harbor.agent import WilldeepAgent; print(WilldeepAgent.name())"
```

Harbor 通过 **import path** 加载 agent（名字 `willdeep` 尚未 upstream 进 Harbor 内置列表）：

```text
willdeep_harbor.agent:WilldeepAgent
```

## 单题冒烟（some.im + glm-5）

先确认本机能调通模型（与 [docs/SOMEIM_INTEGRATION.md](../../docs/SOMEIM_INTEGRATION.md) 一致）：

```bash
export SOMEIM_API_KEY="<your-key>"
export OPENAI_BASE_URL="https://some.im/v1"   # 国内可用 https://api.niuwoai.com/v1
```

从仓库根目录：

```bash
harbor run \
  --agent willdeep_harbor.agent:WilldeepAgent \
  --model glm-5 \
  --base-url "${OPENAI_BASE_URL}" \
  --api-key-name SOMEIM_API_KEY \
  --api-format openai_chat_completions \
  --agent-kwarg version=v0.154.0-rc11 \
  --agent-kwarg provider=some-im \
  --agent-kwarg max_turns=60 \
  --dataset terminal-bench@2.0 \
  --n-concurrent 1 \
  -n 1 \
  -k 1
```

说明：

| 项 | 含义 |
|---|---|
| `--agent …:WilldeepAgent` | 使用本目录的 Python 适配器 |
| `--base-url` / `--api-key-name` | Harbor 把 some.im 端点与 Key 注入容器 |
| `--agent-kwarg provider=some-im` | 生成的 `config.toml` 使用 some.im Provider |
| `version=…` | 从 GitHub Release 拉取 `willdeep-linux-amd64.tar.gz` |

日志：`/logs/agent/willdeep.txt`（容器内）。

## JobConfig 示例

见 [examples/job-willdeep-someim-glm5.yaml](examples/job-willdeep-someim-glm5.yaml)：

```bash
harbor run -c integrations/harbor-willdeep/examples/job-willdeep-someim-glm5.yaml
```

运行前在 shell 或 `.env` 中设置 `SOMEIM_API_KEY`（勿提交仓库）。

## Agent 参数（`--agent-kwarg`）

| 参数 | 默认 | 说明 |
|---|---|---|
| `version` | `v0.154.0-rc11` | Release 标签，用于下载 linux-amd64 包 |
| `install_url` | — | 完整 tarball URL，设置后忽略 `version` |
| `provider` | `some-im` | 写入 `config.toml` 的 `provider` |
| `profile` | `harbor` | `[providers.harbor]` 段名 |
| `max_turns` | `80` | 传给 `willdeep run --max_turns` |
| `local_harness` | `true` | `willdeep run --local`（容器内建议开启） |
| `full_auto` | `true` | `willdeep run --full-auto` |
| `language` | `en` | Terminal-Bench 任务为英文，默认 `en` |
| `workdir` | `/app` | Harbor 默认工作目录 |

## 边界

- **不是** Harbor 上游内置 agent；升级 Harbor 后需自行确认 `BaseInstalledAgent` API 未变。
- 默认下载 **linux-amd64** 官方包；arm64 镜像请用 `install_url` 指向 `willdeep-linux-arm64.tar.gz`。
- 容器内使用 **`--local`** 进程内 harness，**不**依赖宿主机 Runtime daemon。
- 首版 **不** 导出 ATIF 轨迹；成功与否由任务测试判定。
- 与自家 [`bench/model-eval`](../../bench/model-eval/) 互补：model-eval 测 willdeep 小任务回归；本适配器测 Terminal-Bench 全沙箱题。

## 自检

```bash
python -c "from willdeep_harbor.agent import WilldeepAgent; print(WilldeepAgent.name())"
harbor agent auth willdeep_harbor.agent:WilldeepAgent 2>/dev/null || true
```

`harbor agent auth` 对自定义 import path 可能只显示通用 OpenAI 兼容项，属正常。

