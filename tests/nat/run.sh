#!/usr/bin/env bash
# Two copies of the app, each behind its own NAT router (the kernel's iptables, in containers),
# trying to reach each other through a STUN server on a third network (ADR-115). Which kinds of
# NAT the two sit behind is what changes from case to case; the table at the end says which
# cases the peers heard each other in.
#
#   tests/nat/run.sh [case-name ...]       (all cases without names)
#   NAT_LAB_PEER=<path> tests/nat/run.sh   (a peer binary built from another version)
#   NAT_LAB_AUDIO_SECS=10 tests/nat/run.sh (also send audio once the route is settled, and
#                                           report the delay and loss it had)
#
# Needs docker and the ability to give a container NET_ADMIN.
#
# What the routers can and cannot be: the kernel gives a port per destination *address and port*
# only with --random. "sequential" is a port per destination *address* (a rule per address, in a
# row), so it is a NAT whose port depends on the address alone: it stands in for one that counts
# up as it meets new destinations, and says nothing about one that counts up per destination port.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
suffix=${NAT_LAB_SUFFIX:-$$}
wan_prefix=${NAT_LAB_WAN_PREFIX:-45.153.18.}
wan=jamuru-nat-wan-$suffix
lan_a=jamuru-nat-lan-a-$suffix
lan_b=jamuru-nat-lan-b-$suffix
prefix=jamuru-nat-$suffix
router_image=jamuru-nat-router:lab
client_image=jamuru-nat-client:lab
stun_ips="${wan_prefix}11 ${wan_prefix}12 ${wan_prefix}13"
stuns=${stun_ips// /,}
stun_arg=$(for ip in $stun_ips; do printf '%s:3478,' "$ip"; done)
stun_arg=${stun_arg%,}
wan_a=${wan_prefix}2
wan_b=${wan_prefix}3
base_port=${NAT_LAB_BASE_PORT:-30000}
settle=${NAT_LAB_SETTLE_SECS:-20}
audio=${NAT_LAB_AUDIO_SECS:-0}
work_root=$root/target/nat-lab
results=()
fail=0

cleanup() {
  docker rm -f "$prefix-stun" "$prefix-router-a" "$prefix-router-b" "$prefix-a" "$prefix-b" \
    >/dev/null 2>&1 || true
  docker network rm "$wan" "$lan_a" "$lan_b" >/dev/null 2>&1 || true
}
[ -n "${NAT_LAB_KEEP:-}" ] || trap cleanup EXIT

(cd "$root" && cargo build --examples >&2)
peer_bin=${NAT_LAB_PEER:-$root/target/debug/examples/nat_lab_peer}
stun_bin=$root/target/debug/examples/nat_lab_stun
docker build -q -t "$router_image" "$here" >/dev/null
docker build -q -t "$client_image" -f "$here/Dockerfile.client" "$here" >/dev/null

# start_router <a|b> <env...>
start_router() {
  local side=$1; shift
  local lan=$lan_a own=$wan_a other=$wan_b net=1
  if [ "$side" = b ]; then lan=$lan_b own=$wan_b other=$wan_a net=2; fi
  docker create --name "$prefix-router-$side" --cap-add NET_ADMIN \
    --sysctl net.ipv4.ip_forward=1 ${NAT_LAB_UDP_TIMEOUT:+--sysctl net.netfilter.nf_conntrack_udp_timeout=$NAT_LAB_UDP_TIMEOUT} ${NAT_LAB_UDP_TIMEOUT:+-e UDP_TIMEOUT=$NAT_LAB_UDP_TIMEOUT} ${NAT_LAB_CT_EVENTS:+-e CT_EVENTS=1} -e WAN_PREFIX="$wan_prefix" -e STUNS="$stuns" \
    -e PEER_WAN="$other" -e BASE="$base_port" "$@" \
    --network "$wan" --ip "$own" "$router_image" >/dev/null
  docker network connect --ip "10.78.$net.2" "$lan" "$prefix-router-$side"
  docker start "$prefix-router-$side" >/dev/null
}

start_client() { # start_client <a|b> <peer args...>; connect_at_a / connect_at_b: epoch ms to connect at
  local side=$1; shift
  local at_var=connect_at_$side connect_arg=""
  if [ -n "${!at_var:-}" ]; then connect_arg="--connect-at-ms ${!at_var}"; fi
  local lan=$lan_a net=1 other=b
  if [ "$side" = b ]; then lan=$lan_b net=2 other=a; fi
  docker run -d --name "$prefix-$side" --cap-add NET_ADMIN --network "$lan" --ip "10.78.$net.10" \
    -v "$peer_bin":/peer:ro -v "$work":/lab "$client_image" \
    sh -c "ip route replace default via 10.78.$net.2 && exec /peer --name $side --peer $other --dir /lab --stun $stun_arg --settle-secs $settle --audio-secs $audio $connect_arg ${NAT_LAB_PEER_ARGS:-} \"\$@\"" sh "$@" >/dev/null
}

# run_case <name> <a mode> <b mode> <gap> <expected: heard|silent> [skew ms] [records: a|b|ab]
# skew: how many ms after B's first packet A's goes out (negative: before); empty leaves the two to
# start when each has the other's offer. record=1: a router keeps the packets nobody had sent for
# (the app's first packet to that address is then given another port). Which routers: a, b or ab
run_case() {
  local name=$1 mode_a=$2 mode_b=$3 gap=$4 expected=$5 skew=${6:--} record=${7:--}
  cleanup
  work=$work_root/$suffix-$name
  rm -rf "$work"; mkdir -p "$work"
  docker network create --subnet "${wan_prefix}0/24" "$wan" >/dev/null
  docker network create --internal --subnet 10.78.1.0/24 "$lan_a" >/dev/null
  docker network create --internal --subnet 10.78.2.0/24 "$lan_b" >/dev/null
  docker create --name "$prefix-stun" --cap-add NET_ADMIN --network "$wan" --ip "${wan_prefix}11" \
    -v "$stun_bin":/stun:ro "$client_image" \
    sh -c "ip addr add ${wan_prefix}12/24 dev eth0; ip addr add ${wan_prefix}13/24 dev eth0; exec /stun $(for ip in $stun_ips; do printf '%s:3478 ' "$ip"; done)" >/dev/null
  docker start "$prefix-stun" >/dev/null
  start_router a -e MODE="$mode_a" -e GAP="$gap" -e PORTSET_MIDDLE=5 -e PORTSET_BLOCKS=3,10,17,24,31,38,45,52 -e SAMPLE=1 -e RECORD_UNSOLICITED="$([[ $record == *a* ]] && echo 1 || echo 0)"
  start_router b -e MODE="$mode_b" -e GAP="$gap" -e PORTSET_MIDDLE=5 -e PORTSET_BLOCKS=3,10,17,24,31,38,45,52 -e SAMPLE=1 -e RECORD_UNSOLICITED="$([[ $record == *b* ]] && echo 1 || echo 0)"
  sleep 1.5
  connect_at_a="" connect_at_b=""
  if [ "$skew" != - ]; then
    # far enough on for both to have gathered and traded offers
    local base_ms=$(( $(date +%s%3N) + 6000 ))
    connect_at_b=$base_ms; connect_at_a=$((base_ms + skew))
  fi
  start_client a
  start_client b
  for _ in $(seq $(( (60 + settle + audio) * 2 ))); do
    docker logs "$prefix-a" 2>&1 | grep -q '^DONE' && docker logs "$prefix-b" 2>&1 | grep -q '^DONE' && break
    sleep 0.5
  done
  local log_a log_b
  log_a=$(docker logs "$prefix-a" 2>&1); log_b=$(docker logs "$prefix-b" 2>&1)
  local recv_a recv_b route_a route_b ms_a ms_b pps_a pps_b
  recv_a=$(sed -n 's/^STATS sent=[0-9]* received=//p' <<<"$log_a")
  recv_b=$(sed -n 's/^STATS sent=[0-9]* received=//p' <<<"$log_b")
  route_a=$(grep '^ROUTE' <<<"$log_a" | tail -1 | awk '{print $2}')
  route_b=$(grep '^ROUTE' <<<"$log_b" | tail -1 | awk '{print $2}')
  ms_a=$(grep -o 'at_ms=[0-9]*' <<<"$log_a" | tail -1 | cut -d= -f2 || true)
  ms_b=$(grep -o 'at_ms=[0-9]*' <<<"$log_b" | tail -1 | cut -d= -f2 || true)
  pps_a=$(docker logs "$prefix-router-a" 2>&1 | sed -n 's/^OUT_PPS //p' | sort -n | tail -1)
  pps_b=$(docker logs "$prefix-router-b" 2>&1 | sed -n 's/^OUT_PPS //p' | sort -n | tail -1)
  local got=silent
  if [ "${recv_a:-0}" -gt 0 ] && [ "${recv_b:-0}" -gt 0 ]; then got=heard; fi
  local verdict=ok
  if [ "$got" != "$expected" ]; then verdict="UNEXPECTED(want $expected)"; fail=1; fi
  results+=("$(printf '%-22s A=%-10s B=%-10s gap=%-3s %-7s recv a=%s b=%s  route a=%s b=%s  moved-at a=%sms b=%sms  max-out-pps a=%s b=%s  %s' \
    "$name" "$mode_a" "$mode_b" "$gap" "$got" "${recv_a:-?}" "${recv_b:-?}" "${route_a:-?}" "${route_b:-?}" "${ms_a:--}" "${ms_b:--}" "${pps_a:-?}" "${pps_b:-?}" "$verdict")")
  if [ "$verdict" != ok ]; then
    echo "--- A:"; echo "$log_a"; echo "--- B:"; echo "$log_b"
  fi
  echo "${results[-1]}"
  grep -h "^GATHER_MS\|^AUDIO" <<<"$log_a" | sed "s/^/    a: /"
  grep -h "^GATHER_MS\|^AUDIO" <<<"$log_b" | sed "s/^/    b: /"
}

cases=(
  "both-cone            cone       cone       0  heard"
  "b-sequential         cone       sequential 0  heard"
  "b-sequential-gap6    cone       sequential 6  heard"
  "b-sequential-gap13   cone       sequential 13 heard"
  "b-sequential-gap25   cone       sequential 25 silent"
  "both-sequential      sequential sequential 0  heard"
  "b-random             cone       random     0  silent"
  "both-random          random     random     0  silent"
  "b-portset            cone       portset    0  heard"
  "record-b-b-first      cone       cone       0  heard  1000  b"
  "record-b-a-first      cone       cone       0  silent -1000 b"
  "record-b-same-time    cone       cone       0  silent 0     b"
  "record-a-a-first      cone       cone       0  heard  -1000 a"
  "record-a-b-first      cone       cone       0  silent 1000  a"
  "record-both-a-first   cone       cone       0  silent -1000 ab"
)
for line in "${cases[@]}"; do
  read -r name mode_a mode_b gap expected skew record <<<"$line"
  if [ "$#" -gt 0 ] && ! printf '%s\n' "$@" | grep -qx "$name"; then continue; fi
  run_case "$name" "$mode_a" "$mode_b" "$gap" "$expected" "${skew:--}" "${record:--}"
done

echo
echo "== results"
printf '%s\n' "${results[@]}"
exit $fail
