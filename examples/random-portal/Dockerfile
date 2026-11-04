# Build from the repository root (the image installs Kern from this checkout):
#   docker build -f examples/random-portal/Dockerfile -t random-portal .
FROM node:22-alpine AS web
WORKDIR /web
COPY examples/random-portal/web/package.json examples/random-portal/web/package-lock.json ./
RUN npm ci
COPY examples/random-portal/web/ ./
RUN npm run build

FROM python:3.12-slim
RUN apt-get update && apt-get install -y --no-install-recommends nginx \
 && rm -rf /var/lib/apt/lists/* && rm -f /etc/nginx/sites-enabled/default
WORKDIR /kern
COPY pyproject.toml README.md ./
COPY kern/ kern/
RUN pip install --no-cache-dir .
WORKDIR /app
COPY examples/random-portal/portal.k examples/random-portal/portal.host examples/random-portal/portal.why ./
COPY --from=web /web/dist/ /srv/www/
# The proof pages are part of the app, rendered from verifier results when the image is built.
# `check` exits 1 unless every definition is clean, so an unproven module fails the build.
# proof.html has rationale and spec strength; host.html adds the endpoints and host assumptions
# (`kern.host page` doesn't read the .why file).
RUN python -m kern check portal.k --why portal.why --html /srv/www/proof.html --cache '' \
 && python -m kern.host page portal.k --html /srv/www/host.html
COPY examples/random-portal/nginx.conf /etc/nginx/conf.d/portal.conf
COPY examples/random-portal/entrypoint.sh /entrypoint.sh
VOLUME /data
EXPOSE 8000
CMD ["/entrypoint.sh"]
