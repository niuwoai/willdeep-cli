import { useRef, useState } from "react";
import { Dialog, Portal } from "@chakra-ui/react";
import { AlertTriangle } from "lucide-react";
import type { WorkspaceAccess } from "./accessModes";
import { ApprovalModeFailure } from "./approvalModeApi";
import { permissionsText } from "./permissionsMessages";
import { workspaceApprovalText } from "./workspaceApprovalMessages";
import type { Language, Messages } from "./i18n";
import "./permissionsPanel.css";

export function WorkspaceApprovalGuide({ path, language, messages, onSave, onCancel }: {
  path: string;
  language: Language;
  messages: Messages;
  onSave: (mode: WorkspaceAccess, confirmed: boolean) => Promise<void>;
  onCancel: () => void;
}) {
  const t = workspaceApprovalText(language);
  const permissions = permissionsText(language);
  const smart = useRef<HTMLInputElement>(null);
  const [mode, setMode] = useState<WorkspaceAccess>("smart");
  const [confirmed, setConfirmed] = useState(false);
  const [saving, setSaving] = useState(false);
  const [failure, setFailure] = useState("");
  const requiresConfirmation = mode === "full_access";
  const options = [
    { mode: "smart" as const, label: t.smart, hint: t.smartHint },
    { mode: "strict" as const, label: t.strict, hint: t.strictHint },
    { mode: "full_access" as const, label: t.full, hint: t.fullHint },
  ];

  async function apply() {
    if (saving || requiresConfirmation && !confirmed) return;
    setSaving(true); setFailure("");
    try { await onSave(mode, confirmed); }
    catch (reason) {
      if (reason instanceof ApprovalModeFailure && reason.status === 403) setFailure(permissions.notWritable);
      else if (reason instanceof ApprovalModeFailure && reason.status === 409) setFailure(permissions.readOnlyWorkspace);
      else setFailure(`${t.failed}${reason instanceof Error && reason.message ? `: ${reason.message}` : ""}`);
    } finally { setSaving(false); }
  }

  return <Dialog.Root open closeOnEscape={!saving} closeOnInteractOutside={!saving} initialFocusEl={() => smart.current} onOpenChange={({ open }) => { if (!open && !saving) onCancel(); }}>
    <Portal>
      <Dialog.Backdrop bg="var(--shadow)" />
      <Dialog.Positioner>
        <Dialog.Content className="workspace-approval-guide" bg="var(--bg-raised)" color="var(--text)" border="1px solid" borderColor="var(--border)" borderRadius="12px" maxW="480px">
          <Dialog.Header><Dialog.Title>{t.title}</Dialog.Title></Dialog.Header>
          <Dialog.Body>
            <Dialog.Description className="workspace-approval-description">{t.subtitle}</Dialog.Description>
            <p className="workspace-approval-path">{path}</p>
            <fieldset className="permissions-options" disabled={saving}>
              <legend>{permissions.choose}</legend>
              {options.map((option) => <label key={option.mode} className={`permissions-option${mode === option.mode ? " selected" : ""}${option.mode === "full_access" ? " dangerous" : ""}`}>
                <input ref={option.mode === "smart" ? smart : undefined} type="radio" name="workspace-approval-mode" value={option.mode} checked={mode === option.mode} onChange={() => { setMode(option.mode); setConfirmed(false); setFailure(""); }} />
                <span><strong>{option.label}</strong><small>{option.hint}</small></span>
              </label>)}
            </fieldset>
            {requiresConfirmation && <div className="permissions-warning" role="note"><AlertTriangle size={15} /><div>
              <strong>{permissions.fullWarningTitle}</strong><p>{permissions.fullWarningBody}</p>
              <label className="permissions-ack"><input type="checkbox" checked={confirmed} disabled={saving} onChange={(event) => setConfirmed(event.target.checked)} />{permissions.fullAck}</label>
            </div></div>}
            {failure && <p className="permissions-failure" role="alert">{failure}</p>}
          </Dialog.Body>
          <Dialog.Footer className="permissions-footer">
            <button type="button" disabled={saving} onClick={onCancel}>{messages.cancel}</button>
            <button type="button" className="permissions-apply" disabled={saving || requiresConfirmation && !confirmed} onClick={() => void apply()}>{saving ? t.saving : t.continue}</button>
          </Dialog.Footer>
        </Dialog.Content>
      </Dialog.Positioner>
    </Portal>
  </Dialog.Root>;
}
