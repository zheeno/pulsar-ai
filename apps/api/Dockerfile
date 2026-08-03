FROM node:22-alpine AS base
WORKDIR /app

FROM base AS deps
COPY package.json package-lock.json turbo.json ./
COPY apps/api/package.json ./apps/api/
COPY packages/shared/package.json ./packages/shared/
RUN npm ci --workspace=@ngx/api --workspace=@ngx/shared --include-workspace-root

FROM base AS builder
COPY --from=deps /app/node_modules ./node_modules
COPY . .
RUN npm run build --workspace=@ngx/shared && npm run build --workspace=@ngx/api

FROM node:22-alpine AS runner
WORKDIR /app
ENV NODE_ENV=production
RUN apk add --no-cache curl
COPY --from=builder /app/apps/api/dist ./dist
COPY --from=builder /app/node_modules ./node_modules
COPY --from=builder /app/packages/shared/dist ./packages/shared/dist
COPY --from=builder /app/apps/api/package.json ./package.json
COPY --from=builder /app/scripts ./scripts
COPY --from=builder /app/supabase ./supabase
COPY deploy/api-entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
EXPOSE 3001
ENTRYPOINT ["/entrypoint.sh"]
