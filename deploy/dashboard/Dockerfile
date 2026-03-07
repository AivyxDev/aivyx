# ─── Aivyx Ops Dashboard ──────────────────────────────────────────
# Lightweight Nginx container serving static SPA + API reverse proxy.

FROM nginx:1.27-alpine

# Copy static files
COPY src/ /usr/share/nginx/html/

# Copy nginx config as template (for envsubst)
COPY nginx.conf /etc/nginx/templates/default.conf.template

# Copy entrypoint
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Remove default nginx config
RUN rm -f /etc/nginx/conf.d/default.conf

EXPOSE 80

ENTRYPOINT ["/entrypoint.sh"]
