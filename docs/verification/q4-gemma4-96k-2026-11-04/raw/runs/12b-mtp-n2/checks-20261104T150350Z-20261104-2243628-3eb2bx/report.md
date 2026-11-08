# 模型独立验收

状态：failed

当前模型：gemma-4-12b-qat

| 检查 | 状态 | 秒数 | 说明 |
| --- | --- | ---: | --- |
| configuration | passed | 0.0 |  |
| authentication | passed | 0.0 |  |
| service_active | failed | 0.006 | FrozenError: can't modify frozen String: "" |
| health | passed | 0.001 |  |
| loaded_model | failed | 0.001 | Encoding::UndefinedConversionError: "\xE2" from ASCII-8BIT to UTF-8 |
| chat_ok | passed | 0.069 |  |
| legacy_alias_1 | passed | 0.048 |  |
| legacy_alias_2 | passed | 0.047 |  |
| forced_tool_add | passed | 0.196 |  |
| tool_round_trip | passed | 0.081 |  |
| json_object | passed | 0.14 |  |
| sandbox_availability | not_run | 0.01 | GemmaAcceptance::NotRun: sandbox 探针不可用：FrozenError: can't modify frozen String: "" |
| programming_dedupe | not_run | 1.406 | GemmaAcceptance::NotRun: 源码已保存，但 sandbox 未可用：FrozenError: can't modify frozen String: "" |
| programming_decimal_refunds | not_run | 4.309 | GemmaAcceptance::NotRun: 源码已保存，但 sandbox 未可用：FrozenError: can't modify frozen String: "" |

## 验收边界

- 每项独立记录；失败不阻止无依赖的后续项目，任何 failed/not_run 均使最终退出码为 1。
- 当前/旧别名均须精确返回 OK，响应 model 须等于当前 model_alias；/props 单独核对实际文件。
- expected_model_file 为文件名时比较 basename；为绝对路径时比较完整 model_path。
- 工具只执行固定白名单 add(19,23)，校验 tool_call_id 往返及正文 42；另验收 JSON object。
- 编程题只接受 solution.py 完整源码 JSON；固定 unittest 8 + 9 项（含多组 subTest）在隔离 Docker 中运行。
- 源码不在 host 执行；容器无网络、只读 root/夹具、非 root、无新增权限，20 秒超时，不挂载凭据。
- 不拉取镜像；缺 Docker/镜像/Python 明确 not_run。没有吞吐、长上下文或 GPU 内存压力验收。

产物目录：/root/benchmarks/gemma4-96k-70043de5/runs/12b-mtp-n2/checks-20261104T150350Z-20261104-2243628-3eb2bx
