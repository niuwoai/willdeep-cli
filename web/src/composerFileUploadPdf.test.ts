import adobeGb1Map from "pdfjs-dist/cmaps/Adobe-GB1-UCS2.bcmap?url&inline";
import uniGbMap from "pdfjs-dist/cmaps/UniGB-UCS2-H.bcmap?url&inline";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_TEXT_CHARACTERS } from "./attachmentModel";
import { readComposerFile } from "./composerFileUpload";
import { readComposerPdf } from "./composerFileUploadPdf";

// Connect the real PDF.js API and parser over browser-compatible MessagePorts
// so the worker protocol can be exercised in the Node test environment.
let initializeParser: (port: MessagePort) => void;
const terminate = vi.fn();
const startWorker = vi.fn<(port: MessagePort) => void>();
const fetchResource = vi.fn<typeof fetch>();
const cMaps = new Map([
  ["UniGB-UCS2-H.bcmap", uniGbMap],
  ["Adobe-GB1-UCS2.bcmap", adobeGb1Map],
]);

beforeAll(async () => {
  await import("./composerFileUploadWorker");
  const scope = globalThis as typeof globalThis & {
    pdfjsWorker: { WorkerMessageHandler: { initializeFromPort: (port: MessagePort) => void } };
  };
  const handler = scope.pdfjsWorker.WorkerMessageHandler;
  initializeParser = (port) => handler.initializeFromPort(port);
});

