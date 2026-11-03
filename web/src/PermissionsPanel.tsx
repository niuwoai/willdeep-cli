import { useEffect, useState } from "react";
import { AlertTriangle, Check, ChevronDown, ShieldCheck, X } from "lucide-react";
import { accessLabel, isDangerousAccess, selectableAccessModes, type WorkspaceAccess } from "./accessModes";
import { ApprovalModeFailure, changeApprovalMode } from "./approvalModeApi";
import { permissionsText } from "./permissionsMessages";
import { useInspectorDismiss } from "./useInspectorDismiss";
import type { Language, Messages } from "./i18n";
import "./permissionsPanel.css";

export function PermissionsPanel({ sessionId, language, messages, override, workspaceAccess, open, onOpenChange, onSaved, onApplied }: {
  sessionId: string;
  language: Language;
  messages: Messages;
  /** 会话里选过的档位；`null` 表示跟随工作区策略。 */
  override: WorkspaceAccess | null;
  /** 工作区策略，是会话档位的硬上限。 */
  workspaceAccess: WorkspaceAccess;
  /** 由 App 持有：`/permissions` 命令也要能把面板摊开。 */
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved: (mode: WorkspaceAccess) => void;
  /** 保存成功后调用：面板收起，成功提示交给常驻的宿主显示。 */
  onApplied?: (message: string) => void;
}) {
  const t = permissionsText(language);
  const panel = useInspectorDismiss();
  const readOnlyWorkspace = workspaceAccess === "read_only";
  const effective = override ?? workspaceAccess;
  const [draft, setDraft] = useState<WorkspaceAccess>(readOnlyWorkspace ? "strict" : effective);
  const [acknowledged, setAcknowledged] = useState(false);
  const [pending, setPending] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  // 外部（换会话、TUI 那边切换档位、/permissions 命令）改了当前值或开合状态时，
  // 面板要跟上，而不是停在自己那份过期的草稿上。
  useEffect(() => {
    setDraft(readOnlyWorkspace ? "strict" : effective);
    setAcknowledged(false);
  }, [effective, readOnlyWorkspace]);
  useEffect(() => {
    const node = panel.current;
    if (node && node.open !== open) node.open = open;
  }, [open, panel]);

  const needsAcknowledgement = isDangerousAccess(draft) && !acknowledged;
  const unchanged = draft === override || (override === null && draft === workspaceAccess);

  async function apply() {
    if (needsAcknowledgement || pending || readOnlyWorkspace || unchanged) return;
    setPending(true); setFailure(null); setNotice(null);
    try {
      await changeApprovalMode(sessionId, draft);
      onSaved(draft);
      setAcknowledged(false);
      const text = t.saved.replace("{mode}", accessLabel(draft, messages));
      // 档位已经生效，工具栏的按钮就是新的标签；面板留着只会挡住对话。
      // 失败时保持摊开，错误原因要留在原处。
      onApplied?.(text);
      onOpenChange(false);
      setNotice(text);
    } catch (reason) {
      if (reason instanceof ApprovalModeFailure && reason.status === 403) setFailure(t.notWritable);
      else if (reason instanceof ApprovalModeFailure && reason.status === 409) setFailure(t.readOnlyWorkspace);
      else setFailure(`${t.failed}: ${reason instanceof Error ? reason.message : String(reason)}`);
    } finally { setPending(false); }
  }

  const hint = (mode: WorkspaceAccess) => mode === "strict" ? t.strictHint
    : mode === "smart" ? t.smartHint
    : mode === "workspace_write" ? t.workspaceWriteHint
    : t.fullAccessHint;

  return <aside className="conversation-permissions">
    {/* 不受控：开合由浏览器自己管，外部只通过 `open` 单向同步，避免 React 和
        details 各自改同一份状态时互相打架。 */}
    <details ref={panel} className="conversation-disclosure" onToggle={(event) => onOpenChange(event.currentTarget.open)}>
      <summary aria-label={`${t.title} · ${accessLabel(effective, messages)}`} title={t.title}>
        <ShieldCheck size={16} /><span>{accessLabel(effective, messages)}</span><ChevronDown size={13} />
      </summary>
      <div className="conversation-inspector permissions-inspector">
        <div className="inspector-heading">
          <div><h2>{t.title}</h2><p>{t.subtitle}</p></div>
          <div className="inspector-controls">
            <button className="inspector-icon" type="button" aria-label={t.close} title={t.close} onClick={() => onOpenChange(false)}><X size={16} /></button>
          </div>
        </div>

        <dl className="permissions-current">
          <dt>{t.current}</dt>
          <dd>{accessLabel(effective, messages)}{override === null && <em>{t.following}</em>}</dd>
          <dt>{t.workspacePolicy}</dt>
          <dd>{accessLabel(workspaceAccess, messages)}</dd>
        </dl>

        {readOnlyWorkspace && <p className="permissions-blocked" role="note">{t.readOnlyWorkspace}</p>}

        <fieldset className="permissions-options" disabled={readOnlyWorkspace || pending}>
          <legend>{t.choose}</legend>
          {selectableAccessModes.map((mode) => (
            <label key={mode} className={`permissions-option${mode === draft ? " selected" : ""}${isDangerousAccess(mode) ? " dangerous" : ""}`}>
              <input type="radio" name="approval-mode" value={mode} checked={mode === draft} onChange={() => { setDraft(mode); setAcknowledged(false); setNotice(null); setFailure(null); }} />
              <span><strong>{accessLabel(mode, messages)}</strong><small>{hint(mode)}</small></span>
            </label>
          ))}
        </fieldset>

        {isDangerousAccess(draft) && <div className="permissions-warning" role="note">
          <AlertTriangle size={15} />
          <div>
            <strong>{t.fullWarningTitle}</strong>
            <p>{t.fullWarningBody}</p>
            <label className="permissions-ack"><input type="checkbox" checked={acknowledged} onChange={(event) => setAcknowledged(event.target.checked)} />{t.fullAck}</label>
          </div>
        </div>}

        {failure && <p className="permissions-failure" role="alert">{failure}</p>}
        {notice && <p className="permissions-notice" role="status">{notice}</p>}

        <div className="permissions-footer">
          <button type="button" className="permissions-apply" disabled={pending || readOnlyWorkspace || needsAcknowledgement || unchanged} onClick={() => void apply()}>
            <Check size={14} />{pending ? t.applying : t.apply}
          </button>
        </div>
      </div>
    </details>
  </aside>;
}
