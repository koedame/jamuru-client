#!/usr/bin/env bash
# The app against a router that really forwards packets (miniupnpd in a container), on two
# docker networks: the app is on the "lan", a stand-in for a peer on the internet is on the
# "wan". Needs docker and the ability to give a container NET_ADMIN.
#
#   tests/upnp/run.sh
#
# Checks, in order:
#   1. with UPnP on at the router, the app gets a mapping whose public address is the router's;
#      a datagram sent to that address from the "internet" reaches the audio socket; closing the
#      mapping removes the router's forwarding rule
#   2. with UPnP off at the router, the app gets no mapping, still offers its other candidates,
#      and gathering them is no slower than the search timeout
#   3. with the setting off, the router is not asked at all
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
suffix=${UPNP_LAB_SUFFIX:-$$}
# A block that looks public (miniupnpd refuses private outside addresses); it exists only on the
# docker host's bridge for the length of the run
wan_prefix=${UPNP_LAB_WAN_PREFIX:-45.153.18.}
wan=jamuru-upnp-wan-$suffix
lan=jamuru-upnp-lan-$suffix
router=jamuru-upnp-router-$suffix
client_image=jamuru-upnp-client:lab
image=jamuru-upnp-router:lab

fail=0
check() { # check <description> <command...>
  local what=$1; shift
  if "$@"; then
    echo "PASS: $what"
  else
    echo "FAIL: $what"
    fail=1
    echo "--- router log"; docker logs --tail 25 "$router" 2>&1 || true
    echo "--- app log"; docker logs --tail 25 "$router-client" 2>&1 || true
    echo "---"
  fi
}

cleanup() {
  docker rm -f "$router" "$router-client" >/dev/null 2>&1 || true
  docker network rm "$wan" "$lan" >/dev/null 2>&1 || true
}
trap cleanup EXIT

(cd "$root" && cargo build --example port_mapping_probe >&2)
probe=$root/target/debug/examples/port_mapping_probe
docker build -q -t "$image" "$here" >/dev/null
docker build -q -t "$client_image" -f "$here/Dockerfile.client" "$here" >/dev/null

start_lab() { # start_lab <on|off>
  cleanup
  docker network create --subnet ${wan_prefix}0/24 "$wan" >/dev/null
  docker network create --internal --subnet 10.77.0.0/24 "$lan" >/dev/null
  docker create --name "$router" --cap-add NET_ADMIN --sysctl net.ipv4.ip_forward=1 \
    -e UPNP="$1" -e WAN_PREFIX="$wan_prefix" --network "$wan" --ip ${wan_prefix}2 "$image" >/dev/null
  docker network connect --ip 10.77.0.2 "$lan" "$router"
  docker start "$router" >/dev/null
  for _ in $(seq 50); do
    docker logs "$router" 2>&1 | grep -q "router up" && break
    sleep 0.2
  done
  sleep 1
}

run_client() { # run_client <probe args...>   (detached; log: docker logs $router-client)
  docker rm -f "$router-client" >/dev/null 2>&1 || true
  docker run -d --name "$router-client" --cap-add NET_ADMIN --network "$lan" --ip 10.77.0.10 \
    -v "$probe":/probe:ro "$client_image" \
    sh -c 'ip route replace default via 10.77.0.2 && exec /probe "$@"' sh "$@" >/dev/null
}

log() { docker logs "$router-client" 2>&1; }
wait_for() { # wait_for <pattern> <seconds>
  for _ in $(seq $(($2 * 5))); do log | grep -q "$1" && return 0; sleep 0.2; done
  return 1
}

echo "== 1. the router maps the port"
start_lab on
run_client --wait-secs 30
wait_for READY 30 || { log; echo "FAIL: the probe never got ready"; exit 1; }
mapped=$(log | sed -n 's/^MAPPED //p')
check "the app got a mapping on the router's public address" test "${mapped%:*}" = ${wan_prefix}2
check "the router has a forwarding rule for it" \
  bash -c "docker exec $router iptables-legacy -t nat -S MINIUPNPD | grep -q -- '--dport ${mapped#*:} -j DNAT'"
docker run --rm --network "$wan" --ip ${wan_prefix}50 alpine:3.22 \
  sh -c "echo hello-from-the-internet | nc -u -w1 ${mapped%:*} ${mapped#*:}"
check "a datagram from the internet reaches the audio socket" wait_for "RECEIVED hello-from-the-internet" 10
check "the mapping is closed" wait_for CLOSED 10
check "the router's forwarding rule is gone" \
  bash -c "! docker exec $router iptables-legacy -t nat -S MINIUPNPD | grep -q DNAT"

echo "== 2. the router does not speak UPnP"
start_lab off
run_client --wait-secs 1
wait_for NOT_MAPPED 30 && wait_for NOTHING_RECEIVED 10 || true
check "the app has no mapping" bash -c "docker logs $router-client 2>&1 | grep -q '^NOT_MAPPED'"
check "the app still offers its own addresses" \
  bash -c "docker logs $router-client 2>&1 | grep -q '^CANDIDATE host 10.77.0.10:'"
ms=$(log | sed -n 's/^GATHER_MS //p')
check "gathering took no longer than the router search plus STUN (${ms} ms)" test "${ms:-99999}" -lt 8000

echo "== 3. the setting is off"
start_lab on
run_client --no-router --wait-secs 1
wait_for NOTHING_RECEIVED 30 || true
check "the app has no mapping" bash -c "docker logs $router-client 2>&1 | grep -q '^NOT_MAPPED'"
check "the router has no forwarding rule" \
  bash -c "! docker exec $router iptables-legacy -t nat -S MINIUPNPD | grep -q DNAT"

exit $fail
