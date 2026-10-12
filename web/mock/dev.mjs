// `npm run dev:mock`: start the in-memory mock backend and the Vite dev server proxying to it.
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const mockPort = process.env.MOCK_PORT || "3001";
const vitePort = process.env.PORT || "5173";
const env = { ...process.env, MOCK_PORT: mockPort, FORGE_BASE_URL: `http://localhost:${vitePort}`, FORGE_BACKEND: `http://localhost:${mockPort}` };

const children = [
  spawn(process.execPath, [path.join(root, "mock/server.mjs")], { stdio: "inherit", env }),
  spawn(process.execPath, [path.join(root, "node_modules/vite/bin/vite.js"), "--port", vitePort, "--strictPort", ...process.argv.slice(2)], { stdio: "inherit", env, cwd: root }),
];
const stop = () => children.forEach((c) => c.kill());
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
children.forEach((c) => c.on("exit", (code) => { stop(); process.exitCode = code ?? 0; }));
