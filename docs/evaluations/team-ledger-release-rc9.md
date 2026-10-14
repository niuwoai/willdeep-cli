# SWE-bench 运行时强化验证

候选版本：0.99.0-rc9

|检查|状态|秒数|
|---|---|---|
|format|通过|1.24|
|clippy|通过|19.2|
|workspace-tests|通过|290.31|
|web-lint|通过|2.18|
|web-build|通过|5.88|
|source-size|通过|0.21|
|diff|通过|0.03|

这是机制与工程回归，不是新的 SWE-bench 模型成绩。真实费用及解决率提升未经本次测试证明。

Windows 目标 Core 全目标 Clippy 交叉验证通过。Windows 原生动态测试、公开资产和运行实例采用情况另行核验；本报告只记录本地集成回归，不将忽略项算作通过。
