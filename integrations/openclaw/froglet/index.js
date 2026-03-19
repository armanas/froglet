import { readPluginConfig } from "./lib/config.js"
import { registerRuntimeTools } from "./lib/runtime-tools.js"

export default function register(api) {
  const config = readPluginConfig(api)

  registerRuntimeTools(api, config)
  api.logger?.info?.("Loaded Froglet OpenClaw plugin with runtime-only tools")
}
