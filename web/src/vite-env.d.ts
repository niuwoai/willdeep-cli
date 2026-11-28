/// <reference types="vite/client" />

interface Window {
  /// 桌面壳在页面加载前注入。存在且为 true 时，当前页面跑在桌面版里。
  __willdeepDesktop?: boolean;
}
