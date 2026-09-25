import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// 构建产物放在 dist/，打包时嵌进 exe；窗口版和浏览器版（--run serve）用的是同一份页面
export default defineConfig({
  root: "src",
  plugins: [react()],
  base: "./",
  build: { outDir: "../dist", emptyOutDir: true },
  resolve: { alias: { "@": path.resolve(__dirname, "./src") } },
  clearScreen: false,
});
