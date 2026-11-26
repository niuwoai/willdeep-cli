function nextFrame(buffer: string): { frame: string; rest: string } | null {
  const boundary = /\r?\n\r?\n/.exec(buffer);
  if (!boundary) return null;
  return { frame: buffer.slice(0, boundary.index), rest: buffer.slice(boundary.index + boundary[0].length) };
}

/** Return as soon as the consumer observes the terminal event for its request. */
export async function readEventStream<Event>(response: Response, onEvent: (event: Event) => Promise<boolean>) {
  if (!response.ok || !response.body) throw new Error(await response.text());
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let ended = false;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) { ended = true; return; }
      buffer += decoder.decode(value, { stream: true });
      for (;;) {
        const next = nextFrame(buffer);
        if (!next) break;
        buffer = next.rest;
        const data = next.frame.split(/\r?\n/)
          .filter(line => line.startsWith("data:"))
          .map(line => line.slice(5).replace(/^ /, ""))
          .join("\n");
        if (data && await onEvent(JSON.parse(data) as Event)) return;
      }
    }
  } finally {
    // Some intermediaries leave the response open after transmitting a terminal
    // frame. Releasing the reader must not await the upstream socket's teardown.
    if (!ended) void reader.cancel().catch(() => console.debug("Event stream was already closed"));
    reader.releaseLock();
  }
}
