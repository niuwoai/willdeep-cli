import { useState } from "react";
import { Button, Text, VStack } from "@chakra-ui/react";
import type { Messages } from "./i18n";
import type { AgentRun, AgentRunsPage } from "./RuntimeSidebar";

export function RuntimeRunHistory({ id, onLoad, messages: t }: {
  id: string; onLoad: (id: string, after?: number) => Promise<AgentRunsPage>; messages: Messages;
}) {
  const [runs, setRuns] = useState<AgentRun[]>([]);
  const [cursor, setCursor] = useState<number | null | undefined>(undefined);
  const [loading, setLoading] = useState(false);
  const [failed, setFailed] = useState(false);
  const statuses: Record<string, string> = { queued: t.queued, running: t.working, completed: t.toolDone, failed: t.toolFailed, blocked: t.blocked, cancelled: t.cancelled, interrupted: t.interrupted, partial: t.partial };
  async function load() {
    setLoading(true); setFailed(false);
    try {
      const page = await onLoad(id, cursor ?? undefined);
      setRuns((previous) => [...previous, ...page.runs.filter((run) => !previous.some((saved) => saved.id === run.id))]);
      setCursor(page.next_after_sequence);
    } catch { setFailed(true); }
    finally { setLoading(false); }
  }
  return <VStack align="stretch" gap="1" mb="2">
    {cursor !== null && <Button size="xs" disabled={loading} onClick={() => void load()}>{loading ? t.runHistoryLoading : cursor === undefined ? t.runHistory : t.moreRuns}</Button>}
    {failed && <Text fontSize="xs" role="alert">{t.runHistoryFailure}</Text>}
    {cursor === null && runs.length === 0 && <Text fontSize="xs">{t.noRunHistory}</Text>}
    {runs.map((run) => <VStack key={run.id} align="stretch" gap="1" borderTop="1px solid" borderColor="var(--border)" py="1">
      <Text fontSize="2xs">{t.currentRun}: {run.sequence} · {statuses[run.status] ?? t.unknownValue} · {run.id}</Text>
      <Text fontSize="2xs">{t.currentRunUsage}: {run.usage.accounted_tokens} {t.tokenUnit} · {t.modelCalls}: {run.usage.model_calls}</Text>
      {run.report && <Text fontSize="xs" whiteSpace="pre-wrap" overflowWrap="anywhere">{run.report}</Text>}
    </VStack>)}
  </VStack>;
}
