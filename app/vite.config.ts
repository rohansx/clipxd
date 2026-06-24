import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri-ready: a fixed dev port, no clearing the screen so Rust logs stay visible.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 5174, strictPort: true },
});
