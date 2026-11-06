#!/bin/sh
# Turns the container into a router between a "wan" network (a public-looking block: miniupnpd
# and the app both refuse to map on a private outside address) and a "lan" network
# (10.77.0.0/24), then, unless UPNP=off, runs miniupnpd on it.
set -eu

WAN_PREFIX=${WAN_PREFIX:?}
WAN_IF=$(ip -o -4 addr show | awk -v p="$WAN_PREFIX" 'index($4, p) == 1 {print $2}')
LAN_IF=$(ip -o -4 addr show | awk '$4 ~ /^10\.77\.0\./ {print $2}')
WAN_IP=$(ip -o -4 addr show dev "$WAN_IF" | awk '{print $4}' | cut -d/ -f1)

iptables-legacy -t nat -N MINIUPNPD
iptables-legacy -t nat -A PREROUTING -i "$WAN_IF" -j MINIUPNPD
iptables-legacy -t nat -N MINIUPNPD-POSTROUTING
iptables-legacy -t nat -A POSTROUTING -o "$WAN_IF" -j MINIUPNPD-POSTROUTING
iptables-legacy -t nat -A POSTROUTING -o "$WAN_IF" -j MASQUERADE
iptables-legacy -t mangle -N MINIUPNPD
iptables-legacy -t mangle -A PREROUTING -i "$WAN_IF" -j MINIUPNPD
iptables-legacy -t filter -N MINIUPNPD
iptables-legacy -t filter -A FORWARD -i "$WAN_IF" ! -o "$WAN_IF" -j MINIUPNPD
iptables-legacy -t filter -A FORWARD -i "$LAN_IF" -o "$WAN_IF" -j ACCEPT
iptables-legacy -t filter -A FORWARD -i "$WAN_IF" -o "$LAN_IF" -m state --state ESTABLISHED,RELATED -j ACCEPT

cat > /tmp/miniupnpd.conf <<CONF
ext_ifname=$WAN_IF
ext_ip=$WAN_IP
listening_ip=$LAN_IF
port=5000
enable_natpmp=yes
enable_upnp=yes
secure_mode=yes
system_uptime=yes
lease_file=/tmp/upnpd.leases
allow 1024-65535 10.77.0.0/24 1024-65535
deny 0-65535 0.0.0.0/0 0-65535
CONF

echo "router up: wan=$WAN_IF ($WAN_IP) lan=$LAN_IF"
if [ "${UPNP:-on}" = "off" ]; then
  exec sleep infinity
fi
exec miniupnpd -d -f /tmp/miniupnpd.conf
