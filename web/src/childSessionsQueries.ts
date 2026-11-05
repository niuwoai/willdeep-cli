import { useQuery, type InfiniteData, type QueryClient } from "@tanstack/react-query";
import { CHILD_CAPABILITIES, getChildCapabilities, type ChildLink, type ChildTask, type ChildTaskPage } from "./childSessionsApi";

export const childTasksKey = (parent: string) => ["child-session-tasks", parent];
export const childLinkKey = (parent: string, child: string) => ["child-session-link", parent, child];
export const CHILD_DEPENDENCY_LIMIT = 16;

export function useChildCapabilities(parent: string) {
  const query = useQuery({
    queryKey: ["child-session-capabilities", parent],
    queryFn: ({ signal }) => getChildCapabilities(parent, signal),
    staleTime: 30_000, retry: false,
  });
  const capabilities = query.data?.capabilities ?? [];
  return {
    ...query,
    control: capabilities.includes(CHILD_CAPABILITIES.control),
    subscriptions: capabilities.includes(CHILD_CAPABILITIES.subscriptions),
    dependencies: capabilities.includes(CHILD_CAPABILITIES.dependencies),
  };
}
export type ChildFeatures = { subscriptions: boolean; dependencies: boolean };

export function updateChildTask(cache: QueryClient, parent: string, task: ChildTask) {
  cache.setQueryData<InfiniteData<ChildTaskPage>>(childTasksKey(parent), (data) => data && ({
    ...data, pages: data.pages.map((page) => ({ ...page, tasks: page.tasks.map((old) => old.assignment_id === task.assignment_id ? task : old) })),
  }));
}
export function updateChildControl(cache: QueryClient, parent: string, child: string, link: ChildLink) {
  cache.setQueryData(childLinkKey(parent, child), link);
  cache.setQueryData<InfiniteData<ChildTaskPage>>(childTasksKey(parent), (data) => data && ({
    ...data, pages: data.pages.map((page) => ({ ...page, tasks: page.tasks.map((task) => task.child_session_id === child
      ? { ...task, control_state: link.control_state, control_revision: link.control_revision } : task) })),
  }));
}
