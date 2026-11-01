import type { Language } from "./i18n";

// `/model` 选择器的文案。
const zh = {
  title: "选择模型",
  subtitle: "只影响这条会话；下一轮起生效。",
  loading: "正在读取可用模型…",
  loadFailed: "读取模型列表失败",
  empty: "配置文件里还没有可选的模型。先去「配置」里加一个 Provider。",
  applying: "正在切换…",
  applied: "已切换到 {model}",
  failed: "切换模型失败",
  notWritable: "只有在本机回环地址打开这个页面时才能切换模型。",
  noSession: "先打开或新建一条会话，再选模型。",
  close: "关闭",
  current: "当前",
  usage: "用法：/model 打开选择器，或 /model 模型名 直接切换。",
  followDefault: "跟随 Provider 默认",
  pluginBlocked: "模型只能由你亲手切换：插件页面不能替这条会话换模型。",
};

const en: typeof zh = {
  title: "Choose a model",
  subtitle: "Applies to this session only, from the next turn on.",
  loading: "Loading available models…",
  loadFailed: "Could not load the model list",
  empty: "No models are configured yet. Add a provider under Configuration first.",
  applying: "Switching…",
  applied: "Switched to {model}",
  failed: "Could not switch models",
  notWritable: "Models can only be switched from this machine over loopback.",
  noSession: "Open or start a session before choosing a model.",
  close: "Close",
  current: "Current",
  usage: "Usage: /model opens the picker, or /model <name> switches directly.",
  followDefault: "Provider default",
  pluginBlocked: "Only you can switch the model; a plugin page cannot do it for this session.",
};

const ja: typeof zh = {
  title: "モデルを選択",
  subtitle: "このセッションだけに適用され、次のターンから有効になります。",
  loading: "利用可能なモデルを読み込み中…",
  loadFailed: "モデル一覧を読み込めません",
  empty: "設定にモデルがありません。先に「設定」で Provider を追加してください。",
  applying: "切り替え中…",
  applied: "{model} に切り替えました",
  failed: "モデルの切り替えに失敗しました",
  notWritable: "モデルの切り替えはこのマシンのループバック経由でのみ可能です。",
  noSession: "先にセッションを開くか作成してください。",
  close: "閉じる",
  current: "現在",
  usage: "使い方：/model で選択画面、/model モデル名 で直接切り替え。",
  followDefault: "Provider の既定",
  pluginBlocked: "モデルの切り替えは本人のみ可能です。プラグイン画面からは変更できません。",
};

export const modelText = (language: Language) => language === "zh-CN" ? zh : language === "ja" ? ja : en;
