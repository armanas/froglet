import { readPluginConfig } from "./lib/config.js"
import { registerFrogletTool } from "./lib/froglet-tool.js"

export default function register(api) {
  const config = readPluginConfig(api)
  registerFrogletTool(api, config)
  api.logger?.info?.(
    `Loaded Froglet OpenClaw plugin with hostProduct=${config.hostProduct} baseUrl=${config.baseUrl}`
  )
}
