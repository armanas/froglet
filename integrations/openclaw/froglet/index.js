import { readPluginConfig } from "./lib/config.js"
import { registerPublicTools } from "./lib/public-tools.js"
import { registerRuntimeTools } from "./lib/runtime-tools.js"

export default function register(api) {
  const config = readPluginConfig(api)

  registerPublicTools(api, config)
  registerRuntimeTools(api, config, {
    spawnImpl: api?.spawnCommand
  })

  if (config.enablePrivilegedRuntimeTools) {
    api.logger?.info?.("Loaded Froglet OpenClaw plugin with privileged runtime tools enabled")
    return
  }

  api.logger?.info?.("Loaded Froglet OpenClaw plugin")
}
