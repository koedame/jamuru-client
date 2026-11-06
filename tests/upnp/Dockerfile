# A home router that speaks UPnP IGD, NAT-PMP and PCP (miniupnpd, iptables backend), for
# tests/upnp/run.sh
FROM alpine:3.22
RUN apk add --no-cache miniupnpd iptables iptables-legacy iproute2
COPY router.sh /router.sh
ENTRYPOINT ["/bin/sh", "/router.sh"]
