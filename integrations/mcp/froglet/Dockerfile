# Froglet MCP Server Docker Image
#
# Build from repo root:
#   docker build -f integrations/mcp/froglet/Dockerfile -t froglet-mcp .
#
# Run:
#   docker run --rm -i \
#     -e FROGLET_PROVIDER_URL=http://host.docker.internal:8080 \
#     -e FROGLET_RUNTIME_URL=http://host.docker.internal:8081 \
#     froglet-mcp

FROM node:22-slim AS build
WORKDIR /build

# Copy shared library (dependency of MCP server)
COPY integrations/shared/ ./shared/

# Install production dependencies only
COPY integrations/mcp/froglet/package.json integrations/mcp/froglet/package-lock.json ./mcp/
RUN cd mcp && npm ci --production --ignore-scripts

# Copy MCP server source
COPY integrations/mcp/froglet/server.js ./mcp/
COPY integrations/mcp/froglet/lib/ ./mcp/lib/

FROM node:22-slim
LABEL org.opencontainers.image.title="froglet-mcp"
LABEL org.opencontainers.image.description="MCP server for Froglet services, compute, and marketplace"
LABEL org.opencontainers.image.url="https://github.com/armanas/froglet"

RUN groupadd --gid 10001 froglet && \
    useradd --uid 10001 --gid 10001 --no-create-home froglet

WORKDIR /
# Runtime imports resolve ../../../shared/... from /app/mcp/lib/*.js, which lands at /shared.
COPY --from=build --chown=froglet:froglet /build/shared/ ./shared/
WORKDIR /app
COPY --from=build --chown=froglet:froglet /build/mcp/ ./mcp/

USER froglet
WORKDIR /app/mcp

ENTRYPOINT ["node", "server.js"]
