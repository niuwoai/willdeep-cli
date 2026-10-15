# 公共免费复现评分：工程验收

56 项测试、154 断言通过；0 失败、0 错误。六个公开案例实际执行可信 fixture/reference 共 20 次。

| 案例 | 格式 | 分类 | 定位 | 证据 | 复现 | 宿主函数执行数 | 通过 |
|---|---|---|---|---|---|---|---|
| count-zero-bug | true | true | true | true | verified | 2 | true |
| count-zero-clean | true | true | true | true | not_applicable_clean | 6 | true |
| copy-boundary-bug | true | true | true | true | verified | 2 | true |
| copy-boundary-clean | true | true | true | true | not_applicable_clean | 4 | true |
| duplicate-first-bug | true | true | true | true | verified | 2 | true |
| duplicate-first-clean | true | true | true | true | not_applicable_clean | 4 | true |

反例覆盖：伪造 expected/actual、未触发缺陷、无复现、错误行、缺读取/未交付/文件修改、额外代码与路径、重复键、float/exponent/-0、原型键、深度/数量/字符串/字节超限、非白名单源码、fixture/reference 篡改、符号链接越界、可信无限循环超时、过大输出及无 process/require 沙箱边界。

宿主证据与模型报告分开；真实执行结果与模型声明逐项核对。clean 的复现维度是 N/A，公开探针通过不代表证明所有输入。reason 仅校验类型和非空，不做语义 judge。

模型调用 0、费用 0、网络调用 0；旧付费批次/旧 grader/旧分数保持不动。该报告只证明免费工程链路，不宣称模型质量改善或版本晋升。
