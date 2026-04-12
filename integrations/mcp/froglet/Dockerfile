# Build from repo root: docker build -f integrations/mcp/froglet/Dockerfile -t froglet-mcp .
FROM node:22-slim
WORKDIR /app

# Copy shared library first (dependency of MCP server)
COPY integrations/shared/ /app/shared/

# Copy MCP server
COPY integrations/mcp/froglet/package.json integrations/mcp/froglet/package-lock.json ./mcp/
RUN cd mcp && npm ci --production

COPY integrations/mcp/froglet/server.js ./mcp/
COPY integrations/mcp/froglet/lib/ ./mcp/lib/

WORKDIR /app/mcp
ENTRYPOINT ["node", "server.js"]
