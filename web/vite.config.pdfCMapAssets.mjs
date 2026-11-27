import { readdir, readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

/** @returns {import("vite").Plugin} */
export function pdfCMapAssets() {
  return {
    name: "pdf-cmap-assets",
    apply: "build",
    async generateBundle() {
      const require = createRequire(import.meta.url);
      const directory = dirname(require.resolve("pdfjs-dist/cmaps/LICENSE"));
      const names = await readdir(directory);
      const assets = names.filter((name) => name === "LICENSE" || name.endsWith(".bcmap"));
      await Promise.all(assets.map(async (name) => {
        this.emitFile({
          type: "asset",
          fileName: `assets/pdf-cmaps/${name}`,
          source: await readFile(join(directory, name)),
        });
      }));
    },
  };
}
