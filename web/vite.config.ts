import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Backend to proxy to in dev. `npm run dev:mock` points this at the mock server.
const backend = process.env.FORGE_BACKEND || "http://localhost:3000";

const proxyOpts = { target: backend, changeOrigin: false, xfwd: true };

export default defineConfig({
  plugins: [react()],
  build: { outDir: "dist", emptyOutDir: true, sourcemap: false, chunkSizeWarningLimit: 700 },
  server: {
    port: 5173,
    proxy: {
      "/api": proxyOpts,
      // git smart HTTP: /{owner}/{repo}(.git)?/info/refs | git-upload-pack | git-receive-pack
      "^/[^/]+/[^/]+?(\\.git)?/(info/refs|git-upload-pack|git-receive-pack)": proxyOpts,
    },
  },
});
