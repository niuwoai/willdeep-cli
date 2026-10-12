# Runtime 托管 Web 进程验收

版本：0.99.0-rc1。独立临时 home、工作区和假 Provider；没有调用模型，也没有停止或升级用户原有 Runtime。

- 通过：CLI 启动进程退出后，Runtime 托管的 Web 仍可访问
- 通过：独立 API 客户端退出不关闭 Web 监听
- 通过：Runtime 停止关闭 Web，重启恢复原监听地址
- 通过：显式关闭 Web 的开关跨 Runtime 重启保持关闭

构建产物 SHA-256 与逐项结果见同名 JSON。
