// Kept byte-identical in both independently installable packages.
export const limits = {
  modelBytes: 1024,
  contextMin: 4000,
  tokenMax: 1048576,
  outputMin: 1,
  portMax: 65535
};

function record(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function loopbackURL(value) {
  if (typeof value !== "string") return false;
  const match = /^https?:\/\/(localhost|127(?:\.(?:0|[1-9][0-9]{0,2})){3}|\[::1\])(?::([0-9]{1,5}))?\/v1$/.exec(value);
  // JS's $ also matches before a final newline; the whole string must match.
  if (!match || match[0] !== value) return false;
  if (match[1].slice(0, 4) === "127.") {
    const octets = match[1].split(".");
    for (let i = 1; i < octets.length; i++) {
      if (Number(octets[i]) > 255) return false;
    }
  }
  return match[2] === undefined || (Number(match[2]) >= 1 && Number(match[2]) <= limits.portMax);
}

function validModel(value) {
  if (typeof value !== "string" || value.trim() === "" || /[\u0000-\u001f\u007f-\u009f]/.test(value)) return false;
  let bytes = 0;
  for (let i = 0; i < value.length; i++) {
    const c = value.charCodeAt(i);
    if (c <= 0x7f) bytes++;
    else if (c <= 0x7ff) bytes += 2;
    else if (c >= 0xd800 && c <= 0xdbff) {
      const next = value.charCodeAt(++i);
      if (!(next >= 0xdc00 && next <= 0xdfff)) return false;
      bytes += 4;
    } else if (c >= 0xdc00 && c <= 0xdfff) return false;
    else bytes += 3;
    if (bytes > limits.modelBytes) return false;
  }
  return true;
}

function tokens(value, minimum) {
  return typeof value === "number" && Number.isInteger(value) && value >= minimum && value <= limits.tokenMax ? value : null;
}

export function connection(input, clientID, marker, text) {
  const gateway = record(input) && record(input.gateway) ? input.gateway : {};
  if (!loopbackURL(gateway.v1)) throw new Error(text("gateway"));
  // Never stringify, log, return, or interpolate an untrusted key.
  if (gateway.key !== marker) throw new Error(text("marker"));
  const model = record(input) && record(input.model) ? input.model : {};
  if (!validModel(model.id)) throw new Error(text("model"));
  const context = tokens(model.context, limits.contextMin);
  let output = tokens(model.output, limits.outputMin);
  if (context !== null && output !== null && output > context) output = null;
  // Magpie writes model.id through agent.model, preserving every prefix.
  return {
    schema_version: 1,
    client_id: clientID,
    enabled: true,
    base_url: gateway.v1,
    client_marker: marker,
    context_window: context,
    max_output_tokens: output
  };
}
