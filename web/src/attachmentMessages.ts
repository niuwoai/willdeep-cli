import type { Language } from "./i18n";
const index: Record<Language, number> = { "zh-CN": 0, en: 1, ja: 2 };
const messages = {
  title: ["附件", "Attachments", "添付ファイル"], manage: ["管理附件", "Manage attachments", "添付を管理"],
  capacity: ["最多 {limit} 个", "Up to {limit}", "最大 {limit} 件"], more: ["附件排序", "Reorder attachment", "添付の順序"],
  characters: ["{count} 字符", "{count} characters", "{count} 文字"],
  batchTooLarge: ["消息和附件超过当前 Web 请求大小上限（1 MiB），请删减或缩小附件后再发送。", "The message and attachments exceed the Web request limit (1 MiB). Reduce them before sending.", "メッセージと添付が Web の上限（1 MiB）を超えています。小さくして送信してください。"],
  count: ["{count}/{limit} 个附件", "{count}/{limit} attachments", "{count}/{limit} 件の添付"],
  preview: ["预览附件", "Preview attachment", "添付をプレビュー"], close: ["关闭预览", "Close preview", "プレビューを閉じる"],
  delete: ["删除附件", "Remove attachment", "添付を削除"], clear: ["清空附件", "Clear attachments", "添付をすべて削除"],
  clearConfirm: ["清空这些未发送的附件？", "Clear these unsent attachments?", "未送信の添付をすべて削除しますか？"],
  up: ["上移", "Move up", "上に移動"], down: ["下移", "Move down", "下に移動"],
  edit: ["编辑文本", "Edit text", "テキストを編集"], save: ["保存文本", "Save text", "テキストを保存"], cancel: ["取消编辑", "Cancel editing", "編集をキャンセル"],
  text: ["文本", "Text", "テキスト"], image: ["图片", "Image", "画像"],
  limit: ["最多添加 12 个附件，请先删除部分附件。", "At most 12 attachments. Remove some before adding more.", "添付は最大 12 件です。追加する前に削除してください。"],
  textTooLarge: ["文本附件不能超过 200000 个字符。", "Text attachments cannot exceed 200000 characters.", "テキストは 200000 文字までです。"],
  imageTooLarge: ["图片过大，请缩小后再添加。", "The image is too large. Resize it before adding.", "画像が大きすぎます。縮小して追加してください。"],
  imageFailed: ["无法读取图片，请检查格式或文件是否损坏。", "Cannot read the image. Check its format and contents.", "画像を読み込めません。形式と内容を確認してください。"],
  copy: ["引用到新草稿", "Copy to a new draft", "新しい下書きに引用"], readonly: ["已发送附件只读", "Sent attachments are read-only", "送信済みの添付は読み取り専用"],
} as const;
export function attachmentText(key: keyof typeof messages, language: Language) { return messages[key][index[language]]; }
