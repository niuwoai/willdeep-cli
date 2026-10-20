# 流式输出渲染性能验收

日期：2026-10-20。待发布版本：0.108.0-rc6。没有安装、重启或修改用户当前客户端、Runtime、原任务和插话。

## 方法与边界

使用真实 TUI `draw` 函数和 ratatui TestBackend（160×50），构造相同的中文、Markdown、代码块历史。每组预热一帧后测量 40 次两字增量，修复前后各重复三次。使用优化构建，排除模型、网关和网络波动。

另用独立线程每 20ms 发送一个片段，共 100 个，记录发送到 TUI 消费并写入测试终端后的延迟。它测量客户端事件排队和绘制，不代表屏幕物理刷新、网络耗时或模型首字耗时。

## 三次重复的平均单帧耗时

| 历史条数 | 修复前 ms | 修复后 ms | 加速比 |
| --- | ---: | ---: | ---: |
| 20 | 0.774 | 0.711 | 1.1x |
| 500 | 13.355 | 0.636 | 21.0x |
| 2000 | 53.595 | 0.801 | 66.9x |

2000 条历史时，修复前三次的 40 帧全部超过 20ms 输入间隔；修复后三次均为 0 帧超过该间隔。CPU 时间与逐次原始结果见 [JSON](streaming-performance.json)。

定速输入的显示延迟 P95 为 1.80–4.68ms。最大单次延迟 27.34ms，存在调度抖动，但未出现随片段数增长的持续积压，100 个片段全部保留。

## 修复与功能验证

- 缓存已完成历史的 Markdown、颜色和换行结果，避免每个增量重复处理整段历史两遍。
- 只复制并绘制可见行；保留全文复制选区的全局行坐标。
- 历史内容、宽度、搜索词、工具折叠与语言变化会使缓存失效；与原完整渲染路径逐行、逐样式对比一致。
- Chat Completions 思考内容首片立即输出；连续到达的后续片段按 100ms、换行或 120 字分批，不等待固定字数才首次显示。正文增量不加定速延迟。
- TUI 261 项通过；Chat Completions 流式测试 12 项通过；优化版性能/渲染测试 4 项通过；Clippy all-targets `-D warnings`、格式、diff、源文件大小检查通过。

## 复现

```sh
cargo test -p willdeep --release --bin willdeep deterministic_stream_render_benchmark -- --ignored --nocapture
cargo test -p willdeep --release --bin willdeep paced_stream_display_latency -- --ignored --nocapture
cargo test -p willdeep --bin willdeep tui::
cargo test -p willdeep-core provider::chat_completions::streaming
```

修复前使用相同合成 fixture 测量原始完整渲染路径；原始数据与 CPU 时间已保留。以上证明客户端长历史渲染确实存在可复现瓶颈并已明显改善，不排除独立存在上游生成、网络或真实终端的延迟。
