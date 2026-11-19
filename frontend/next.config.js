/** @type {import('next').NextConfig} */
const nextConfig = {
  output: "export", // Tauri WebView 直接加载 out/，无 node 服务
  reactStrictMode: false, // 流式气泡/事件双订阅
  images: { unoptimized: true },
  webpack: (config, { isServer, dev }) => {
    if (!isServer) {
      // dev 按需编译可能超过默认 120s，触发 ChunkLoadError（meetily 实测）
      config.output.chunkLoadTimeout = 600000;
    }
    if (dev) {
      config.watchOptions = {
        ...(config.watchOptions || {}),
        // macOS maxfiles 256：不排除 target/ 会 EMFILE，路由 404
        ignored: [
          "**/node_modules/**",
          "**/.git/**",
          "**/.next/**",
          "**/out/**",
          "**/target/**",
          "**/dist/**",
        ],
      };
    }
    return config;
  },
};

module.exports = nextConfig;
