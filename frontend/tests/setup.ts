// vitest 全局 setup。RTL 的 auto-cleanup 依赖 globals: true，vitest 会自动挂载
// afterEach；这里只补 jsdom 缺失的浏览器 API。
if (typeof window !== "undefined") {
  if (!window.matchMedia) {
    window.matchMedia = (() => ({
      matches: false,
      media: "",
      onchange: null,
      addListener: () => {},
      removeListener: () => {},
      addEventListener: () => {},
      removeEventListener: () => {},
      dispatchEvent: () => false,
    })) as unknown as typeof window.matchMedia;
  }
}
