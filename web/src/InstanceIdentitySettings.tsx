import { useId, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { Language } from "./i18n";
import { IdentityFailure, loadIdentity, renameIdentity, type IdentityView } from "./identityApi";
import { identityText } from "./identityMessages";

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

  return <form className="row builtin-identity-row" data-search={`${t.title} instance name`.toLowerCase()} aria-label={t.title} onSubmit={event => {
    event.preventDefault();
    if (canSave) rename.mutate(name.trim());
  }}>
    <label className="row-label" htmlFor={`${id}-name`}>{t.title}</label>
    <div className="row-control">
      <input id={`${id}-name`} type="text" aria-describedby={`${id}-hint`} aria-invalid={rename.isError || undefined}
        value={name} disabled={unavailable || rename.isPending} autoComplete="off" spellCheck={false}
        onChange={event => { setDraft(event.target.value); rename.reset(); }} />
      <button className="btn btn-sm" type="submit" disabled={!canSave}>{rename.isPending ? t.saving : t.save}</button>
    </div>
    <div className="row-help" id={`${id}-hint`}><span>{t.description}</span> <span>{t.nameHint}</span></div>
    {identity.isPending && <div className="row-help" role="status">{t.loading}</div>}
    {failure && <div className="row-help error" role="alert">{failure}</div>}
    {identity.isError && !loopbackOnly && <div className="row-help"><button className="btn btn-sm" type="button" disabled={identity.isFetching} onClick={() => void identity.refetch()}>{t.retry}</button></div>}
    {rename.isSuccess && <div className="row-help" role="status">{t.saved}</div>}
  </form>;
}
