import { useId, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { Language } from "./i18n";
import { IdentityFailure, loadIdentity, renameIdentity, type IdentityView } from "./identityApi";
import { identityText } from "./identityMessages";
import "./InstanceIdentitySettings.css";

const identityQueryKey = ["instance-identity"] as const;

export function InstanceIdentitySettings({ language, onIdentityChange }: {
  language: Language;
  onIdentityChange: (identity: IdentityView) => void;
}) {
  const t = identityText(language);
  const id = useId();
  const client = useQueryClient();
  const [draft, setDraft] = useState<string | null>(null);
  const identity = useQuery({
    queryKey: identityQueryKey,
    queryFn: ({ signal }) => loadIdentity(signal),
    retry: false,
    staleTime: 0,
    refetchOnWindowFocus: false,
  });
  const rename = useMutation({
    mutationFn: renameIdentity,
    retry: false,
    onSuccess: (next) => {
      client.setQueryData(identityQueryKey, next);
      setDraft(null);
      onIdentityChange(next);
    },
  });
  const name = draft ?? identity.data?.name ?? "";
  const unavailable = identity.isPending || identity.isError || !identity.data;
  const canSave = !unavailable && !identity.isFetching && !rename.isPending && !!name.trim() && name.trim() !== identity.data?.name;
  const error = identity.error ?? rename.error;
  const loopbackOnly = error instanceof IdentityFailure && error.status === 403;
  const failure = error ? loopbackOnly ? t.loopbackOnly
    : `${identity.error ? t.loadFailed : t.saveFailed}: ${error.message}` : null;

  return <section className="instance-identity-settings" aria-labelledby={`${id}-title`}>
    <div className="instance-identity-heading">
      <h2 id={`${id}-title`}>{t.title}</h2>
      <p>{t.description}</p>
    </div>
    <form aria-label={t.title} onSubmit={(event) => {
      event.preventDefault();
      if (canSave) rename.mutate(name.trim());
    }}>
      <div className="instance-identity-controls">
        <input id={`${id}-name`} aria-label={t.title} aria-describedby={`${id}-hint`} aria-invalid={rename.isError || undefined}
          value={name} disabled={unavailable || rename.isPending} autoComplete="off" spellCheck={false}
          onChange={(event) => { setDraft(event.target.value); rename.reset(); }} />
        <button type="submit" disabled={!canSave}>{rename.isPending ? t.saving : t.save}</button>
      </div>
      <p id={`${id}-hint`} className="instance-identity-hint">{t.nameHint}</p>
      {identity.isPending && <p role="status">{t.loading}</p>}
      {failure && <p className="instance-identity-error" role="alert">{failure}</p>}
      {identity.isError && !loopbackOnly && <button className="instance-identity-retry" type="button" disabled={identity.isFetching} onClick={() => void identity.refetch()}>{t.retry}</button>}
      {rename.isSuccess && <p className="instance-identity-success" role="status">{t.saved}</p>}
    </form>
  </section>;
}
