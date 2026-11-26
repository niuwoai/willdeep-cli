import type { Language } from "./i18n";

export const markdownMessages = {
  "zh-CN": {
    openLink: "在浏览器中打开",
    preview: "预览文件",
    loading: "正在加载文件…",
    unavailable: "无法预览此文件。请确认它是当前会话工作区内的文本文件，且不超过 256 KiB。",
    close: "关闭预览",
  },
  en: {
    openLink: "Open in browser",
    preview: "Preview file",
    loading: "Loading file…",
    unavailable: "Could not preview this file. Check that it is a text file in this session’s workspace and no larger than 256 KiB.",
    close: "Close preview",
  },
  ja: {
    openLink: "ブラウザーで開く",
    preview: "ファイルをプレビュー",
    loading: "ファイルを読み込み中…",
    unavailable: "ファイルをプレビューできません。このセッションのワークスペース内にある 256 KiB 以下のテキストファイルか確認してください。",
    close: "プレビューを閉じる",
  },
} satisfies Record<Language, Record<string, string>>;
