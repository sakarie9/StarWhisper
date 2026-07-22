FROM debian:stable-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
ARG TARGETPLATFORM
COPY $TARGETPLATFORM/starwhisper /usr/bin/starwhisper
ENV STARWHISPER_CONFIG=/etc/starwhisper/config.toml
WORKDIR /data
ENTRYPOINT [ "/usr/bin/starwhisper" ]
