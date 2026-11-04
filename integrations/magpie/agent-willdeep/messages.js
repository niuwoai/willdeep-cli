// Magpie exposes a single notice/error string, with no locale in connect input.
export const messages = {
  zh: {
    notice: "预计最低版本：WillDeep Desktop 1.427.0-rc1；请先启动一次支持版本。连接和模型变更仅对未显式选择的新会话生效，已有会话保留原 Provider 和模型。",
    gateway: "无法连接：gateway.v1 必须是本机回环 HTTP(S) 地址，路径须为 /v1。",
    marker: "无法连接：gateway.key 必须精确为 magpie-willdeep；首版不支持真实网关密钥或共享网关。",
    model: "无法连接：model.id 必须是非空完整模型 ID，最多 1024 UTF-8 字节，且不含控制字符。"
  },
  en: {
    notice: "Expected minimum: WillDeep Desktop 1.427.0-rc1; launch a supported version once first. Connections and model changes affect new sessions without an explicit choice. Existing sessions keep their Provider and model.",
    gateway: "Cannot connect: gateway.v1 must be a loopback HTTP(S) URL with the exact /v1 path.",
    marker: "Cannot connect: gateway.key must be exactly magpie-willdeep. Real gateway keys and shared gateways are not supported in this version.",
    model: "Cannot connect: model.id must be a nonblank complete model ID, at most 1024 UTF-8 bytes, without control characters."
  }
};

export function text(key) {
  return messages.zh[key] + "\n" + messages.en[key];
}
