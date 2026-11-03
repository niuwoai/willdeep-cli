import type { Language } from "./i18n";

const zh = {
  title: "实例名",
  description: "给这台 WillDeep 起个名字。Web 与终端共用，改名后身份 UUID 保持不变。",
  nameHint: "支持中文、日文、字母、数字、- 和 _；最多 24 个字符，显示宽度不超过 24 列。",
  loading: "正在读取实例名…",
  loadFailed: "读取实例名失败",
  save: "保存名字",
  saving: "正在保存…",
  saved: "实例名已保存",
  saveFailed: "保存实例名失败",
  loopbackOnly: "请通过本机回环地址打开 Web，才能查看和修改实例名。",
  retry: "重新读取",
};

const en: typeof zh = {
  title: "Instance name",
  description: "Name this WillDeep instance. Web and terminal share the name; renaming keeps the identity UUID unchanged.",
  nameHint: "Chinese, Japanese, letters, digits, - and _ are supported; up to 24 characters and 24 display columns.",
  loading: "Loading instance name…",
  loadFailed: "Could not load the instance name",
  save: "Save name",
  saving: "Saving…",
  saved: "Instance name saved",
  saveFailed: "Could not save the instance name",
  loopbackOnly: "Open Web through this machine’s loopback address to view or change its instance name.",
  retry: "Reload",
};

const ja: typeof zh = {
  title: "インスタンス名",
  description: "この WillDeep に名前を付けます。Web とターミナルで共有され、名前を変更しても UUID は変わりません。",
  nameHint: "中国語・日本語・英数字・-・_ に対応。最大 24 文字、表示幅は 24 桁まで。",
  loading: "インスタンス名を読み込み中…",
  loadFailed: "インスタンス名を読み込めません",
  save: "名前を保存",
  saving: "保存中…",
  saved: "インスタンス名を保存しました",
  saveFailed: "インスタンス名を保存できません",
  loopbackOnly: "インスタンス名の確認・変更には、このマシンのループバックアドレスから Web を開いてください。",
  retry: "再読み込み",
};

export const identityText = (language: Language) => language === "zh-CN" ? zh : language === "ja" ? ja : en;
