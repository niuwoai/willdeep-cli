# Changelog

## 0.1.0-rc2 · 2026-11-04

- 新增 `test:packages`：使用临时 HOME 运行本地 Yarn 打包，审计归档清单及源码字节，解包后执行各包原生测试和真实 Magpie 宿主验证，通过后交付本地 tarball。
- `test:host` 补齐共享根目录不误检测、独立 CLI PATH 检测，支持指定 `--packages-root`，输出宿主二进制 SHA-256 和契约源码基线。
- 修复 Ruby ASCII locale 下的宿主 UTF-8 输出匹配，并单独断言无已安装 Agent 时的退出码与错误分类。
- 两包同步至 `0.1.0-rc2`；消费端最低版本仍为 CLI `0.142.0-rc1`、Desktop `1.427.0-rc1`。

## 0.1.0-rc1 · 2026-11-04

- 提供两个独立本地 Agent 插件、严格 JSON v1 连接字段、图标及中英文提示。
- 添加原生 JS 测试、隔离真实 Magpie 宿主驱动、安装说明及社区材料草案。
- 仅本地源码和包交付；不发布 npm 或提交社区。
