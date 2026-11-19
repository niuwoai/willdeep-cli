import { afterEach, describe, expect, it, vi } from "vitest";
import { createRequestId } from "./requestId";

const UUID_V4_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const platformCrypto = globalThis.crypto;

afterEach(() => vi.unstubAllGlobals());

describe("createRequestId", () => {
  it("prefers native randomUUID and preserves its receiver", () => {
    const expected = "12345678-9abc-4def-8123-456789abcdef";
    const crypto = {
      randomUUID: vi.fn(function (this: unknown): string {
        expect(this).toBe(crypto);
        return expected;
      }),
      getRandomValues: vi.fn(),
    };
    vi.stubGlobal("crypto", crypto);

    expect(createRequestId()).toBe(expected);
    expect(crypto.randomUUID).toHaveBeenCalledOnce();
    expect(crypto.getRandomValues).not.toHaveBeenCalled();
  });

  it("works when randomUUID is absent using real cryptographic random bytes", () => {
    const crypto = {
      getRandomValues: vi.fn(function (this: unknown, bytes: Uint8Array): Uint8Array {
        expect(this).toBe(crypto);
        expect(bytes).toBeInstanceOf(Uint8Array);
        expect(bytes).toHaveLength(16);
        return platformCrypto.getRandomValues(bytes);
      }),
    };
    vi.stubGlobal("crypto", crypto);
    expect("randomUUID" in globalThis.crypto).toBe(false);

    expect(createRequestId()).toMatch(UUID_V4_PATTERN);
    expect(crypto.getRandomValues).toHaveBeenCalledOnce();
  });

  it.each([
    [0x00, "00000000-0000-4000-8000-000000000000"],
    [0xff, "ffffffff-ffff-4fff-bfff-ffffffffffff"],
  ])("sets v4 and RFC variant bits while preserving random bits for byte %i", (byte, expected) => {
    vi.stubGlobal("crypto", {
      getRandomValues: (bytes: Uint8Array): Uint8Array => bytes.fill(byte),
    });

    const id = createRequestId();
    expect(id).toBe(expected);
    expect(id).toMatch(UUID_V4_PATTERN);
  });

  it("draws fresh random bytes into independent buffers for each call", () => {
    let nextByte = 0;
    const buffers: Uint8Array[] = [];
    const getRandomValues = vi.fn((bytes: Uint8Array): Uint8Array => {
      buffers.push(bytes);
      return bytes.fill(nextByte++);
    });
    vi.stubGlobal("crypto", { getRandomValues });

    const first = createRequestId();
    const second = createRequestId();
    expect(first).toBe("00000000-0000-4000-8000-000000000000");
    expect(second).toBe("01010101-0101-4101-8101-010101010101");
    expect(second).not.toBe(first);
    expect(buffers[0]).not.toBe(buffers[1]);
    expect(getRandomValues).toHaveBeenCalledTimes(2);
  });

  it("propagates cryptographic random source failures", () => {
    const failure = new Error("Cryptographic random source unavailable");
    vi.stubGlobal("crypto", {
      getRandomValues: () => { throw failure; },
    });

    expect(() => createRequestId()).toThrow(failure);
  });
});
