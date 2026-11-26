import { createContext } from "react";
import type { MarkdownCopyLabels } from "./markdownCopy";
import { markdownMessages } from "./markdownMessages";

export const MarkdownMessageContext = createContext<{ sessionId?: string | null; labels: typeof markdownMessages.en }>({
  labels: markdownMessages.en,
});

export const CopyLabelsContext = createContext<MarkdownCopyLabels>({
  copy: "复制",
  copied: "已复制",
  failed: "复制失败",
});
