# 模型独立验收

状态：failed

当前模型：qwen3.6-35b-a3b

| 检查 | 状态 | 秒数 | 说明 |
| --- | --- | ---: | --- |
| configuration | passed | 0.0 |  |
| authentication | passed | 0.0 |  |
| service_active | passed | 0.006 |  |
| health | passed | 0.001 |  |
| loaded_model | passed | 0.001 |  |
| chat_ok | passed | 0.118 |  |
| legacy_alias_1 | passed | 0.052 |  |
| forced_tool_add | passed | 0.527 |  |
| tool_round_trip | failed | 0.36 | JSON::ParserError: unexpected token at '```json {   "result": 42,   "receipt": "c5107bb0f71d2f6c" } ```' |
| json_object | failed | 0.231 | JSON::ParserError: unexpected token at '```json {"result": 42} ```' |
| sandbox_availability | passed | 0.388 |  |
| programming_dedupe | failed | 2.143 | GemmaAcceptance::CheckFailure: 固定 unittest 验收失败，详见 sandbox 输出 |
| programming_decimal_refunds | failed | 3.717 | JSON::ParserError: unexpected token at '```json {   "path": "solution.py",   "source": "from decimal import Decimal, getcontext, ROUND_HALF_EVEN\nimport re\n\ndef aggregate_amounts(rows):\n    # Pattern for amount: non-negative decimal string with up to 2 decimal places\n    # Strictly matches (0\|[1-9][0-9]*)(\\.[0-9]{1,2})?\n    amount_pattern = re.compile(r'^(0\|[1-9][0-9]*)(\\.[0-9]{1,2})?$')\n    \n    totals = {}\n    \n    for row in rows:\n        if not isinstance(row, dict):\n            raise TypeError(\"Each row must be a dict\")\n        \n        # Check required fields\n        if 'currency' not in row or 'kind' not in row or 'amount' not in row:\n            raise ValueError(\"Missing required fields\")\n        \n        currency = row['currency']\n        kind = row['kind']\n        amount_str = row['amount']\n        \n        # Validate currency: exactly three ASCII uppercase letters\n        if not isinstance(currency, str) or len(currency) != 3 or not currency.isupper() or not currency.isalpha():\n            raise ValueError(\"Invalid currency\")\n        \n        # Validate kind: must be 'sale' or 'refund'\n        if kind not in ('sale', 'refund'):\n            raise ValueError(\"Invalid kind\")\n        \n        # Validate amount: must be a string matching the pattern\n        if not isinstance(amount_str, str):\n            raise ValueError(\"Invalid amount type\")\n        \n        if not amount_pattern.match(amount_str):\n            raise ValueError(\"Invalid amount format\")\n        \n        # Convert to Decimal\n        amount = Decimal(amount_str)\n        \n        # Initialize totals for this currency if not present\n        if currency not in totals:\n            totals[currency] = Decimal('0.00')\n        \n        if kind == 'sale':\n            totals[currency] += amount\n        elif kind == 'refund':\n            totals[currency] -= amount\n    \n    # Round all totals to 2 decimal places and ensure they are Decimal with exponent -2\n    result = {}\n    for currency, total in totals.items():\n        # Quantize to 2 decimal places\n        rounded = total.quantize(Decimal('0.01'), rounding=ROUND_HALF_EVEN)\n        result[currency] = rounded\n    \n    return result\n" } ```' |

## 验收边界

- 每项独立记录；失败不阻止无依赖的后续项目，任何 failed/not_run 均使最终退出码为 1。
- 当前/旧别名均须精确返回 OK，响应 model 须等于当前 model_alias；/props 单独核对实际文件。
- expected_model_file 为文件名时比较 basename；为绝对路径时比较完整 model_path。
- 工具只执行固定白名单 add(19,23)，校验 tool_call_id 往返、result=42 及本轮随机 receipt；另验收 JSON object。
- 编程题只接受 solution.py 完整源码 JSON；固定 unittest 8 + 9 项（含多组 subTest）在隔离 Docker 中运行。
- 源码不在 host 执行；容器无网络、只读 root/夹具、非 root、无新增权限，20 秒超时，不挂载凭据。
- 不拉取镜像；缺 Docker/镜像/Python 明确 not_run。没有吞吐、长上下文或 GPU 内存压力验收。

产物目录：/root/benchmarks/gemma4-96k-70043de5/runs/qwen-coding-control/checks-20261104T153120Z-20261104-2262334-kteg5k
