import { createContext } from "react";
import type { MarkdownCopyLabels } from "./markdownCopy";

export const CopyLabelsContext = createContext<MarkdownCopyLabels>({
  copy: "复制",
  copied: "已复制",
  failed: "复制失败",
});
