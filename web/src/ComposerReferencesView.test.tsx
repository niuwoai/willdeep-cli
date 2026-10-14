import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { ComposerReferences } from "./ComposerReferences";
import { messages, languages } from "./i18n";

it("renders MCP then agents then files in each supported language", () => {
  for (const language of languages) {
    const t = messages[language];
    const html = renderToStaticMarkup(<ComposerReferences entries={[
      { kind: "file", name: "中文 file.rs", value: '@file:"中文 file.rs"' },
      { kind: "agent", name: "generalist", value: "@agent:generalist" },
      { kind: "mcp", name: "search", value: "@mcp:search" },
    ]} selected={0} loading={false} messages={t} choose={() => undefined} />);
    expect(html.indexOf(t.referenceMcp)).toBeLessThan(html.indexOf(t.agents));
    expect(html.indexOf(t.agents)).toBeLessThan(html.indexOf(t.referenceFiles));
    expect(html).toContain('aria-selected="true"');
    expect(html).toContain("中文 file.rs");
  }
});
