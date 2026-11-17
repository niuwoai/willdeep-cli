# 自定义智能体启动回归

版本：willdeep 0.152.0-rc2。结果：通过。临时配置、临时智能体库与 Git 工作区外的临时目录，仅使用回环模型夹具。

- PASS unknown section fails with config path and actionable library guidance
- PASS valid config loads after preserving/removing the misplaced section
- PASS custom definition remains enabled with its independent model and 65536 window binding
- PASS real PTY reaches the interactive greeting with the custom agent installed
- PASS TUI /agents interaction returns the installed custom definition
- PASS local CLI dispatch executes custom instructions and returns its result to the parent
- PASS runtime CLI dispatch executes custom instructions and returns its result to the parent
- PASS both dispatch paths used the specialist model without contacting external providers
