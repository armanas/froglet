import { readFile } from "node:fs/promises"

import { requestJson } from "./shared.js"

async function readControlToken(tokenPath, label) {
  const token = (await readFile(tokenPath, "utf8")).trim()
  if (token.length === 0) {
    throw new Error(`${label} auth token file ${tokenPath} is empty`)
  }
  return token
}

export async function controlRequest(url, tokenPath, timeoutMs, method, path, label, { jsonBody, expectedStatuses } = {}) {
  const token = await readControlToken(tokenPath, label)
  return requestJson(`${url}${path}`, {
    method,
    timeoutMs,
    headers: {
      Authorization: `Bearer ${token}`
    },
    jsonBody,
    expectedStatuses
  })
}
