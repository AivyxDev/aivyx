# ─── Stage 1: Build the Astro site ─────────────────────────────────
FROM node:22-alpine AS builder

WORKDIR /build

# Copy package files first for layer caching
COPY apps/website/package.json apps/website/package-lock.json ./
RUN npm ci

# Copy source and build
COPY apps/website/ ./
RUN npm run build

# ─── Stage 2: Serve with nginx ─────────────────────────────────────
FROM nginx:alpine

# Install curl for health checks
RUN apk add --no-cache curl

# Copy built static files
COPY --from=builder /build/dist /usr/share/nginx/html

# Custom nginx config for SPA-friendly routing
COPY deploy/website/nginx.conf /etc/nginx/conf.d/default.conf

EXPOSE 80
