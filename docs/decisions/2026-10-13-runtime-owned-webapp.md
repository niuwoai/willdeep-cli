# 手机与 Web 服务由 Runtime 托管

用户目标：退出 WillDeep CLI 后，手机与 Web 仍能访问会话、提交任务和处理审批。

## 现状与变更

- 手机中继已有 Runtime 所有权：`mobile_gateway::MobileRelay` 挂在 ServerState，`/mobile` 只调用控制面，启动时读取已保存的启用开关。
- 原 `/webapp` 由 TUI 创建独立 `--web` 子进程，并把 PID、地址写入 webapp-state.json；正常 CLI 退出通常不影响子进程，但 Runtime 不负责其恢复与启停。
- 新 `/webapp` 调用统一 API 的 `webapp.start/status/stop`，Web listener 运行在 daemon 的任务中，不再创建或按 PID 终止独立进程。
- Web 启用状态、监听地址、工作区与配置选择原子保存到私有 webapp-service.json；正常 Runtime 停止关闭 listener，后续启动恢复启用的服务。
- 监听仅允许回环地址，保留原工作区白名单，不因托管扩大可访问目录。
- Web 就绪观察任务也归 Runtime；CLI 在初始化中退出不能丢失实际监听地址。
- `/webapp stop` 持久关闭开关，不在下次启动时恢复。Runtime 离线时也可关闭开关，状态查询不会为了观察而启动 Runtime。
- 前台 `willdeep --web` 与首次设置仍保留原入口；Runtime 托管使用独立关闭信号，不给前台入口新增 daemon 所有权。

## 验收范围

1. mobile 所有权与恢复路径源码核对；实际手机访问仍需运行版本和中继连接证据。
2. `/webapp` 启动、状态、停止经 Runtime API，客户端不持有 listener 或服务进程。
3. Web 请求方退出/断连后，listener 仍可响应，并记录实际地址。
4. Runtime 停止后 listener 关闭、启用意图保留；重启在同一地址恢复。
5. 显式 stop 后不恢复；离线 stop 与只读 status 不拉起 Runtime。
6. 非回环地址、端口占用、无效工作区明确失败，不误杀旧进程。
7. 原前台 Web、首次设置、插件与 Runtime 工作区/审批边界不回归。
8. 旧独立 Web 不自动认领或按状态文件 PID 终止；端口被占用时明确报告，切换时需先结束旧服务。

## 当前证据

`cargo check --workspace --tests --offline --jobs 1` 已通过。Web 生命周期 3 项、命令入口 2 项、现有手机中继 22 项及 Web/插件 Web 40 项测试通过。

独立临时 home 的真实 Runtime 进程验收通过 4 项：CLI 启动进程退出后 Web 可访问；另一个 API 客户端退出不关闭 Web；Runtime 停止关闭监听、重启恢复原端口；显式 Web stop 后重启不恢复。报告与可复跑脚本见 `docs/verification/0.99.0-rc1/runtime-web-smoke.*`。

本机原有 Runtime 在验收前后均为 0.98.0-rc1、同一 PID，手机中继保持连接。本次没有替换已安装 CLI、升级或停止用户正在使用的 Runtime；新 Web 托管代码尚未部署到原安装版。真实手机 UI 操作不属于上述测试证据。
