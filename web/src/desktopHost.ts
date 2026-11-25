const currentQuery = () => new URLSearchParams(window.location.search);
export const isDesktop = currentQuery().get("desktop") === "1";

type Selection = { workspace: string | null; session: string | null };
let selection: Selection | null = null;
let savedSelection = "";
let saving = Promise.resolve();
const selectionEndpoint = "/api/desktop/selection";

export async function restoreDesktopSelection() {
  const locationQuery = currentQuery();
  if (!isDesktop || locationQuery.has("workspace") || locationQuery.has("session")) return;
  try {
    const response = await fetch(selectionEndpoint);
    if (!response.ok) return;
    selection = await response.json() as Selection;
    savedSelection = JSON.stringify(selection);
  } catch (error) {
    console.warn("Could not read desktop selection", error);
  }
}

export function desktopLocation() {
  const locationQuery = currentQuery();
  return {
    workspace: isDesktop ? locationQuery.get("workspace") ?? selection?.workspace ?? null : null,
    session: locationQuery.get("session") ?? selection?.session ?? null,
    fresh: isDesktop && locationQuery.get("fresh") === "1",
  };
}

export const desktopDraftId = (workspace: string, session: string) => JSON.stringify([workspace, session]);

export function saveDesktopSelection(workspace: string, session: string | null) {
  if (!isDesktop || !workspace) return;
  const body = JSON.stringify({ workspace, session });
  if (savedSelection === body) return;
  savedSelection = body;
  saving = saving.then(async () => {
    const response = await fetch(selectionEndpoint, {
      method: "PUT", headers: { "content-type": "application/json" }, body, keepalive: true,
    });
    if (!response.ok) throw new Error(String(response.status));
  }).catch((error: unknown) => {
    if (savedSelection === body) savedSelection = "";
    console.warn("Could not save desktop selection", error);
  });
}

export function pickDesktopWorkspace() {
  window.location.assign("/__willdeep_native/open-folder");
}

export function openDesktopProjectWindow(workspace: string) {
  const url = new URL("/", window.location.origin);
  url.searchParams.set("desktop", "1");
  url.searchParams.set("workspace", workspace);
  url.searchParams.set("fresh", "1");
  window.open(url, "_blank", "noopener");
}
