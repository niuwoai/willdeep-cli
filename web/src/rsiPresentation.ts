const PREFIX = "willdeep.rsi.presentation.v1/";
const MAX_NOTE = 160;
export function readRsiNote(scope: string, identity: string): string {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(`${PREFIX}${scope}`) ?? "{}");
    if (value && typeof value === "object" && identity in value) {
      const note = (value as Record<string, unknown>)[identity];
      return typeof note === "string" ? note.slice(0, MAX_NOTE) : "";
    }
  } catch { /* Presentation metadata is optional; unavailable storage never blocks a trial. */ }
  return "";
}
export function writeRsiNote(scope: string, identity: string, note: string): boolean {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(`${PREFIX}${scope}`) ?? "{}");
    const notes = stored && typeof stored === "object" && !Array.isArray(stored) ? stored : {};
    localStorage.setItem(`${PREFIX}${scope}`, JSON.stringify({...notes, [identity]: note.slice(0, MAX_NOTE)}));
    return true;
  } catch { return false; }
}
