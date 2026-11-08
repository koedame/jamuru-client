# A home router for tests/nat/run.sh: forwards between a "wan" and a "lan" network and does the
# address translation with the kernel's iptables, in the way MODE says
FROM alpine:3.22
RUN apk add --no-cache iptables iptables-legacy iproute2 ipset conntrack-tools
COPY router.sh /router.sh
ENTRYPOINT ["/bin/sh", "/router.sh"]
