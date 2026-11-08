# 模型独立验收

状态：failed

当前模型：qwen3.8-27b

| 检查 | 状态 | 秒数 | 说明 |
| --- | --- | ---: | --- |
| configuration | passed | 0.0 |  |
| authentication | passed | 0.0 |  |
| service_active | passed | 0.007 |  |
| health | passed | 0.001 |  |
| loaded_model | passed | 0.001 |  |
| chat_ok | passed | 0.239 |  |
| legacy_alias_1 | passed | 0.114 |  |
| forced_tool_add | passed | 1.23 |  |
| tool_round_trip | failed | 0.926 | JSON::ParserError: unexpected token at '<tool_call> <function=add> <parameter=a> 19 </parameter> <parameter=b> 23 </parameter> </function> </tool_call>' |
| json_object | passed | 0.492 |  |
| sandbox_availability | passed | 0.389 |  |
| programming_dedupe | passed | 3.474 |  |
| programming_decimal_refunds | failed | 8.355 | GemmaAcceptance::CheckFailure: 固定 unittest 验收失败，详见 sandbox 输出 |

## 验收边界

- 每项独立记录；失败不阻止无依赖的后续项目，任何 failed/not_run 均使最终退出码为 1。
- 当前/旧别名均须精确返回 OK，响应 model 须等于当前 model_alias；/props 单独核对实际文件。
- expected_model_file 为文件名时比较 basename；为绝对路径时比较完整 model_path。
- 工具只执行固定白名单 add(19,23)，校验 tool_call_id 往返、result=42 及本轮随机 receipt；另验收 JSON object。
- 编程题只接受 solution.py 完整源码 JSON；固定 unittest 8 + 9 项（含多组 subTest）在隔离 Docker 中运行。
- 源码不在 host 执行；容器无网络、只读 root/夹具、非 root、无新增权限，20 秒超时，不挂载凭据。
- 不拉取镜像；缺 Docker/镜像/Python 明确 not_run。没有吞吐、长上下文或 GPU 内存压力验收。

产物目录：/root/benchmarks/gemma4-96k-70043de5/runs/qwen38-27b-coding-retry1/checks-20261104T165250Z-20261105-2305583-i9gtug
