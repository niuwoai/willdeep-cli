import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { pdfCMapAssets } from "./vite.config.pdfCMapAssets.mjs";

export default defineConfig({
  plugins: [react(), pdfCMapAssets()],
  build: { rollupOptions: { input: { index: "index.html", share: "share.html" } } },
  server: { proxy: { "/api": "http://127.0.0.1:9847", "/health": "http://127.0.0.1:9847" } },
});
