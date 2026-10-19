# Build context must be a Linux app package, never the source checkout or operator directory.
# Pin the runtime base by digest; see DOCKER.md for OS package reproducibility limits.
FROM node:26-trixie-slim@sha256:0d1e33586362bb9e608c4e751e7a9e7edb55c9ac11461ed5666af696cc461dd4
USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates python3 bubblewrap \
    && mkdir -p /usr/share/aip-runtime \
    && dpkg-query -W > /usr/share/aip-runtime/packages.txt \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/aip
COPY --chown=10001:10001 . /opt/aip/
USER 10001:10001
RUN node tools/app-deploy.mjs check
STOPSIGNAL SIGTERM
HEALTHCHECK --interval=10s --timeout=5s --start-period=15s --retries=3 \
    CMD node tools/app-deploy.mjs ready --config /run/aip/service.json
ENTRYPOINT ["node", "/opt/aip/tools/app-deploy.mjs"]
CMD ["serve", "--config", "/run/aip/service.json"]
