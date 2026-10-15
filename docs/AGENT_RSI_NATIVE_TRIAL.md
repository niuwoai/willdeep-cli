# 受控原生 Worker 评测适配

版本：0.102.0-rc2。本入口只服务已经单独确认的 reviewer-original-24-v1 提案；普通 `prepare/run` 的离线准入保持原样。实现和免费夹具测试不代表真实模型评测已经执行。

`willdeep agents rsi native-trial --control /private/path/control.json` 执行一个样本尝试。控制文件必须是 0600 常规文件、不可为符号链接，传入路径不携带 capability。`--preview` 在真实 Worker 组装第一请求后立即停止，不发送 HTTP；保存不含凭据的 `first_request.json`，可核对系统提示词、工具和原始 UTF-8 字节上界。

控制 JSON 的字段为：

```json
{
  "schema": "willdeep.agent-rsi.native-trial-control.v1",
  "proposal_sha256": "a3ae36e96d268f8aaab8a02efacea61cb68f58c1d3bf812b9a38311890736606",
  "experiment_id": "pilot-example",
  "trial_id": "trial-example",
  "arm": "baseline",
  "repetition": 1,
  "attempt": 1,
  "resume_of": null,
  "agent_package": "/private/path/baseline.json",
  "agent_revision": "51b79a923ceceeae80a7cc6e20f2f0dbdfdb136f1bc516b895450ea633273bdd",
  "suite": "/private/path/reviewer-original-24-v1/suite.json",
  "suite_sha256": "281f7dadf99f4cf86c4de5a922cdac7531c933654f878bb6b8f532d502a96575",
  "task_id": "loop-bound-bug",
  "broker_origin": "http://127.0.0.1:12345",
  "capability": "REPLACE_WITH_BROKER_TRIAL_CAPABILITY",
  "output_directory": "/private/path/new-trial-result"
}
```

`output_directory` 必须尚不存在，父目录由宿主预先创建。Candidate arm 必须使用提案冻结的 candidate revision；代码不接受任意模型、套件或定义。第二次尝试需显式引用父 trial；是否允许基础设施重试、holdout 访问和本次实际费率由预算代理决定，客户端 JSON 不是付费批准。

Worker 只接收原套件 prompt 和随机独立工作区里的 fixture.txt，不接收 task ID、标准答案或控制文件。宿主不改变生产 Worker 的报告格式规则，不调用摘要、优化器或 LLM 判官。四轮、16000 token、120 秒是每尝试上限；所有模型请求只发往本机代理，参数固定 `model=deepseek-v4-flash, stream=false, temperature=0, max_tokens=4000`。HTTP 禁止代理和重定向，不做后台重试，不读取产品密钥或 Provider 配置。每 trial capability 只出现在本机 Authorization header。

`observation.json` 保留实际 ExecutionContext、工具事件、Worker 报告与摘要、模型最后无 tool call 的原始正文、每次 request / receipt / response ID、实际 route、代理确认的逻辑模型和原始 usage。X-RSI-Approved-Model 必须等于批准逻辑名，X-Routed-Model 保留不透明上游标识并由预算代理检查冻结映射，不能混作同一个名称。未知用量保持 null，不补零。Worker 本身的内部 attempt 与外层实验 attempt 分别保留，不将新 Worker 伪装成旧运行。

适配器的独立机制验证只检查实际成功 read_file、本题 fixture 的结果通过匹配 tool_call_id 进入下一次已回执模型请求、fixture 未变和 Worker 完成，输入绑定实际正文 / 报告及工具事件；质量评分状态保持 pending。统一宿主 Ruby 评分器从原始正文提取唯一完整 JSON 对象，严格校验 has_defect / line / reason 三字段，分别报告分类、定位、结构、真实读取。分类和精确行判定对照宿主 oracle，reason 仅检查非空，不以 LLM 判断语义。通用能力提升或官方晋升不能由本小套件推断。
