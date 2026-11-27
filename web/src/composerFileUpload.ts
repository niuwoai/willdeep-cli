import type { attachmentText } from "./attachmentMessages";
import { attachmentError, MAX_CHAT_BODY_BYTES, type AttachmentPayload } from "./attachmentModel";

const IMAGE_MIME_TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"] as const;
type ImageMime = (typeof IMAGE_MIME_TYPES)[number];
const TEXT_MIME_TYPES = ["text/plain", "text/markdown", "text/x-markdown"];
// TAB, LF, FF, and CR are valid text whitespace, including manual page breaks.
const TEXT_WHITESPACE_CONTROLS = new Set([0x09, 0x0a, 0x0c, 0x0d]);
const PDF_MIME_TYPE = "application/pdf";
const TYPES_BY_EXTENSION = new Map([
  ["png", "image/png"], ["jpg", "image/jpeg"], ["jpeg", "image/jpeg"],
  ["gif", "image/gif"], ["webp", "image/webp"],
  ["txt", "text/plain"], ["md", "text/markdown"], ["pdf", PDF_MIME_TYPE],
]);

export const COMPOSER_FILE_ACCEPT = [
  ...IMAGE_MIME_TYPES, ...TEXT_MIME_TYPES, PDF_MIME_TYPE,
  ...Array.from(TYPES_BY_EXTENSION.keys(), (extension) => `.${extension}`),
].join(",");
// Leave space for image base64 expansion and for the surrounding chat JSON.
export const COMPOSER_FILE_MAX_BYTES = 700 * 1024;

type ComposerFileErrorKey = Extract<Parameters<typeof attachmentText>[0],
  "fileUnsupported" | "fileTooLarge" | "fileEmpty" | "fileReadFailed" | "fileInvalidText" |
  "filePdfFailed" | "filePdfNoText" | "filePdfLocked" | "filePdfLimit" |
  "textTooLarge" | "imageFailed" | "imageTooLarge" | "batchTooLarge"
>;

export class ComposerFileError extends Error {
  constructor(readonly messageKey: ComposerFileErrorKey, options?: ErrorOptions) {
    super(messageKey, options);
    this.name = "ComposerFileError";
  }
}

function detectImageMime(bytes: Uint8Array): ImageMime | undefined {
  const matches = (signature: readonly number[], offset = 0) =>
    signature.every((byte, index) => bytes[offset + index] === byte);
  if (matches([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return "image/png";
  if (matches([0xff, 0xd8, 0xff])) return "image/jpeg";
  if (matches([0x47, 0x49, 0x46, 0x38, 0x37, 0x61]) || matches([0x47, 0x49, 0x46, 0x38, 0x39, 0x61])) return "image/gif";
  if (matches([0x52, 0x49, 0x46, 0x46]) && matches([0x57, 0x45, 0x42, 0x50], 8)) return "image/webp";
  return undefined;
}

function toBase64(bytes: Uint8Array): string {
  const chunkSize = 8_192;
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }
  return btoa(binary);
}

async function untilAborted<T>(work: Promise<T>, signal?: AbortSignal): Promise<T> {
  if (!signal) return work;
  let onAbort: (() => void) | undefined;
  const cancellation = new Promise<never>((_resolve, reject) => {
    onAbort = () => reject(signal.reason);
    signal.addEventListener("abort", onAbort, { once: true });
    if (signal.aborted) onAbort();
  });
  try {
    return await Promise.race([work, cancellation]);
  } finally {
    if (onAbort) signal.removeEventListener("abort", onAbort);
  }
}

async function readImage(name: string, bytes: Uint8Array, expectedMime: ImageMime, signal?: AbortSignal): Promise<AttachmentPayload> {
  const media_type = detectImageMime(bytes);
  if (media_type !== expectedMime) throw new ComposerFileError("imageFailed");
  const data = toBase64(bytes);
  const image = new Image();
  try {
    await untilAborted(new Promise<void>((resolve, reject) => {
      image.onload = () => resolve();
      image.onerror = () => reject(new ComposerFileError("imageFailed"));
      image.src = `data:${media_type};base64,${data}`;
    }), signal);
  } finally {
    image.onload = null;
    image.onerror = null;
  }
  return { kind: "image", name, media_type, data, width: image.naturalWidth, height: image.naturalHeight };
}

function readText(bytes: Uint8Array): string {
  // Disallow binary control characters even when all bytes happen to be valid UTF-8.
  if (bytes.some((byte) => byte < 0x20 && !TEXT_WHITESPACE_CONTROLS.has(byte))) {
    throw new ComposerFileError("fileInvalidText");
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch (cause) {
    throw new ComposerFileError("fileInvalidText", { cause });
  }
}

export async function readComposerFile(file: File, signal?: AbortSignal): Promise<AttachmentPayload> {
  signal?.throwIfAborted();
  if (file.size === 0) throw new ComposerFileError("fileEmpty");
  if (file.size > COMPOSER_FILE_MAX_BYTES) throw new ComposerFileError("fileTooLarge");

  const declaredMime = file.type.toLowerCase();
  const extension = /\.([^./]+)$/.exec(file.name)?.[1]?.toLowerCase();
  const mime = !declaredMime || declaredMime === "application/octet-stream"
    ? TYPES_BY_EXTENSION.get(extension ?? "")
    : declaredMime;
  const imageMime = IMAGE_MIME_TYPES.find((type) => type === mime);
  if (!mime || (!imageMime && mime !== PDF_MIME_TYPE && !TEXT_MIME_TYPES.includes(mime))) {
    throw new ComposerFileError("fileUnsupported");
  }

  let bytes: Uint8Array<ArrayBuffer>;
  try {
    bytes = new Uint8Array(await untilAborted(file.arrayBuffer(), signal));
  } catch (cause) {
    signal?.throwIfAborted();
    throw new ComposerFileError("fileReadFailed", { cause });
  }
  signal?.throwIfAborted();

  let attachment: AttachmentPayload;
  if (imageMime) {
    attachment = await readImage(file.name, bytes, imageMime, signal);
  } else {
    let content: string;
    if (mime === PDF_MIME_TYPE) {
      try {
        const { readComposerPdf } = await untilAborted(import("./composerFileUploadPdf"), signal);
        content = await readComposerPdf(bytes, signal);
      } catch (cause) {
        signal?.throwIfAborted();
        if (cause instanceof ComposerFileError) throw cause;
        throw new ComposerFileError("filePdfFailed", { cause });
      }
    } else {
      content = readText(bytes);
      if (!content.trim()) throw new ComposerFileError("fileEmpty");
    }
    attachment = { kind: "text", name: file.name, content };
  }

  signal?.throwIfAborted();
  const error = attachmentError(attachment);
  if (error) throw new ComposerFileError(error);
  if (new TextEncoder().encode(JSON.stringify(attachment)).length >= MAX_CHAT_BODY_BYTES) {
    throw new ComposerFileError("batchTooLarge");
  }
  return attachment;
}
