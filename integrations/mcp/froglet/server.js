#!/usr/bin/env node

import { readFile } from "node:fs/promises"
import { Server } from "@modelcontextprotocol/sdk/server/index.js"
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js"
import {
  CallToolRequestSchema,
  ListToolsRequestSchema
} from "@modelcontextprotocol/sdk/types.js"

import { readConfig } from "./lib/config.js"
import { buildToolDefinitions, handleToolCall } from "./lib/tools.js"

const packageMetadata = JSON.parse(
  await readFile(new URL("./package.json", import.meta.url), "utf8")
)
const config = readConfig()
const toolDefinitions = buildToolDefinitions(config)

const server = new Server(
  { name: "froglet", version: packageMetadata.version },
  { capabilities: { tools: {} } }
)

server.setRequestHandler(ListToolsRequestSchema, async () => ({
  tools: toolDefinitions
}))

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params
  return handleToolCall(name, args, config)
})

const transport = new StdioServerTransport()
await server.connect(transport)
