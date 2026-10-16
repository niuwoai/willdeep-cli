import type { Language } from "./i18n";
import { agentText } from "./agentLibraryMessages";

export function allowAgentNavigation(dirty: boolean, language: Language): boolean {
  return !dirty || window.confirm(agentText("discardDraft", language));
}
