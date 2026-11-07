#!/bin/sh
# Turns the container into a NAT router between a "wan" network (a public-looking block: the app
# does not take a private address for a public one) and a "lan" network (10.78.x.0/24).
#
# MODE is how the router gives a public port to what leaves the lan:
#   cone        MASQUERADE: the port the app sends from is kept (the kernel only changes it when
#               another flow already has it), whatever it sends to
#   random      MASQUERADE --random: each destination (address and port) gets a random port
#   sequential  a port of its own for each destination address, in a row from BASE: the STUN
#               servers (STUNS, in order) take BASE, BASE+1, ..., and PEER_WAN takes the next one
#               plus GAP ports that something else used meanwhile
#   portset     SNAT --random into 16 ports of each of several blocks of 1024: a carrier-grade
#               NAT that gives each customer a set of ports (MAP-E)
# What comes in from the wan is let through only for a flow that went out first (the kernel's
# connection tracking: the packet has to come from the address and port the app sent to).
set -eu

WAN_PREFIX=${WAN_PREFIX:?}
MODE=${MODE:-cone}
WAN_IF=$(ip -o -4 addr show | awk -v p="$WAN_PREFIX" 'index($4, p) == 1 {print $2}')
LAN_IF=$(ip -o -4 addr show | awk '$4 ~ /^10\.78\./ {print $2}')
WAN_IP=$(ip -o -4 addr show dev "$WAN_IF" | awk '{print $4}' | cut -d/ -f1)

iptables-legacy -t filter -A FORWARD -i "$LAN_IF" -o "$WAN_IF" -j ACCEPT
iptables-legacy -t filter -A FORWARD -i "$WAN_IF" -o "$LAN_IF" -m state --state ESTABLISHED,RELATED -j ACCEPT
# A packet from an address and port the app has not sent to is dropped before the kernel keeps
# any record of it. Left alone, the kernel records it, and when the app then sends to that
# address and port its mapping cannot keep the port, because the reply it expects is the
# packet already recorded (the lab would break a peer that is not behind any odd NAT).
ipset create seen hash:ip,port timeout 120
iptables-legacy -t mangle -A POSTROUTING -o "$WAN_IF" -p udp -j SET --add-set seen dst,dst --exist
iptables-legacy -t raw -A PREROUTING -i "$WAN_IF" -p udp -m set ! --match-set seen src,src -j DROP
# Counts what the app sends out, for the send rate
iptables-legacy -t mangle -A POSTROUTING -o "$WAN_IF" -p udp -j RETURN

case "$MODE" in
  cone)
    iptables-legacy -t nat -A POSTROUTING -o "$WAN_IF" -j MASQUERADE
    ;;
  random)
    iptables-legacy -t nat -A POSTROUTING -o "$WAN_IF" -j MASQUERADE --random
    ;;
  portset)
    # 16 ports, with middle bits PORTSET_MIDDLE, out of each of the blocks of 1024 in
    # PORTSET_BLOCKS; each flow gets one of the blocks and a random port in it
    blocks=$(echo "${PORTSET_BLOCKS:?}" | tr ',' ' ')
    left=$(echo "$blocks" | wc -w)
    for block in $blocks; do
      from=$((block * 1024 + ${PORTSET_MIDDLE:?} * 16))
      if [ "$left" -gt 1 ]; then
        iptables-legacy -t nat -A POSTROUTING -o "$WAN_IF" -p udp -m statistic --mode random \
          --probability "$(awk -v n="$left" 'BEGIN {printf "%.5f", 1 / n}')" -j SNAT \
          --to-source "$WAN_IP:$from-$((from + 15))" --random
      else
        iptables-legacy -t nat -A POSTROUTING -o "$WAN_IF" -p udp -j SNAT \
          --to-source "$WAN_IP:$from-$((from + 15))" --random
      fi
      left=$((left - 1))
    done
    ;;
  sequential)
    n=0
    for s in $(echo "${STUNS:?}" | tr ',' ' '); do
      iptables-legacy -t nat -A POSTROUTING -o "$WAN_IF" -p udp -d "$s" -j SNAT \
        --to-source "$WAN_IP:$((${BASE:?} + n))"
      n=$((n + 1))
    done
    iptables-legacy -t nat -A POSTROUTING -o "$WAN_IF" -p udp -d "${PEER_WAN:?}" -j SNAT \
      --to-source "$WAN_IP:$((BASE + n + ${GAP:-0}))"
    iptables-legacy -t nat -A POSTROUTING -o "$WAN_IF" -j MASQUERADE
    ;;
  *)
    echo "unknown MODE $MODE" >&2
    exit 1
    ;;
esac

echo "router up: mode=$MODE wan=$WAN_IF ($WAN_IP) lan=$LAN_IF"
# The packets that went out toward the wan, once a second, so the log shows the rate
if [ "${SAMPLE:-0}" = 1 ]; then
  prev=0
  while sleep 1; do
    now=$(iptables-legacy -t mangle -L POSTROUTING -n -v -x | awk '/udp/ {print $1; exit}')
    echo "OUT_PPS $((now - prev))"
    prev=$now
  done
else
  exec sleep infinity
fi
