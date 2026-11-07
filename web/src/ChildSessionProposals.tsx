import { useRef, useState } from "react";
import { useMutation, useQueryClient, type InfiniteData } from "@tanstack/react-query";
import type { Language } from "./i18n";
import { childProposalText } from "./childProposalMessages";
import { childProposalsKey } from "./childProposalQueries";
import { childSessionsText } from "./childSessionsMessages";
import { childFailureText } from "./childSessionsPresentation";
import { childTasksKey } from "./childSessionsQueries";
import { ChildSessionFailure, dismissChildProposal, startChildProposal, type ChildProposal, type ChildProposalPage, type ChildTask, type ChildTaskPage } from "./childSessionsApi";

type Action = { proposal: string; kind: "start" | "dismiss" };

export function ChildSessionProposals({ sessionId, proposals, tasks, language }: {
  sessionId: string; proposals: ChildProposal[]; tasks: ChildTask[]; language: Language;
}) {
  const t = childSessionsText(language);
  const p = childProposalText(language);
  const cache = useQueryClient();
  const identities = useRef(new Map<string, string>());
  const inFlight = useRef(false);
  const [notice, setNotice] = useState(false);
  const proposalKey = childProposalsKey(sessionId);
  const taskKey = childTasksKey(sessionId);
  const mutation = useMutation({
    mutationFn: async (action: Action) => {
      const key = `${action.kind}:${action.proposal}`;
      let requestId = identities.current.get(key);
      if (!requestId) { requestId = crypto.randomUUID(); identities.current.set(key, requestId); }
      if (action.kind === "dismiss") {
        await dismissChildProposal(sessionId, action.proposal, requestId);
        return null;
      }
      return startChildProposal(sessionId, action.proposal, requestId);
    },
    onSuccess: async (task, action) => {
      await Promise.all([cache.cancelQueries({ queryKey: proposalKey }), cache.cancelQueries({ queryKey: taskKey })]);
      cache.setQueryData<ChildProposalPage>(proposalKey, (page) => page && ({ proposals: page.proposals.filter((item) => item.id !== action.proposal) }));
      if (task) {
        cache.setQueryData<InfiniteData<ChildTaskPage>>(taskKey, (data) => {
          if (!data) return { pages: [{ tasks: [task], next_cursor: null }], pageParams: [null] };
          const pages = data.pages.map((page) => ({ ...page, tasks: page.tasks.filter((item) => item.assignment_id !== task.assignment_id) }));
          if (pages[0]) pages[0] = { ...pages[0], tasks: [task, ...pages[0].tasks] };
          return { ...data, pages };
        });
        setNotice(true);
      }
      identities.current.delete(`${action.kind}:${action.proposal}`);
      await Promise.all([cache.invalidateQueries({ queryKey: taskKey }), cache.invalidateQueries({ queryKey: proposalKey })]);
    },
    onError: async (error) => {
      if (error instanceof ChildSessionFailure && [404, 409].includes(error.status)) {
        await Promise.all([cache.invalidateQueries({ queryKey: taskKey }), cache.invalidateQueries({ queryKey: proposalKey })]);
      }
    },
    onSettled: () => { inFlight.current = false; },
  });
  const act = (action: Action) => {
    // A synchronous guard also covers clicks before React paints the disabled state.
    if (inFlight.current) return;
    inFlight.current = true;
    setNotice(false);
    mutation.mutate(action);
  };
  return <>
    {notice && <p role="status">{t.created}</p>}
    <ol className="child-proposal-list">{proposals.map((proposal) => {
      const value = proposal.params;
      const subscription = value.subscription ?? (value.notify === "none" ? "none" : "auto_wake");
      const active = mutation.variables?.proposal === proposal.id;
      return <li key={proposal.id} className="child-proposal">
        <header><h3>{value.title}</h3><span>{p.proposed}</span></header>
        <p className="child-proposal-brief">{value.prompt}</p>
        <p className="child-session-scope">{value.workspace_mode === "worktree" ? t.worktree : t.readOnly} · {value.context === "fork" ? t.contextFork : t.contextFresh}</p>
        <details className="child-proposal-options"><summary>{p.options}</summary>
          <dl>
            <dt>{p.model}</dt><dd>{value.model || t.modelPlaceholder}</dd>
            <dt>{t.subscriptionLabel}</dt><dd>{t.subscriptions[subscription]}</dd>
            {value.workspace_mode === "worktree" && <><dt>{t.worktreeBaseLabel}</dt><dd>{value.worktree_base === "snapshot" ? t.worktreeSnapshot : t.worktreeHead}</dd></>}
          </dl>
          {!!value.depends_on?.length && <div className="child-dependency-history"><span>{p.dependencies}</span><ul>{value.depends_on.map((id) => <li key={id}>{tasks.find((task) => task.assignment_id === id)?.title ?? t.dependencyUnavailable} <code>{id}</code></li>)}</ul></div>}
          {value.workspace_mode === "worktree" && <p className="child-session-hint">{t.worktreeHint}</p>}
        </details>
        <p className="child-session-hint">{p.hint}</p>
        {active && mutation.error && <p className="child-session-error" role="alert">{childFailureText(mutation.error, language)}</p>}
        <footer>
          <button type="button" disabled={mutation.isPending} onClick={() => act({ proposal: proposal.id, kind: "dismiss" })}>{active && mutation.isPending && mutation.variables?.kind === "dismiss" ? p.dismissing : p.dismiss}</button>
          <button type="button" className="child-proposal-start" disabled={mutation.isPending} onClick={() => act({ proposal: proposal.id, kind: "start" })}>{active && mutation.isPending && mutation.variables?.kind === "start" ? p.starting : p.start}</button>
        </footer>
      </li>;
    })}</ol>
  </>;
}
