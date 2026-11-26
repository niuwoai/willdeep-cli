export async function writeClipboard(text: string) {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return;
    }
  } catch {
    // 非安全上下文或被拒绝时改走选区复制。
  }
  const selection = window.getSelection();
  const anchor = selection?.anchorNode;
  const focus = selection?.focusNode;
  const anchorOffset = selection?.anchorOffset ?? 0;
  const focusOffset = selection?.focusOffset ?? 0;
  const active = document.activeElement;
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.style.position = "fixed";
  area.style.left = "-9999px";
  document.body.appendChild(area);
  area.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  } finally {
    area.remove();
    if (active instanceof HTMLElement && active.isConnected && active !== document.activeElement) active.focus({ preventScroll: true });
    if (selection && anchor?.isConnected && focus?.isConnected) {
      selection.setBaseAndExtent(anchor, anchorOffset, focus, focusOffset);
    }
  }
  if (!ok) throw new Error("copy failed");
}
