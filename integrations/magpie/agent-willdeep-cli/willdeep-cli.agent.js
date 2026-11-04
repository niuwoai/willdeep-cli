import { connection } from "./contract.js";
import { text } from "./messages.js";
import { icon } from "./icon.js";

export const agent = {
  id: "willdeep-cli",
  name: "WillDeep CLI",
  bin: "willdeep",
  config: "~/.willdeep/integrations/magpie/cli/connection.json",
  format: "json",
  model: "model",
  prefix: "",
  icon,
  notice: text("notice")
};

export function connect(input) {
  return connection(input, "willdeep-cli", "magpie-willdeep-cli", text);
}
