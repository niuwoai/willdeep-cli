import { expect, it, vi } from "vitest";
import { readEventStream } from "./readEventStream";

it("stops consuming SSE at the turn's terminal frame even if upstream stays open", async () => {
  const encoder = new TextEncoder();
  const cancel = vi.fn();
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(encoder.encode(': keepalive\r\n\r\ndata: {"type":"delta"}\r\n\r'));
      controller.enqueue(encoder.encode('\ndata: {"type":\r\ndata: "completed"}\r\n\r\ndata: invalid\n\n'));
      // A proxy need not close the HTTP connection after transmitting the terminal frame.
    },
    cancel,
  });
  const types: string[] = [];
  await readEventStream<{ type: string }>(new Response(stream), async ({ type }) => {
    types.push(type);
    return type === "completed";
  });
  expect(types).toEqual(["delta", "completed"]);
  expect(cancel).toHaveBeenCalledOnce();
  expect(stream.locked).toBe(false);
});
