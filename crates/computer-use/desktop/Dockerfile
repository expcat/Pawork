FROM alpine:3.22

ARG ALPINE_MIRROR=
RUN if [ -n "$ALPINE_MIRROR" ]; then \
      sed -i "s#https://dl-cdn.alpinelinux.org/alpine#${ALPINE_MIRROR}#g" /etc/apk/repositories; \
    fi \
    && apk add --no-cache \
        tigervnc \
        fluxbox \
        mousepad \
        firefox \
        font-dejavu \
        font-noto-cjk \
        dbus-x11 \
    && addgroup -S -g 1000 desktop \
    && adduser -S -u 1000 -G desktop -h /home/desktop -s /bin/sh desktop

COPY start.sh /usr/local/bin/start.sh
RUN chmod 755 /usr/local/bin/start.sh

USER 1000:1000
ENV HOME=/home/desktop \
    USER=desktop \
    DISPLAY=:0 \
    LANG=C.UTF-8
WORKDIR /home/desktop
EXPOSE 5900
CMD ["/usr/local/bin/start.sh"]
