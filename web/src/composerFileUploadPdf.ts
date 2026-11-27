import { getDocument, PasswordException, PDFWorker, VerbosityLevel, type PDFDocumentProxy } from "pdfjs-dist/legacy/build/pdf.mjs";
import type { TextContent } from "pdfjs-dist/types/src/display/api";
import { MAX_TEXT_CHARACTERS } from "./attachmentModel";
import { ComposerFileError } from "./composerFileUpload";

const MAX_PDF_PAGES = 40;
const MAX_PDF_READ_MS = 10_000;
const PDF_HEADER_SEARCH_BYTES = 1_024;
// Vite serves the installed dependency in development and emits its CMaps
// under assets/ for the embedded production server.
const PDF_CMAP_BASE_URL = `${import.meta.env.BASE_URL}${import.meta.env.DEV
  ? "node_modules/pdfjs-dist/cmaps/"
  : "assets/pdf-cmaps/"}`;

function cMapDataFactory(signal: AbortSignal) {
  return class {
    async fetch({ kind, filename }: { kind: string; filename: string }): Promise<Uint8Array> {
      if (kind !== "cMapUrl" || !/^[a-z0-9-]+\.bcmap$/i.test(filename)) {
        throw new Error(`Unsupported PDF.js resource: ${kind}/${filename}`);
      }
      const response = await fetch(`${PDF_CMAP_BASE_URL}${filename}`, { mode: "same-origin", redirect: "error", signal });
      if (!response.ok) throw new Error(`Could not load PDF CMap ${filename}: HTTP ${response.status}`);
      return new Uint8Array(await response.arrayBuffer());
    }
  };
}

async function collectText(pdf: PDFDocumentProxy): Promise<string> {
  if (pdf.numPages > MAX_PDF_PAGES) throw new ComposerFileError("filePdfLimit");
  const fragments: string[] = [];
  let characters = 0;
  const append = (fragment: string) => {
    if (!fragment) return;
    const codePoints = fragment[Symbol.iterator]();
    while (!codePoints.next().done) {
      if (++characters > MAX_TEXT_CHARACTERS) throw new ComposerFileError("textTooLarge");
    }
    fragments.push(fragment);
  };

  for (let pageNumber = 1; pageNumber <= pdf.numPages; pageNumber++) {
    if (pageNumber > 1) append("\n\n");
    const page = await pdf.getPage(pageNumber);
    const stream: ReadableStream<TextContent> = page.streamTextContent();
    const reader = stream.getReader();
    try {
      for (;;) {
        const chunk = await reader.read();
        if (chunk.done) break;
        for (const item of chunk.value.items) {
          if ("str" in item) append(item.str + (item.hasEOL ? "\n" : ""));
        }
      }
    } finally {
      reader.releaseLock();
    }
  }

  const content = fragments.join("").trim();
  if (!content) throw new ComposerFileError("filePdfNoText");
  return content;
}

export async function readComposerPdf(bytes: Uint8Array<ArrayBuffer>, signal?: AbortSignal): Promise<string> {
  signal?.throwIfAborted();
  if (!String.fromCharCode(...bytes.subarray(0, PDF_HEADER_SEARCH_BYTES)).includes("%PDF-")) {
    throw new ComposerFileError("filePdfFailed");
  }

  // Supplying a real Worker port prevents PDF.js from falling back to parsing
  // untrusted PDFs on the UI thread when a worker cannot start.
  const port = new Worker(new URL("./composerFileUploadWorker.ts", import.meta.url), { type: "module" });
  let worker: PDFWorker | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let onAbort: (() => void) | undefined;
  const resourceRequests = new AbortController();
  try {
    const interruption = new Promise<never>((_resolve, reject) => {
      if (signal) {
        onAbort = () => reject(signal.reason);
        signal.addEventListener("abort", onAbort, { once: true });
        if (signal.aborted) onAbort();
      }
      timer = setTimeout(() => reject(new ComposerFileError("filePdfLimit")), MAX_PDF_READ_MS);
      port.addEventListener("error", (event) => reject(new ComposerFileError("filePdfFailed", { cause: event.error })), { once: true });
      port.addEventListener("messageerror", () => reject(new ComposerFileError("filePdfFailed")), { once: true });
    });
    worker = PDFWorker.create({ port, verbosity: VerbosityLevel.ERRORS });
    const task = getDocument({
      data: bytes,
      worker,
      stopAtErrors: true,
      disableWorkerRendering: true,
      disableFontFace: true,
      useSystemFonts: false,
      // Only CMaps are needed for extraction. Keep font/wasm requests disabled.
      useWorkerFetch: false,
      BinaryDataFactory: cMapDataFactory(resourceRequests.signal),
      useWasm: false,
      verbosity: VerbosityLevel.ERRORS,
    });
    const extract = async () => {
      const content = await collectText(await task.promise);
      await task.destroy();
      return content;
    };
    return await Promise.race([extract(), interruption]);
  } catch (cause) {
    signal?.throwIfAborted();
    if (cause instanceof PasswordException) throw new ComposerFileError("filePdfLocked", { cause });
    throw cause;
  } finally {
    clearTimeout(timer);
    if (onAbort) signal?.removeEventListener("abort", onAbort);
    resourceRequests.abort();
    worker?.destroy();
    // Termination cannot depend on a document cleanup handshake after a timeout.
    port.terminate();
  }
}
