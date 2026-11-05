import { useId, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { Language } from "./i18n";
import { childSessionsText } from "./childSessionsMessages";
import { changeChildControl, ChildSessionFailure, getChildLink, type ChildControlState, type ChildLink } from "./childSessionsApi";
import { childLinkKey, childTasksKey, updateChildControl } from "./childSessionsQueries";

import { childFailureText } from "./childSessionsPresentation";

export function ChildSessionControl({ parentId, childId, state = "delegated", language, onChanged }: {
  parentId: string; childId: string; state?: ChildControlState; language: Language;
  onChanged?: (link: ChildLink) => void;
}) {
  const t = childSessionsText(language);
  const cache = useQueryClient();
  const headingId = useId();
  const [confirmation, setConfirmation] = useState<"takeover" | "detach" | null>(null);
  const [saved, setSaved] = useState(false);
  const identities = useRef(new Map<string, string>());
  const link = useQuery({
    queryKey: childLinkKey(parentId, childId),
    queryFn: ({ signal }) => getChildLink(parentId, childId, signal),
    enabled: confirmation !== null, retry: false,
  });
  // Control only moves away from delegation; an older cached Link must not restore buttons.
  const current = state === "detached" || link.data?.control_state === "detached" ? "detached"
    : state === "user_owned" || link.data?.control_state === "user_owned" ? "user_owned" : "delegated";
  const mutation = useMutation({
    mutationFn: ({ kind, revision }: { kind: "takeover" | "detach"; revision: number }) => {
      const key = `${kind}:${revision}`;
      let id = identities.current.get(key);
      if (!id) { id = crypto.randomUUID(); identities.current.set(key, id); }
      return changeChildControl(parentId, childId, kind, revision, id);
    },
    onSuccess: async (updated, action) => {
      identities.current.delete(`${action.kind}:${action.revision}`);
      updateChildControl(cache, parentId, childId, updated);
      setConfirmation(null); setSaved(true);
      onChanged?.(updated);
      await cache.invalidateQueries({ queryKey: childTasksKey(parentId) });
    },
    onError: async (error) => {
      if (error instanceof ChildSessionFailure && error.status === 409) {
        const refreshed = await link.refetch();
        if (refreshed.data) { updateChildControl(cache, parentId, childId, refreshed.data); onChanged?.(refreshed.data); }
        await cache.invalidateQueries({ queryKey: childTasksKey(parentId) });
      }
    },
  });
  const allowed = confirmation === "takeover" ? current === "delegated" : current !== "detached";
  const open = (kind: "takeover" | "detach") => { mutation.reset(); setSaved(false); setConfirmation(kind); };
  return <div className="child-control">
    <div className="child-task-actions">
      {current === "delegated" && <button type="button" disabled={mutation.isPending} onClick={() => open("takeover")}>{t.takeover}</button>}
      {current !== "detached" && <button type="button" disabled={mutation.isPending} onClick={() => open("detach")}>{t.detach}</button>}
    </div>
    {confirmation && <div className="child-control-confirm" role="dialog" aria-labelledby={headingId}>
      <h3 id={headingId}>{confirmation === "takeover" ? t.confirmTakeover : t.confirmDetach}</h3>
      <p>{confirmation === "takeover" ? t.takeoverHint : t.detachHint}</p><p className="child-session-hint">{t.controlLimits}</p>
      {(link.isPending || link.isFetching) && <p role="status">{t.controlLoading}</p>}
      {link.error && <p className="child-session-error" role="alert">{t.controlError}</p>}
      {link.error && <button type="button" onClick={() => void link.refetch()}>{t.refresh}</button>}
      {link.data && <p>{t.controlStates[current]}</p>}
      {mutation.error && <p className="child-session-error" role="alert">{childFailureText(mutation.error, language)}</p>}
      <div className="child-task-actions">
        <button type="button" disabled={mutation.isPending} onClick={() => { setConfirmation(null); mutation.reset(); }}>{t.cancel}</button>
        <button type="button" disabled={mutation.isPending || link.isFetching || !link.data || !!link.error || !allowed} onClick={() => {
          if (link.data && allowed) mutation.mutate({ kind: confirmation, revision: link.data.control_revision });
        }}>{mutation.isPending ? t.working : confirmation === "takeover" ? t.confirmTakeover : t.confirmDetach}</button>
      </div>
    </div>}
    {saved && <p role="status">{t.controlSaved}</p>}
  </div>;
}
