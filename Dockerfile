FROM debian:stable-slim
ARG TARGETPLATFORM
COPY $TARGETPLATFORM/starwhisper /usr/bin/starwhisper
ENV STARWHISPER_CONFIG=/etc/starwhisper/config.toml
WORKDIR /data
ENTRYPOINT [ "/usr/bin/starwhisper" ]
