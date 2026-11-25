import { useState } from "react";
import { Pencil } from "lucide-react";
import type { Messages } from "./i18n";

type GoalMessages = Pick<Messages, "goalMode" | "goalModePlaceholder" | "goalModeHint" | "goalModeEdit">;
type GoalEditorProps = { goal: string; messages: GoalMessages; onChange: (goal: string) => void };

function GoalEditor({ goal, messages: t, onChange }: GoalEditorProps) {
  const [editing, setEditing] = useState(false);
  const hasGoal = Boolean(goal.trim());

  if (editing || !hasGoal) {
    return (
      <textarea
        className="goal-mode-input"
        rows={2}
        autoFocus={editing}
        value={goal}
        onFocus={() => setEditing(true)}
        onBlur={() => setEditing(false)}
        onChange={(event) => onChange(event.target.value)}
        placeholder={t.goalModePlaceholder}
        aria-label={t.goalMode}
        title={t.goalModeHint}
      />
    );
  }

  return (
    <div className="goal-mode-summary">
      <span className="goal-mode-text">{goal}</span>
      <button type="button" className="goal-mode-edit" onClick={() => setEditing(true)} title={t.goalModeEdit} aria-label={t.goalModeEdit}>
        <Pencil size={14} aria-hidden="true" />
      </button>
    </div>
  );
}

export function GoalModeControl({ goal, enabled, messages: t, onChange }: {
  goal: string;
  enabled: boolean;
  messages: GoalMessages;
  onChange: (goal: string, enabled: boolean) => void;
}) {
  return (
    <div className="goal-mode-control">
      <label className="goal-mode-toggle">
        <input type="checkbox" checked={enabled} onChange={(event) => onChange(goal, event.target.checked)} />
        <span>{t.goalMode}</span>
      </label>
      {enabled && <GoalEditor goal={goal} messages={t} onChange={(nextGoal) => onChange(nextGoal, true)} />}
    </div>
  );
}
