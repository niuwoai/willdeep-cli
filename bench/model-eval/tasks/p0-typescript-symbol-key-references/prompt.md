将 `Formatter` 的 `formatLabel` 方法重命名为 `renderLabel`，更新对象实现及其在 `src/view.ts` 中的全部语义引用。保留现有三种调用形式：点访问、字符串键访问和带别名的解构。字符串键 `formatter["formatLabel"]` 属于目标引用，必须迁移；同名的独立导出函数 `formatLabel`、审计对象的同名属性和普通字符串 `"formatLabel"` 都不属于目标，必须保留。

仅允许修改 `src/formatter.ts` 和 `src/view.ts`，其他文件均受保护，包括 `src/audit.ts`、`package.json` 和 `test/`。不要增加文件或修改测试。先定位定义与引用，再确认允许写集，并运行 `ruby test/verify.rb`。

本任务使用 Node.js 22.18.0 或更新版本内置的 TypeScript 类型擦除，另需 Ruby；不安装 npm/TypeScript，不访问网络。宿主对固定引用形态做结构断言，并运行多输入行为测试；这不等于完整 TypeScript 类型检查，也不验证 LSP 引用响应或编辑工具协议。
