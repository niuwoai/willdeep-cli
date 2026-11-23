# 未知斜杠输入真实 TUI 验收

- 结果：通过
- 隔离 TUI、Runtime 和本机假模型，真实模型请求 0 次。
- 用例：4/4
- 失败：无

- 通过：`/tmp/slash-fixture/project Please inspect this directory`
- 通过：`/wat Please treat this as ordinary input`
- 通过：`/help/file Please inspect this file`
- 通过：`/verbose on`
