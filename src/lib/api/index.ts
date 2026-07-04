/**
 * The typed bridge to the Rust core: mirrors of the serde structs plus thin
 * wrappers over Tauri `invoke` commands. Split by domain into sibling modules
 * and re-exported here, so every `@/lib/api` import keeps resolving unchanged.
 */
export * from "./events";
export * from "./recorder";
export * from "./custom-games";
export * from "./clips";
export * from "./audio";
export * from "./settings";
export * from "./cloud";
export * from "./valorant";
export * from "./overlay";
