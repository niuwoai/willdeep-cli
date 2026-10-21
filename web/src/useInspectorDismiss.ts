import { useEffect, useRef } from "react";

/** Disclosures keep their drafts mounted; dismissing never discards form state. */
export function useInspectorDismiss() {
  const ref = useRef<HTMLDetailsElement>(null);
  useEffect(() => {
    const dismiss = (event: PointerEvent | KeyboardEvent) => {
      const panel = ref.current;
      if (!panel?.open) return;
      if (event instanceof KeyboardEvent) {
        if (event.key !== "Escape") return;
        panel.open = false;
        panel.querySelector("summary")?.focus();
      } else if (event.target instanceof Node && !panel.contains(event.target)) {
        panel.open = false;
      }
    };
    document.addEventListener("pointerdown", dismiss);
    document.addEventListener("keydown", dismiss);
    return () => {
      document.removeEventListener("pointerdown", dismiss);
      document.removeEventListener("keydown", dismiss);
    };
  }, []);
  return ref;
}
