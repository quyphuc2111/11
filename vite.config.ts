import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { readFileSync, existsSync } from "node:fs";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;
// @ts-expect-error process is a nodejs global
const useHttps = process.env.VITE_HTTPS === "true";

// Load custom SSL cert if exists and HTTPS mode requested
const hasCustomCert = existsSync("./cert.pem") && existsSync("./key.pem");
const enableHttps = useHttps && hasCustomCert;

console.log("HTTPS mode:", enableHttps ? "enabled" : "disabled");

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],

  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host || "0.0.0.0",
    https: enableHttps
      ? {
          key: readFileSync("./key.pem"),
          cert: readFileSync("./cert.pem"),
        }
      : undefined,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
  },
}));
