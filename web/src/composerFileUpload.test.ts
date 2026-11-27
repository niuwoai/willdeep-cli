import { afterEach, describe, expect, it, vi } from "vitest";
import { COMPOSER_FILE_ACCEPT, COMPOSER_FILE_MAX_BYTES, ComposerFileError, readComposerFile } from "./composerFileUpload";
import { MAX_IMAGE_DIMENSION, MAX_TEXT_CHARACTERS } from "./attachmentModel";

const pdf = vi.hoisted(() => ({ readComposerPdf: vi.fn<(bytes: Uint8Array, signal?: AbortSignal) => Promise<string>>() }));
vi.mock("./composerFileUploadPdf", () => pdf);

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  pdf.readComposerPdf.mockReset();
});

describe("readComposerFile", () => {
  it("restricts the chooser to supported types and known extensions", () => {
    expect(COMPOSER_FILE_ACCEPT.split(",").sort()).toEqual([
      ".gif", ".jpeg", ".jpg", ".md", ".pdf", ".png", ".txt", ".webp", "application/pdf",
      "image/gif", "image/jpeg", "image/png", "image/webp", "text/markdown", "text/plain", "text/x-markdown",
    ]);
  });

  it("reads UTF-8 Markdown without a provided MIME, preserving whitespace", async () => {
    const content = "# 标题\r\n\n正文\n";
    const file = new File([content], "notes.MD");
    await expect(readComposerFile(file)).resolves.toEqual({ kind: "text", name: "notes.MD", content });
  });

  it("accepts legacy Markdown MIME and preserves text page breaks", async () => {
    const content = "# 第一页\f## 第二页";
    const file = new File([content], "notes", { type: "text/x-markdown" });
    await expect(readComposerFile(file)).resolves.toEqual({ kind: "text", name: "notes", content });
  });

  it.each([
    { file: new File([], "empty.txt"), key: "fileEmpty" },
    { file: new File([" \n\t"], "blank.txt"), key: "fileEmpty" },
    { file: new File(["unsupported"], "mystery"), key: "fileUnsupported" },
    { file: new File(["<svg/>"], "vector.svg", { type: "image/svg+xml" }), key: "fileUnsupported" },
    { file: new File([new Uint8Array([0xff])], "bad.txt"), key: "fileInvalidText" },
    { file: new File(["binary\0data"], "bad.txt"), key: "fileInvalidText" },
    { file: new File(["a".repeat(MAX_TEXT_CHARACTERS + 1)], "long.txt"), key: "textTooLarge" },
  ])("rejects $file.name with $key", async ({ file, key }) => {
    await expect(readComposerFile(file)).rejects.toMatchObject({ name: "ComposerFileError", messageKey: key });
  });

  it("rejects an oversized file before reading its bytes", async () => {
    const file = new File([new Uint8Array(COMPOSER_FILE_MAX_BYTES + 1)], "large.pdf");
    const read = vi.spyOn(file, "arrayBuffer");
    await expect(readComposerFile(file)).rejects.toMatchObject({ messageKey: "fileTooLarge" });
    expect(read).not.toHaveBeenCalled();
  });

  it("rejects an already aborted upload without reading bytes", async () => {
    const file = new File(["notes"], "notes.txt");
    const read = vi.spyOn(file, "arrayBuffer");
    const signal = AbortSignal.abort();
    await expect(readComposerFile(file, signal)).rejects.toBe(signal.reason);
    expect(read).not.toHaveBeenCalled();
  });

  it("rejects promptly while the file buffer is still being read", async () => {
    const file = new File(["%PDF-1.7"], "notes.pdf");
    const read = vi.spyOn(file, "arrayBuffer").mockImplementation(() => new Promise(() => {}));
    const controller = new AbortController();
    const reading = readComposerFile(file, controller.signal);
    expect(read).toHaveBeenCalledOnce();
    controller.abort();
    await expect(reading).rejects.toBe(controller.signal.reason);
    expect(pdf.readComposerPdf).not.toHaveBeenCalled();
  });

  it("maps a browser file read failure to a localized error", async () => {
    const file = new File(["hello"], "notes.txt");
    vi.spyOn(file, "arrayBuffer").mockRejectedValue(new Error("file disappeared"));
    await expect(readComposerFile(file)).rejects.toMatchObject({ messageKey: "fileReadFailed" });
  });

  it.each([
    ["sample.PNG", "image/png", [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]],
    ["sample.jpg", "image/jpeg", [0xff, 0xd8, 0xff]],
    ["sample.gif", "image/gif", [0x47, 0x49, 0x46, 0x38, 0x39, 0x61]],
    ["sample.webp", "image/webp", [0x52, 0x49, 0x46, 0x46, 0, 0, 0, 0, 0x57, 0x45, 0x42, 0x50]],
  ] as const)("converts %s to the supported image wire format", async (name, media_type, signature) => {
    stubImage(20, 15);
    const file = new File([new Uint8Array(signature)], name, { type: "application/octet-stream" });
    await expect(readComposerFile(file)).resolves.toEqual({
      kind: "image", name, media_type, data: btoa(String.fromCharCode(...signature)), width: 20, height: 15,
    });
  });

  it("rejects images with the wrong signature, failed decoding, or excess dimensions", async () => {
    await expect(readComposerFile(new File(["<svg/>"], "bad.png"))).rejects.toMatchObject({ messageKey: "imageFailed" });
    const file = new File([new Uint8Array([0xff, 0xd8, 0xff])], "bad.jpg");
    stubImage(20, 15, true);
    await expect(readComposerFile(file)).rejects.toMatchObject({ messageKey: "imageFailed" });
    stubImage(MAX_IMAGE_DIMENSION + 1, 1);
    await expect(readComposerFile(file)).rejects.toMatchObject({ messageKey: "imageTooLarge" });
  });

  it("converts a PDF to a text attachment and preserves typed parser failures", async () => {
    const file = new File(["%PDF-1.7 test"], "article.pdf");
    pdf.readComposerPdf.mockResolvedValueOnce("Extracted text");
    const { signal } = new AbortController();
    await expect(readComposerFile(file, signal)).resolves.toEqual({ kind: "text", name: "article.pdf", content: "Extracted text" });
    expect(pdf.readComposerPdf).toHaveBeenCalledWith(new TextEncoder().encode("%PDF-1.7 test"), signal);
    pdf.readComposerPdf.mockRejectedValueOnce(new ComposerFileError("filePdfNoText"));
    await expect(readComposerFile(file)).rejects.toMatchObject({ messageKey: "filePdfNoText" });
    pdf.readComposerPdf.mockRejectedValueOnce(new Error("invalid PDF"));
    await expect(readComposerFile(file)).rejects.toMatchObject({ messageKey: "filePdfFailed" });
  });
});

function stubImage(width: number, height: number, fail = false) {
  vi.stubGlobal("Image", class {
    readonly naturalWidth = width;
    readonly naturalHeight = height;
    onload?: () => void;
    onerror?: () => void;
    set src(_url: string) { queueMicrotask(() => fail ? this.onerror?.() : this.onload?.()); }
  });
}
