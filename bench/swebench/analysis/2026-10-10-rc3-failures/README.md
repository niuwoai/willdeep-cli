# CLI 0.94.0-rc3 失败与退步分析

两轮共50个未解决终态，分类是诊断标签，不修改官方成绩；判定失败不等于已证明补丁因果。

## 已确认的主要问题

- DeepSeek 7题均伴随上游502 failover_exhausted；其中6题只有system/user消息、零已报告token。第7题在执行过程中失败。HTTP响应前有界重试已存在，不能通过无限重试或换模型美化原分数。
- 6个历史30题配对退步中5个空补丁：4个全Agent预算停止，1个只读探索耗尽本Agent预算。
- DeepSeek django-13821有38次只读工具调用，最后仍在翻历史；未产生修改。
- DeepSeek django-14017、matplotlib-14623与GLM django-13837最终在委派实现附近耗尽预算，无父工作树补丁。GLM sphinx-8035委派后继续大段读取，最终无补丁。持久化证据可能遗漏子会话，不能断言子Agent从未编辑。
- DeepSeek django-13089补丁通过全部FAIL_TO_PASS，但PASS_TO_PASS的test_expiration失败；它属于计时相关缓存测试，可能是环境波动，也可能是回归。未受控复核前不能断言代码退步，原判定保持。

## 通用改进优先级

1. 用持久化主子辅助usage共享预算状态，委派前保留集成/验收预算；不能只告诉父Agent自己的剩余token。预算不足时避免启动长子任务，优先形成可验证最小补丁。
2. 只读停滞检测纳入子Agent、历史查询和工具输出新信息；从提示升级为明确阶段转换，已有定位后停止重复读取。禁止按题号或参考补丁特判。
3. 对修改范围生成行为矩阵，覆盖目标修复、边界输入和现有兼容行为；普通相关测试通过不能替代问题复现和回归检查。
4. 网络错误与模型修复失败分开展示，安全保存checkpoint；未开始响应前有界重试，部分流不能自动重放。rc4已有超时恢复修复，后续候选需从最新develop隔离开发，避免重复实现。

当前Qwen仍使用冻结rc3二进制；本分析不改变评测，不重做原模型答案。上述改进尚未实现或证明提升；需要确定性假Provider/子Agent测试和独立评测验证。已分析60题用于回归，不能宣称它们是未见holdout。

## 全部未解决题

|模型|题目|诊断类别|配对退步|已观察token|补丁字节|
|---|---|---|---|---:|---:|
|deepseek-flash|astropy__astropy-13398|budget_empty_patch||1044433|0|
|deepseek-flash|django__django-10554|no_test_verdict_details||995336|2138|
|deepseek-flash|django__django-11265|target_tests_failed||962946|2786|
|deepseek-flash|django__django-11885|budget_empty_patch||1024580|0|
|deepseek-flash|django__django-13089|regression_tests_failed|是|1007404|912|
|deepseek-flash|django__django-13112|other_empty_patch||970839|0|
|deepseek-flash|django__django-13121|provider_502||0|0|
|deepseek-flash|django__django-13315|provider_502||0|0|
|deepseek-flash|django__django-13590|provider_502||0|0|
|deepseek-flash|django__django-13821|other_empty_patch|是|963198|0|
|deepseek-flash|django__django-14017|budget_empty_patch|是|1003833|0|
|deepseek-flash|django__django-14034|target_tests_failed||988119|735|
|deepseek-flash|django__django-15930|target_tests_failed||981270|2330|
|deepseek-flash|django__django-16145|regression_tests_failed||1039085|144803|
|deepseek-flash|django__django-16256|target_tests_failed||877063|3315|
|deepseek-flash|django__django-16315|budget_empty_patch||1034317|0|
|deepseek-flash|django__django-16950|target_tests_failed||1010900|1685|
|deepseek-flash|matplotlib__matplotlib-14623|budget_empty_patch|是|1044185|0|
|deepseek-flash|matplotlib__matplotlib-25332|budget_empty_patch||1005190|0|
|deepseek-flash|matplotlib__matplotlib-26208|budget_empty_patch||1007368|0|
|deepseek-flash|mwaskom__seaborn-3187|target_and_regression_tests_failed||1029163|1720|
|deepseek-flash|pylint-dev__pylint-4551|other_empty_patch||985690|0|
|deepseek-flash|pytest-dev__pytest-7490|provider_502||0|0|
|deepseek-flash|scikit-learn__scikit-learn-25973|budget_empty_patch||1021945|0|
|deepseek-flash|scikit-learn__scikit-learn-26194|provider_502||0|0|
|deepseek-flash|sphinx-doc__sphinx-8035|budget_empty_patch||1015079|0|
|deepseek-flash|sympy__sympy-12489|budget_empty_patch||1005131|0|
|deepseek-flash|sympy__sympy-18199|provider_502||0|0|
|deepseek-flash|sympy__sympy-19346|provider_502||701698|0|
|deepseek-flash|sympy__sympy-20438|budget_empty_patch||1001500|0|
|glm5|astropy__astropy-13398|target_and_regression_tests_failed||1039151|5388|
|glm5|astropy__astropy-13977|target_and_regression_tests_failed||544760|956|
|glm5|django__django-10554|no_test_verdict_details||978481|3658|
|glm5|django__django-11265|target_tests_failed||431737|598|
|glm5|django__django-11885|regression_tests_failed||951957|2490|
|glm5|django__django-13112|target_tests_failed||964645|690|
|glm5|django__django-13837|budget_empty_patch|是|1015440|0|
|glm5|django__django-14034|target_tests_failed||790297|1324|
|glm5|django__django-14500|target_tests_failed||973614|6942|
|glm5|django__django-14725|budget_empty_patch||1041819|0|
|glm5|django__django-16256|target_tests_failed||990084|6614|
|glm5|matplotlib__matplotlib-26208|budget_empty_patch||1006705|0|
|glm5|mwaskom__seaborn-3187|target_tests_failed||1006517|725|
|glm5|pylint-dev__pylint-4551|target_tests_failed||1018509|148123|
|glm5|scikit-learn__scikit-learn-26194|target_tests_failed||466889|2167|
|glm5|sphinx-doc__sphinx-7462|target_tests_failed||398713|1019|
|glm5|sphinx-doc__sphinx-8035|budget_empty_patch|是|1023695|0|
|glm5|sympy__sympy-12489|target_and_regression_tests_failed||994028|4799|
|glm5|sympy__sympy-18199|target_tests_failed||1026079|563|
|glm5|sympy__sympy-20438|target_tests_failed||1001451|513|
