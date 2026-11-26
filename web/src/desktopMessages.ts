import type { Language } from "./i18n";

const desktopMessages = {
  "zh-CN": {
    changes: "工作区改动", changedFiles: "改动 {count} 个文件", refresh: "刷新改动",
    expandProjects: "展开项目栏", collapseProjects: "收起项目栏", close: "收起改动面板",
    loading: "正在读取改动…", pickFile: "选择文件查看差异",
    empty: "当前工作区没有改动", noGit: "请打开 Git 工作树的根目录查看改动",
    noWorkspace: "先打开一个工作区", binary: "二进制文件", noDiff: "没有可显示的文本差异",
    fromSession: "本会话涉及", otherChanges: "其他或归属未知的改动",
    attributionScope: "记录只标记涉及的文件。下方差异可能包含其他来源的改动。",
    fileDiff: "文件差异", staged: "已暂存", unstaged: "未暂存", untracked: "未跟踪",
    staleSnapshot: "工作区已变化，请刷新改动。",
    failed: "无法读取改动", desktopVersion: "桌面 {version}", runtimeVersion: "Runtime {version}",
    protocolVersion: "协议 {version}", runtimeUnavailable: "Runtime 未连接",
  },
  en: {
    changes: "Workspace changes", changedFiles: "{count} changed files", refresh: "Refresh changes",
    expandProjects: "Show projects", collapseProjects: "Hide projects", close: "Hide changes",
    loading: "Loading changes…", pickFile: "Select a file to see its diff",
    empty: "No changes in this workspace", noGit: "Open the Git worktree root to view changes",
    noWorkspace: "Open a workspace first", binary: "Binary file", noDiff: "No text diff available",
    fromSession: "Touched in this session", otherChanges: "Other or unattributed changes",
    attributionScope: "Session records identify files. This diff may include edits from other sources.",
    fileDiff: "File diff", staged: "Staged", unstaged: "Unstaged", untracked: "Untracked",
    staleSnapshot: "The workspace has changed. Refresh to see the current diff.",
    failed: "Could not load changes", desktopVersion: "Desktop {version}", runtimeVersion: "Runtime {version}",
    protocolVersion: "Protocol {version}", runtimeUnavailable: "Runtime unavailable",
  },
  ja: {
    changes: "ワークスペースの変更", changedFiles: "{count} 個の変更ファイル", refresh: "変更を更新",
    expandProjects: "プロジェクトを表示", collapseProjects: "プロジェクトを非表示", close: "変更を閉じる",
    loading: "変更を読み込み中…", pickFile: "ファイルを選択して差分を表示",
    empty: "ワークスペースに変更はありません", noGit: "変更を表示するには Git ワークツリーのルートを開いてください",
    noWorkspace: "ワークスペースを開いてください", binary: "バイナリファイル", noDiff: "表示できる差分がありません",
    fromSession: "このセッションで変更を検出", otherChanges: "その他または変更元が不明なファイル",
    attributionScope: "記録はファイル単位です。差分には別の変更元による編集が含まれることがあります。",
    fileDiff: "ファイルの差分", staged: "ステージ済み", unstaged: "未ステージ", untracked: "未追跡",
    staleSnapshot: "ワークスペースが変更されました。差分を更新してください。",
    failed: "変更を読み込めませんでした", desktopVersion: "デスクトップ {version}", runtimeVersion: "ランタイム {version}",
    protocolVersion: "プロトコル {version}", runtimeUnavailable: "ランタイムに接続できません",
  },
} satisfies Record<Language, Record<string, string>>;

export const desktopText = (language: Language) => desktopMessages[language];
