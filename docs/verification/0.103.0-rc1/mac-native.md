# Mac P4b 免费审查 v2 验收

版本 **1.421.0-rc1 / build921**。原生定向测试 **14/14，4套**，其中13项有断言；旧宿主控制入口没有控制文件，直接返回占1项，未执行trial。另有免费入口Ruby门禁 **5项/25断言**通过。

新路径实际执行6个公共fixture、12次进程内mock请求。共享12条原始JSON正反例、词法/歧义、深度、全树项数、UTF8字节、保留键、只读权限、模式、路径及原定义绑定检查通过。

共享宿主评分器对本次原生raw与真实读取/交付证据验收 **6/6**，实际执行冻结fixture/reference **20次**；另一个独立进程完成78项绑定检查与同样20次执行，原生文件前后哈希不变。原生状态仍是unverified；clean复现维度明确N/A。首次CLI错误地将限长读取的UTF8内容标成ASCII-8BIT，修复CLI边界后用新目录重验，旧失败与原生原件均保留。

| 公共fixture | 原生终态 | 独立宿主复现 | 宿主执行次数 |
|---|---|---|---|
| count-zero-bug | unverified | verified | 2 |
| count-zero-clean | unverified | not_applicable_clean | 6 |
| copy-boundary-bug | unverified | verified | 2 |
| copy-boundary-clean | unverified | not_applicable_clean | 4 |
| duplicate-first-bug | unverified | verified | 2 |
| duplicate-first-clean | unverified | not_applicable_clean | 4 |

工程构建退出0，用时496.44秒；test-without-building退出0，用时10.07秒。成功构建/测试最低余量分别约7.10/6.97GiB；此前43.17秒的一次构建被6GiB监视器正常中断（退出75），属于环境空间不足。复用独立写时复制缓存，不触原冻结App/DD，不声称提速倍数。

共享contract哈希为`91b66fce5a84e96340573e43a0743c5d8205b02d228e9240a4947cc86dad163a`，instructions哈希为`ace7d00ee76f9234df3fe934624ba443402c7dee80255972128199e4ed4c9ae8`。只说明输出指令与任务原字节相同，两个平台的完整系统提示及其effective_prompt哈希独立。旧付费控制协议、旧baseline包和原冻结App/测试包/两脚本均保持不变。真实模型请求与费用均为0。

这是公开工程fixture的mock链路和客观复现验证，不是新盲测成绩，不证明模型质量提升，不自动晋升；未merge/push/tag/release/install。完整本地结果保留于build/rsi-p4b-app-tests、build/rsi-free-review-v2、build/rsi-free-review-host-final，独立互通报告在主方案的host-interop-mac-01验收目录。
