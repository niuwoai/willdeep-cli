import type { ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ChakraProvider, defaultSystem } from "@chakra-ui/react";
import { describe, expect, it } from "vitest";
import { McpAccess } from "./McpAccess";
import { messages } from "./i18n";

function render(node: ReactNode) {
  return renderToStaticMarkup(<ChakraProvider value={defaultSystem}>{node}</ChakraProvider>);
}

describe("McpAccess", () => {
  it.each(["zh-CN", "en", "ja"] as const)("%s 打开时说明用途并给出创建入口", (language) => {
    const html = render(
      <McpAccess messages={messages[language]} open onOpenChange={() => undefined} />,
    );
    expect(html).toContain(messages[language].mcpTitle);
    // 英文说明里的撇号会被转义成实体，取撇号之前的部分断言。
    expect(html).toContain(messages[language].mcpIntro.split("'")[0]);
    expect(html).toContain(messages[language].mcpCreate);
    // 还没创建 token 时不出现安装指令，避免复制到一个空的。
    expect(html).not.toContain("Bearer");
  });

  it("关闭时不渲染内容", () => {
    const html = render(
      <McpAccess messages={messages["zh-CN"]} open={false} onOpenChange={() => undefined} />,
    );
    expect(html).not.toContain(messages["zh-CN"].mcpIntro);
  });
});
