const UUID_BYTE_LENGTH = 16;
const UUID_VERSION_BYTE = 6;
const UUID_VARIANT_BYTE = 8;
const UUID_VERSION_MASK = 0x0f;
const UUID_VERSION_4 = 0x40;
const UUID_VARIANT_MASK = 0x3f;
const UUID_RFC_VARIANT = 0x80;

export function createRequestId(): string {
  const crypto = globalThis.crypto;
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();

  const bytes = crypto.getRandomValues(new Uint8Array(UUID_BYTE_LENGTH));
  bytes[UUID_VERSION_BYTE] = (bytes[UUID_VERSION_BYTE] & UUID_VERSION_MASK) | UUID_VERSION_4;
  bytes[UUID_VARIANT_BYTE] = (bytes[UUID_VARIANT_BYTE] & UUID_VARIANT_MASK) | UUID_RFC_VARIANT;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
