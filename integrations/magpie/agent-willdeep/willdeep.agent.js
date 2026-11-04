import { connection } from "./contract.js";
import { text } from "./messages.js";
import { icon } from "./icon.js";

export const agent = {
  id: "willdeep",
  name: "WillDeep",
  dir: "~/.willdeep/integrations/magpie/desktop",
  config: "~/.willdeep/integrations/magpie/desktop/connection.json",
  format: "json",
  model: "model",
  prefix: "",
  icon,
  notice: text("notice")
};

export function connect(input) {
  return connection(input, "willdeep", "magpie-willdeep", text);
}
