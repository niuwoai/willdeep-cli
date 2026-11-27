export type AttachmentPayload = { kind: "text"; name: string; content: string } | { kind: "image"; name: string; media_type: string; data: string; width: number; height: number };
export type DraftAttachment = AttachmentPayload & { id: string };
export const MAX_ATTACHMENTS = 12;
export const MAX_TEXT_CHARACTERS = 200_000;
export const MAX_IMAGE_BASE64 = 20_000_000;
export const MAX_IMAGE_DIMENSION = 12_000;
export const MAX_CHAT_BODY_BYTES = 1024 * 1024;
// Leave room for the prompt and submission fields. Send and enqueue also measure the final body.
const CHAT_BODY_RESERVE_BYTES = 8 * 1024;
export const MAX_INLINE_PASTE_CHARACTERS = 100;

export function chatBodyFits(body: string, reservedBytes = 0): boolean {
  return new TextEncoder().encode(body).byteLength <= MAX_CHAT_BODY_BYTES - reservedBytes;
}

export function draftAttachmentsFit(attachments: AttachmentPayload[]): boolean {
  return chatBodyFits(JSON.stringify({ attachments }), CHAT_BODY_RESERVE_BYTES);
}

export function shouldAttachPastedText(text: string): boolean {
  if (text.length <= MAX_INLINE_PASTE_CHARACTERS) return false;
  const characters = text[Symbol.iterator]();
  for (let count = 0; count <= MAX_INLINE_PASTE_CHARACTERS; count++) {
    if (characters.next().done) return false;
  }
  return true;
}
export function attachmentPayload(item: DraftAttachment): AttachmentPayload {
  if (item.kind === "text") return { kind: item.kind, name: item.name, content: item.content };
  return { kind: item.kind, name: item.name, media_type: item.media_type, data: item.data, width: item.width, height: item.height };
}
export function attachmentError(attachment: AttachmentPayload): "textTooLarge" | "imageTooLarge" | null {
  if (attachment.kind === "text") return Array.from(attachment.content).length > MAX_TEXT_CHARACTERS ? "textTooLarge" : null;
  return attachment.data.length > MAX_IMAGE_BASE64 || attachment.width < 1 || attachment.height < 1 || attachment.width > MAX_IMAGE_DIMENSION || attachment.height > MAX_IMAGE_DIMENSION ? "imageTooLarge" : null;
}
export function moveAttachment(items: DraftAttachment[], id: string, delta: number): DraftAttachment[] {
  const index = items.findIndex((item) => item.id === id);
  const target = index + delta;
  if (index < 0 || target < 0 || target >= items.length) return items;
  const result = [...items];
  [result[index], result[target]] = [result[target], result[index]];
  return result;
}
