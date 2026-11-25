// Rust 命令的 payload 类型。字段名按 Rust serde 实际输出（snake_case），
// 以 ui/main.js 实际消费的字段为准，不确定的标 optional。
// 命令签名以 src-tauri/src/lib.rs generate_handler 为准，不凭记忆写。
//
// 分域文件在 `./types/`（AGENTS §12）；本文件只做再导出，保持 `@/lib/types` 稳定。

export * from "./types/core";
export * from "./types/dialogue";
export * from "./types/llm";
export * from "./types/memory";
export * from "./types/agent";