beforeEach(() => {
  terminate.mockClear();
  startWorker.mockReset().mockImplementation(initializeParser);
  fetchResource.mockReset().mockImplementation(async (input) => {
    const url = String(input);
    const filename = url.startsWith("/node_modules/pdfjs-dist/cmaps/") ? url.split("/").at(-1) : undefined;
    const dataUrl = filename ? cMaps.get(filename) : undefined;
    if (!dataUrl) throw new Error(`Unexpected PDF resource URL: ${url}`);
    const bytes = Uint8Array.from(atob(dataUrl.split(",")[1]), (character) => character.charCodeAt(0));
    return new Response(bytes);
  });
  vi.stubGlobal("fetch", fetchResource);
  vi.stubGlobal("Worker", class {
    private readonly channel = new MessageChannel();
    readonly addEventListener = this.channel.port1.addEventListener.bind(this.channel.port1);
    readonly removeEventListener = this.channel.port1.removeEventListener.bind(this.channel.port1);
    readonly postMessage = this.channel.port1.postMessage.bind(this.channel.port1);

    constructor() {
      startWorker(this.channel.port2);
      this.channel.port1.start();
      this.channel.port2.start();
    }

    terminate() {
      this.channel.port1.close();
      this.channel.port2.close();
      terminate();
    }
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("PDF composer file", () => {
  it("extracts text from a valid PDF using the PDF.js parser", async () => {
    await expect(readComposerFile(pdfFile(["First page", "Second page"]))).resolves.toEqual({
      kind: "text", name: "article.pdf", content: "First page\n\nSecond page",
    });
    expect(terminate).toHaveBeenCalledOnce();
    expect(fetchResource).not.toHaveBeenCalled();
  });

  it("loads only the packaged CMaps needed to extract Chinese without a ToUnicode mapping", async () => {
    await expect(readComposerFile(cjkPdfFile())).resolves.toEqual({
      kind: "text", name: "article.pdf", content: "中文文档",
    });
    expect(fetchResource.mock.calls.map(([url]) => url)).toEqual(
      Array.from(cMaps.keys(), (name) => `/node_modules/pdfjs-dist/cmaps/${name}`),
    );
    for (const [, options] of fetchResource.mock.calls) {
      expect(options).toMatchObject({ mode: "same-origin", redirect: "error" });
    }
    expect(terminate).toHaveBeenCalledOnce();
  });

  it("rejects a PDF with no text and closes the worker", async () => {
    await expect(readComposerFile(pdfFile([""]))).rejects.toMatchObject({ messageKey: "filePdfNoText" });
    expect(terminate).toHaveBeenCalledOnce();
  });

  it("rejects a corrupt PDF and closes the worker", async () => {
    const file = new File(["%PDF-1.4\ncorrupt"], "corrupt.pdf", { type: "application/pdf" });
    await expect(readComposerFile(file)).rejects.toMatchObject({ messageKey: "filePdfFailed" });
    expect(terminate).toHaveBeenCalledOnce();
  });

  it("rejects PDFs with more than forty pages", async () => {
    await expect(readComposerFile(pdfFile(Array.from({ length: 41 }, () => "Text")))).rejects.toMatchObject({ messageKey: "filePdfLimit" });
    expect(terminate).toHaveBeenCalledOnce();
  });

  it("stops extraction at the text character limit", async () => {
    const text = "A".repeat(MAX_TEXT_CHARACTERS / 5);
    await expect(readComposerFile(pdfFile(Array<string>(5).fill(text), 1_000_000))).rejects.toMatchObject({ messageKey: "textTooLarge" });
    expect(terminate).toHaveBeenCalledOnce();
  });

  it("terminates a worker that does not respond before the deadline", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    startWorker.mockImplementation(() => {});
    const bytes = new TextEncoder().encode("%PDF-1.4");
    const result = expect(readComposerPdf(bytes)).rejects.toMatchObject({ messageKey: "filePdfLimit" });
    await vi.advanceTimersByTimeAsync(10_000);
    await result;
    expect(terminate).toHaveBeenCalledOnce();
  });

  it("does not start a worker for an already-aborted PDF read", async () => {
    const signal = AbortSignal.abort();
    const bytes = new TextEncoder().encode("%PDF-1.4");
    await expect(readComposerPdf(bytes, signal)).rejects.toBe(signal.reason);
    expect(startWorker).not.toHaveBeenCalled();
  });

  it("aborts a pending CMap request and terminates the parser without a cleanup handshake", async () => {
    fetchResource.mockImplementation(() => new Promise(() => {}));
    const controller = new AbortController();
    const result = readComposerFile(cjkPdfFile(), controller.signal);
    await vi.waitFor(() => expect(fetchResource).toHaveBeenCalledOnce());
    const requestSignal = fetchResource.mock.calls[0][1]?.signal;
    expect(requestSignal?.aborted).toBe(false);
    controller.abort();
    await expect(result).rejects.toBe(controller.signal.reason);
    expect(requestSignal?.aborted).toBe(true);
    expect(terminate).toHaveBeenCalledOnce();
  });
});

function pdfFile(pages: string[], pageWidth = 200): File {
  const pageRefs = pages.map((_page, index) => `${4 + 2 * index} 0 R`).join(" ");
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    `<< /Type /Pages /Kids [${pageRefs}] /Count ${pages.length} >>`,
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ...pages.flatMap((text, index) => {
      const stream = text ? `BT /F1 18 Tf 20 20 Td (${text}) Tj ET` : "";
      return [
        `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${pageWidth} 100] /Resources << /Font << /F1 3 0 R >> >> /Contents ${5 + 2 * index} 0 R >>`,
        pdfStream(stream),
      ];
    }),
  ];
  return assemblePdf(objects);
}

function cjkPdfFile(): File {
  // Use the predefined Adobe maps for the UCS-2 bytes for 中文文档.
  return assemblePdf([
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [4 0 R] /Count 1 >>",
    "<< /Type /Font /Subtype /Type0 /BaseFont /STSong-Light /Encoding /UniGB-UCS2-H /DescendantFonts [6 0 R] >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>",
    pdfStream("BT /F1 18 Tf 20 20 Td <4E2D658765876863> Tj ET"),
    "<< /Type /Font /Subtype /CIDFontType0 /BaseFont /STSong-Light /CIDSystemInfo << /Registry (Adobe) /Ordering (GB1) /Supplement 2 >> /DW 1000 >>",
  ]);
}

function pdfStream(content: string): string {
  return `<< /Length ${content.length} >>\nstream\n${content}\nendstream`;
}

function assemblePdf(objects: string[]): File {
  let source = "%PDF-1.4\n";
  const offsets = objects.map((object, index) => {
    const offset = source.length;
    source += `${index + 1} 0 obj\n${object}\nendobj\n`;
    return `${String(offset).padStart(10, "0")} 00000 n \n`;
  });
  const xref = source.length;
  const size = objects.length + 1;
  source += `xref\n0 ${size}\n0000000000 65535 f \n${offsets.join("")}trailer\n<< /Size ${size} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return new File([source], "article.pdf", { type: "application/pdf" });
}
