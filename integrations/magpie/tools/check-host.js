// Yarn entrypoint for the Ruby E2E driver. Production plugins never import it.
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const result = spawnSync("ruby", [fileURLToPath(new URL("./check-host.rb", import.meta.url)), ...process.argv.slice(2)], {
  stdio: "inherit", env: process.env
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
