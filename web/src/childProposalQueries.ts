import { useQuery } from "@tanstack/react-query";
import { listChildProposals } from "./childSessionsApi";

export const childProposalsKey = (parent: string) => ["child-session-proposals", parent];

export function useChildProposals(parent: string) {
  return useQuery({
    queryKey: childProposalsKey(parent),
    queryFn: ({ signal }) => listChildProposals(parent, signal),
    refetchInterval: 4000,
    retry: false,
  });
}
