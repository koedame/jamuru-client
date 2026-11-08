#!/usr/bin/env python3
"""A many-peer room check for CI (ADR-117).

Every participant is a headless `jamuru` that sends a tone of its own and writes
what it hears to a file. The participants take turns: one is unmuted for a few
seconds while everyone else is muted, so that a listener hears a single tone at
a time (several tones added up are clipped at full scale, and the clipping
makes every other frequency look present). In each turn, every listener's file
is searched for the speaker's tone: the share of 100 ms windows in which it was
audible is how well that listener heard that speaker. The same files show how
often a tone is heard in somebody else's turn, which has to be never.

The participants of one room are spread over several runners. The runner that
hosts the room (`start-host`) publishes the invite code to the others through
an encrypted file (`fetch` reads it); each runner then starts its own
participants (`run`) and measures them. `summary` joins the results of all
runners into a pass or fail.

The runners are of different systems (Linux, Windows, macOS), so one room also
measures every listening system against every speaking system. Each participant
joins under the name `r<run>a<attempt>-p<NN>`: the number of the workflow run and
of the participant, which is what a record kept by the server is matched with
(an address says little - the participants of a runner share one, and one runner
may be seen under several).

Beside the verdict, each participant's result has what is needed to follow the
quality over versions and to explain it: the system and the version, how soon
each link came up, the kind of route of each link and the family of its
address, the round trip of the link and of the whole audio path with its
spread, the receive buffer, and what the runner is (its CPU count, the
datacenter as the cloud's own metadata tells it, whether IPv6 works, and how its
NAT maps ports). An address is never written: where two must be compared, a
keyed hash stands for it, and the key is of this run alone.

The output of this script is the only thing that reaches the log: pass or fail
and numbers per participant. What the server answers, what `jamuru` prints and
where the participants connected from are never printed.

Standard library only, so that CI installs nothing it does not pin.
"""

import argparse
import array
import hashlib
import hmac
import ipaddress
import json
import math
import os
import platform
import re
import secrets
import select
import socket
import statistics
import struct
import subprocess
import sys
import time
import urllib.request
import zipfile
from operator import mul
from pathlib import Path

# Participant i sends TONE_BASE_HZ + TONE_STEP_HZ * i. With a 100 ms window and
# a step of 100 Hz every tone fits a whole number of periods into a window, so
# the tones do not leak into each other's measurement.
TONE_BASE_HZ = 300
TONE_STEP_HZ = 100
WINDOW_SECONDS = 0.1
# The tones stay below 2 kHz, so adding up 8 samples (6 kHz at 48 kHz) loses
# nothing and makes the search 8 times cheaper.
DECIMATE = 8
# A speaker counts as heard in a window when its tone is at least this loud.
# The tone is sent at 0.5, and a window with a glitch in it measures less;
# the broadband click of a glitch puts a little into every other tone, up to
# about a tenth of the level, which is below this and so is not mistaken for a
# speaker.
PRESENT_AMPLITUDE = 0.2
# What runs after the last turn, so that nobody has left yet while the turns
# are read.
TAIL_SECONDS = 10
# Everyone is muted this long before the first turn.
QUIET_SECONDS = 4
# Not read at a turn's start and end: the tone fades in and out, and the clocks
# of the runners differ a little.
GUARD_SECONDS = 0.8
# How far the end of a file may be from the time the run was meant to end.
SEARCH_SECONDS = 3
# The least share of windows in which every speaker must be heard.
DEFAULT_MIN_HEARD = 0.9
ROOM_NAME = "room check"
INVITE_PREFIX = "Invite code:"
# The first turn starts this long after the last participant has entered the
# room, which is when the connections between them are made. A link that found nothing
# sends again only after a quiet of 100 seconds or two, so a run that is to show whether
# such links come up in the end sets it longer (`join_seconds` of the workflow).
JOIN_SECONDS = int(os.environ.get("JOIN_SECONDS") or 40)
# How long the host waits for every participant to enter the room. A runner
# can wait a long time for a machine (macOS ones are capped at 5 at a time).
JOINED_WAIT_SECONDS = 1200
# How long a runner waits for the others to say where their UDP port is, in the check
# of whether the runners reach each other directly, and how long they then send to each other.
UDP_WAIT_SECONDS = 600
UDP_PROBE_SECONDS = 25
# How many flows (socket pairs) each pair of runners is judged by.
UDP_FLOWS = 16
# The longest the check goes on sending after that, if nothing tells it to stop.
UDP_KEEP_SECONDS = 3000
# How long a participant waits for the host to say when the turns start.
START_WAIT_SECONDS = JOINED_WAIT_SECONDS + 300
JOINED_PATTERN = re.compile(r"📥 (\S+) joined the room")


def name_of(index):
    return f"p{index:02d}"


def layout_of(text):
    """Participants on each runner, from `2,2,1`."""
    return [int(part) for part in text.split(",")]


def runner_of(layout, index):
    """The runner participant `index` runs on: the participants are numbered runner by runner."""
    first = 0
    for runner, count in enumerate(layout):
        if index < first + count:
            return runner
        first += count
    raise ValueError(f"participant {index} is beyond the layout {layout}")


def room_name_of(index):
    """The name a participant enters the room under: the run, the attempt and the participant."""
    run = os.environ.get("GITHUB_RUN_ID", "local")
    attempt = os.environ.get("GITHUB_RUN_ATTEMPT", "1")
    return f"r{run}a{attempt}-{name_of(index)}"


def participant_of(room_name):
    """The number of the participant a name in the room belongs to, or None."""
    found = re.search(r"(?:^|-)p(\d+)$", room_name)
    return int(found.group(1)) if found else None


def tone_of(index):
    return TONE_BASE_HZ + TONE_STEP_HZ * index


def binary_of(path):
    """`jamuru`, or `jamuru.exe` where that is what the build made."""
    path = Path(path)
    exe = path.with_name(path.name + ".exe")
    return exe if not path.exists() and exe.exists() else path


def server_url(env_name):
    url = os.environ.get(env_name, "").strip()
    if not url:
        sys.exit(f"{env_name} is not set")
    return url


def start_file_of(work):
    """Where the time the turns start is: the host writes it, the others copy it."""
    return Path(work) / "start" / "start.json"


def schedule(start_at, total, slot):
    """When each participant speaks, and when the run ends.

    Returns `(turns, end_at)`; `turns[i]` is the `(from, until)` of participant i.
    """
    turns = [(start_at + i * slot, start_at + (i + 1) * slot) for i in range(total)]
    return turns, start_at + total * slot + TAIL_SECONDS


def launch(binary, work, index, command, server, room, total, slot):
    """Starts participant `index`; it speaks in its turn once the host has set the start.

    Each has its own HOME, so that no two share an identity or settings, and
    runs under `supervise`, which drives its mute switch, ends it the way
    Ctrl+C does (so it writes its report) and records what it cost.
    """
    home = work / "home" / name_of(index)
    home.mkdir(parents=True, exist_ok=True)
    (work / "out").mkdir(parents=True, exist_ok=True)
    out = work / "out" / name_of(index)
    jamuru = [
        str(binary),
        command,
        "--server",
        server,
        "--name",
        room_name_of(index),
        "--input-tone",
        str(tone_of(index)),
        "--output-file",
        str(out) + ".f32",
        "--report-json",
        str(out) + ".json",
    ]
    if command == "create-room":
        jamuru += ["--room-name", ROOM_NAME]
    else:
        jamuru += ["--room", room]
    args = [
        sys.executable, __file__, "supervise",
        "--out", str(out),
        "--start-file", str(start_file_of(work)),
        "--index", str(index),
        "--total", str(total),
        "--slot", str(slot),
        "--", *jamuru,
    ]
    # On Windows the settings are where the system says, not where HOME points,
    # so the directory is named: two participants must not share an identity.
    env = dict(
        os.environ,
        HOME=str(home),
        XDG_CONFIG_HOME=str(home / ".config"),
        JAMJAM_CONFIG_DIR=str(home / "config"),
    )
    log = open(str(out) + ".log", "wb")
    detach = (
        {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
        if os.name == "nt" else {"start_new_session": True}
    )
    process = subprocess.Popen(
        args,
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=subprocess.STDOUT,
        env=env,
        **detach,
    )
    return process, out


def supervise(args):
    """Runs a command through its turn and until the run ends, and writes what it cost.

    The turns start when the host writes `--start-file`, so the command may be
    started long before. The session reads its commands from standard input:
    `/mute` and `/unmute` switch what it sends, `/quit` ends it the way Ctrl+C
    does (so it writes its report) on every system.
    """
    started = time.time()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    child = subprocess.Popen(command, stdin=subprocess.PIPE)
    steps = None
    end_at = None
    quit_at = None
    usage = None
    while True:
        if hasattr(os, "wait4"):
            done, _, usage = os.wait4(child.pid, os.WNOHANG)
        else:
            done = child.poll() is not None
        if done:
            break
        now = time.time()
        if steps is None:
            try:
                start_at = json.loads(Path(args.start_file).read_text())["start_at"]
            except (OSError, ValueError, KeyError):
                start_at = None
            if start_at is not None:
                turns, end_at = schedule(start_at, args.total, args.slot)
                turn = turns[args.index]
                steps = [(start_at - QUIET_SECONDS, b"/mute\n"), (turn[0], b"/unmute\n"), (turn[1], b"/mute\n")]
            elif now >= started + START_WAIT_SECONDS:
                steps, end_at = [], now
        while steps and now >= steps[0][0]:
            try:
                child.stdin.write(steps.pop(0)[1])
                child.stdin.flush()
            except OSError:
                steps.clear()
        if quit_at is None and end_at is not None and now >= end_at:
            quit_at = now
            try:
                child.stdin.write(b"/quit\n")
                child.stdin.flush()
            except OSError:
                pass
        elif quit_at is not None and now >= quit_at + 30:
            child.kill()
        time.sleep(0.1)
    elapsed = time.time() - started
    if usage is not None:
        # Kilobytes, except on macOS where the system counts bytes.
        rss_kb = usage.ru_maxrss / 1024 if sys.platform == "darwin" else usage.ru_maxrss
        text = f"{usage.ru_utime} {usage.ru_stime} {elapsed} {int(rss_kb)}\n"
    else:
        # What the system does not tell about a child that has ended is left out.
        text = f"- - {elapsed} -\n"
    Path(args.out + ".time").write_text(text)


def encrypt(plain, destination, key):
    subprocess.run(
        ["openssl", "enc", "-aes-256-cbc", "-pbkdf2", "-md", "sha256", "-salt", "-pass", "env:ROOM_CHECK_KEY",
         "-out", str(destination)],
        input=plain.encode(),
        env=dict(os.environ, ROOM_CHECK_KEY=key),
        check=True,
    )


def decrypt(source, key):
    done = subprocess.run(
        ["openssl", "enc", "-d", "-aes-256-cbc", "-pbkdf2", "-md", "sha256", "-pass", "env:ROOM_CHECK_KEY",
         "-in", str(source)],
        env=dict(os.environ, ROOM_CHECK_KEY=key),
        capture_output=True,
        check=True,
    )
    return done.stdout.decode()


def start_host(args):
    """Creates the room as participant 0 and publishes how to join it.

    When the turns start is not known yet: the host says so once everyone has
    entered (`join` on runner 0).
    """
    server = server_url(args.server_env)
    work = Path(args.work)
    layout = layout_of(args.layout)
    total = sum(layout)
    process, out = launch(binary_of(args.binary), work, 0, "create-room", server, None, total, args.slot)
    code = None
    deadline = time.time() + 90
    while time.time() < deadline and code is None:
        if process.poll() is not None:
            break
        for line in Path(str(out) + ".log").read_text(errors="replace").splitlines():
            if line.startswith(INVITE_PREFIX):
                code = line[len(INVITE_PREFIX):].strip()
                break
        time.sleep(0.5)
    if not code:
        print("host: no invite code")
        sys.exit(1)
    (work / "host.pid").write_text(str(process.pid))
    room = {"code": code, "slot": args.slot, "total": total, "layout": layout}
    (work / "room.json").write_text(json.dumps(room))
    published = work / "rendezvous"
    published.mkdir(exist_ok=True)
    encrypt(json.dumps(room), published / "room.enc", server)
    print("host: room created")


def fetch_artifact(work, name, member, timeout):
    """Waits for the artifact `name` of this workflow run and extracts `member` from it into `work`.

    It looks once at least, so a `timeout` of 0 asks "is it there yet?".
    """
    repo, run_id = os.environ["GITHUB_REPOSITORY"], os.environ["GITHUB_RUN_ID"]
    work.mkdir(parents=True, exist_ok=True)
    deadline = time.time() + timeout
    while True:
        listed = subprocess.run(
            ["gh", "api", f"repos/{repo}/actions/runs/{run_id}/artifacts?name={name}",
             "--jq", ".artifacts[0].id // empty"],
            capture_output=True, text=True,
        )
        artifact = listed.stdout.strip()
        if listed.returncode == 0 and artifact:
            archive = work / f"{name}.zip"
            with open(archive, "wb") as handle:
                done = subprocess.run(
                    ["gh", "api", f"repos/{repo}/actions/artifacts/{artifact}/zip"],
                    stdout=handle,
                )
            if done.returncode == 0:
                with zipfile.ZipFile(archive) as bundle:
                    bundle.extract(member, work)
                return True
        if time.time() >= deadline:
            return False
        time.sleep(5)


def fetch(args):
    """Waits for the host's file in this workflow run and reads it."""
    server = server_url(args.server_env)
    work = Path(args.work)
    work.mkdir(parents=True, exist_ok=True)
    attempt = os.environ["GITHUB_RUN_ATTEMPT"]
    if not fetch_artifact(work, f"room-rendezvous-{attempt}", "room.enc", args.timeout):
        print("fetch: the host published nothing in time")
        sys.exit(1)
    (work / "room.json").write_text(decrypt(work / "room.enc", server))
    print("fetch: got the room")


def entered(work):
    """The names of the participants the host has seen enter the room."""
    log = Path(work) / "out" / (name_of(0) + ".log")
    return set(JOINED_PATTERN.findall(log.read_text(errors="replace"))) if log.exists() else set()


def wait_for_everyone(work, total):
    """Waits until every other participant has entered the room, then sets when the turns start.

    The start time follows the arrival of the last runner, so a runner that
    waited long for a machine is still in the room when the turns begin. If
    someone never comes, the turns start anyway after `JOINED_WAIT_SECONDS`;
    the verdict then fails for the missing ones.
    """
    began = time.time()
    while len(entered(work)) < total - 1 and time.time() < began + JOINED_WAIT_SECONDS:
        time.sleep(1)
    seen = len(entered(work)) + 1
    print(f"host: {seen} of {total} participants entered the room after {int(time.time() - began)} s")
    start_at = time.time() + JOIN_SECONDS
    target = start_file_of(work)
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps({"start_at": start_at}))


def pid_alive(pid):
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True


def own_participants(room, runner):
    first = sum(room["layout"][:runner])
    return list(range(first, first + room["layout"][runner]))


def join(args):
    """Starts this runner's participants. On runner 0 it then waits for everyone and sets the start.

    The participants wait for the start by themselves, so nothing is lost when
    a runner is late to get a machine.
    """
    server = server_url(args.server_env)
    work = Path(args.work)
    room = json.loads((work / "room.json").read_text())
    runner = probe_runner()
    print(
        f"runner {args.runner}: {runner['os']} {runner['cpus']} cpus, "
        f"ipv6 {'works' if runner['ipv6'].get('servers_answered') else 'does not answer'}, "
        f"nat {'per destination' if runner['nat'].get('port_changes') else 'same port for all'}"
    )
    try:
        runner["udp"] = json.loads((udp_folder(work) / "result.json").read_text())
    except (OSError, ValueError):
        runner["udp"] = {"checked": False}
    (work / "runner.json").write_text(json.dumps(runner))
    for index in own_participants(room, args.runner):
        if index == 0:
            continue
        launch(binary_of(args.binary), work, index, "join-room", server, room["code"], room["total"], room["slot"])
    if args.runner == 0:
        wait_for_everyone(work, room["total"])


def measure(args):
    """Waits for this runner's participants to finish their run, and measures what they heard."""
    work = Path(args.work)
    room = json.loads((work / "room.json").read_text())
    runner = json.loads((work / "runner.json").read_text())
    target = start_file_of(work)
    if args.runner != 0:
        attempt = os.environ["GITHUB_RUN_ATTEMPT"]
        if not fetch_artifact(target.parent, f"room-start-{attempt}", "start.json", START_WAIT_SECONDS):
            print("measure: the host did not say when the turns start")
            sys.exit(1)
        start_at = json.loads(target.read_text())["start_at"]
        if time.time() > start_at - QUIET_SECONDS:
            target.unlink()
            print(f"measure: this runner arrived {int(time.time() - start_at)} s after the turns started")
            sys.exit(1)
    start_at = json.loads(target.read_text())["start_at"]
    _, end_at = schedule(start_at, room["total"], room["slot"])
    own = own_participants(room, args.runner)
    deadline = end_at + 120
    waiting = [work / "out" / (name_of(index) + ".time") for index in own]
    while time.time() < deadline and not all(path.exists() for path in waiting):
        time.sleep(1)
    host_pid = work / "host.pid"
    if host_pid.exists():
        pid = int(host_pid.read_text())
        while pid_alive(pid) and time.time() < end_at + 30:
            time.sleep(1)

    folder = udp_folder(work)
    if folder.exists():
        (folder / "stop").write_text("done")
        stopped = time.time() + 15
        while time.time() < stopped and not (folder / "final.json").exists():
            time.sleep(0.5)
        try:
            runner["udp"] = json.loads((folder / "final.json").read_text())
        except (OSError, ValueError):
            pass

    results = work / "results"
    results.mkdir(exist_ok=True)
    for index in own:
        result = analyze(work, index, room)
        result["runner_facts"] = runner
        (results / f"{name_of(index)}.json").write_text(json.dumps(result))
        print(describe(result))


def run(args):
    """`join` then `measure` in one step, for the runners other than the host."""
    join(args)
    measure(args)


STUN_SERVERS = (("stun.l.google.com", 19302), ("stun.cloudflare.com", 3478))
STUN_COOKIE = 0x2112A442
# Azure's own metadata about the machine it runs on; answers only from inside it.
CLOUD_METADATA = "http://169.254.169.254/metadata/instance/compute?api-version=2021-02-01&format=json"


def hashed(text):
    """Stands for `text` where two things are compared: keyed with this run's own key.

    The key is made of the server address (a secret) and the number of the run, so
    the same address has the same hash on every runner of the run - and in no
    other run, nor for anyone who does not have the secret.
    """
    server = os.environ.get("JAMJAM_SERVER_URL_BETA", "")
    key = f"{server}|{os.environ.get('GITHUB_RUN_ID', 'local')}".encode()
    return hmac.new(key, str(text).encode(), hashlib.sha256).hexdigest()[:12]


def stun_mapped(sock, family, host, port):
    """The address and port a STUN server sees `sock` from, or None."""
    try:
        target = socket.getaddrinfo(host, port, family, socket.SOCK_DGRAM)[0][4]
    except OSError:
        return None
    transaction = secrets.token_bytes(12)
    request = struct.pack("!HHI", 0x0001, 0, STUN_COOKIE) + transaction
    for _ in range(2):
        try:
            sock.sendto(request, target)
            data, _ = sock.recvfrom(2048)
        except OSError:
            continue
        if len(data) < 20 or data[8:20] != transaction:
            continue
        at = 20
        while at + 4 <= len(data):
            kind, size = struct.unpack("!HH", data[at:at + 4])
            value = data[at + 4:at + 4 + size]
            if kind == 0x0020 and size >= 8:
                port_seen = struct.unpack("!H", value[2:4])[0] ^ (STUN_COOKIE >> 16)
                if value[1] == 1:
                    raw = bytes(a ^ b for a, b in zip(value[4:8], struct.pack("!I", STUN_COOKIE)))
                elif value[1] == 2 and size >= 20:
                    mask = struct.pack("!I", STUN_COOKIE) + transaction
                    raw = bytes(a ^ b for a, b in zip(value[4:20], mask))
                else:
                    break
                return str(ipaddress.ip_address(raw)), port_seen
            at += 4 + size + (-size % 4)
    return None


def nat_probe(family):
    """What the NAT does to a socket's port, and the address it shows, of one IP family.

    Two servers are asked from one socket: a NAT that hands out the same port
    for any destination can be crossed by a peer that guesses nothing; one that
    changes it per destination (a symmetric NAT) cannot.
    """
    sock = socket.socket(family, socket.SOCK_DGRAM)
    sock.settimeout(1.5)
    try:
        sock.bind(("", 0))
        seen = [m for m in (stun_mapped(sock, family, h, p) for h, p in STUN_SERVERS) if m]
    except OSError:
        seen = []
    finally:
        sock.close()
    found = {"servers_answered": len(seen)}
    if seen:
        found["public_hash"] = hashed(seen[0][0])
        found["address_changes"] = len({m[0] for m in seen}) > 1
        found["port_changes"] = len({m[1] for m in seen}) > 1
    return found


def local_network_hash(family, probe_address):
    """A hash of the network the machine's own address is in (a /24, or a /64), or None.

    Connecting a UDP socket sends nothing: it only makes the system choose the
    address it would send from.
    """
    sock = socket.socket(family, socket.SOCK_DGRAM)
    try:
        sock.connect((probe_address, 9))
        own = sock.getsockname()[0].split("%")[0]
    except OSError:
        return None
    finally:
        sock.close()
    bits = 24 if family == socket.AF_INET else 64
    return hashed(ipaddress.ip_network(f"{own}/{bits}", strict=False).network_address)


def cloud_facts():
    """Where the cloud says the runner is, from its own metadata; empty if it says nothing."""
    request = urllib.request.Request(CLOUD_METADATA, headers={"Metadata": "true"})
    try:
        with urllib.request.urlopen(request, timeout=2) as answer:
            data = json.load(answer)
    except (OSError, ValueError):
        return {}
    found = {
        "region": data.get("location"),
        "zone": data.get("zone") or None,
        "vm_size": data.get("vmSize"),
    }
    if data.get("vmId"):
        found["machine_hash"] = hashed(data["vmId"])
    return {name: value for name, value in found.items() if value is not None}


def probe_runner():
    """What this runner is: system, size, where, and how it reaches the Internet."""
    found = {
        "os": os.environ.get("RUNNER_OS", platform.system()).lower(),
        "arch": os.environ.get("RUNNER_ARCH", platform.machine()).lower(),
        "image": os.environ.get("ImageVersion", ""),
        "cpus": os.cpu_count(),
        "cloud": cloud_facts(),
        "inner_network_hash": local_network_hash(socket.AF_INET, "192.0.2.1"),
        "nat": nat_probe(socket.AF_INET),
    }
    v6 = {"route": local_network_hash(socket.AF_INET6, "2001:db8::1") is not None}
    if v6["route"]:
        v6.update(nat_probe(socket.AF_INET6))
    found["ipv6"] = v6
    return found


def udp_folder(work):
    return Path(work) / "udp"


def udp_serve(args):
    """Checks whether this runner and each of the others reach each other by UDP, without the app.

    On the macOS runners, some UDP flows are lost on the way while others between the
    same two machines pass (11/8: a socket that heard nothing for the whole room next to
    the app's sockets that worked, and the other way round), so one flow says little about
    a pair. Each runner opens `UDP_FLOWS` sockets, and socket i of one runner talks to
    socket i of each other: a pair of runners is judged by how many of its flows came.

    The others' NATs let a packet in only from an address this one has sent to first, so
    every runner sends to every other at the same time: the addresses each socket shows to
    a STUN server are told through an artifact (encrypted, like the invite code), each waits
    until it has everyone's - the last to come starts all - and then they send to each other
    for a while, and go on at a lower rate until the measuring is over. Only how many
    packets came is kept, never an address.
    """
    server = server_url(args.server_env)
    folder = udp_folder(args.work)
    folder.mkdir(parents=True, exist_ok=True)
    room = json.loads((Path(args.work) / "room.json").read_text())
    runners = len(room["layout"])
    attempt = os.environ.get("GITHUB_RUN_ATTEMPT", "1")
    began = time.time()
    socks, mapped_to = [], []
    for _ in range(UDP_FLOWS):
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.settimeout(1.5)
        try:
            sock.bind(("", 0))
            found = None
            for host, port in STUN_SERVERS:
                found = stun_mapped(sock, socket.AF_INET, host, port)
                if found:
                    break
        except OSError:
            found = None
        if not found:
            sock.close()
            continue
        sock.setblocking(False)
        socks.append(sock)
        mapped_to.append(found)
    others = [r for r in range(runners) if r != args.runner]
    # Every runner must have the same number of flows, or socket i has nobody to talk to.
    mine = {"runner": args.runner}
    if len(socks) == UDP_FLOWS:
        mine["endpoints"] = [list(found) for found in mapped_to]
    encrypt(json.dumps(mine), folder / "udp.enc", server)
    result = {"checked": True, "endpoint": "endpoints" in mine, "flows": UDP_FLOWS, "peers": {}, "unknown": []}
    if "endpoints" not in mine:
        # Without addresses of its own nothing can be told about this runner.
        result["unknown"] = others
        (folder / "result.json").write_text(json.dumps(result))
        return

    # Wait for the others' addresses, keeping the mappings alive meanwhile.
    stun_request = struct.pack("!HHI", 0x0001, 0, STUN_COOKIE) + secrets.token_bytes(12)
    try:
        stun_target = socket.getaddrinfo(*STUN_SERVERS[0], socket.AF_INET, socket.SOCK_DGRAM)[0][4]
    except OSError:
        stun_target = None
    targets = {}
    answered = set()
    deadline = began + UDP_WAIT_SECONDS
    last_keepalive = 0.0
    while len(answered) < len(others) and time.time() < deadline:
        for other in others:
            if other in answered:
                continue
            where = folder / f"from-{other}"
            if fetch_artifact(where, f"room-udp-{other}-{attempt}", "udp.enc", 0):
                answered.add(other)
                told = json.loads(decrypt(where / "udp.enc", server))
                if len(told.get("endpoints") or []) == UDP_FLOWS:
                    targets[other] = [tuple(endpoint) for endpoint in told["endpoints"]]
        if time.time() - last_keepalive > 10 and stun_target:
            for sock in socks:
                try:
                    sock.sendto(stun_request, stun_target)
                except OSError:
                    pass
            last_keepalive = time.time()
        if len(answered) < len(others):
            time.sleep(2)
    result["unknown"] = [r for r in others if r not in targets]
    result["waited_seconds"] = int(time.time() - began)

    # Everyone sends to everyone, flow by flow, and counts what comes.
    token = f"room-check-udp {hashed('udp-probe')} ".encode()
    peers = {
        str(other): {"flows": UDP_FLOWS, "sent": 0, "received": 0, "flows_received": 0, "first_ms": None,
                     "during_sent": 0, "during_received": 0, "during_flows_received": 0}
        for other in targets
    }
    seen = {str(other): set() for other in targets}
    seen_during = {str(other): set() for other in targets}
    probing = time.time()

    def send_all(key):
        for other, endpoints in targets.items():
            for index, endpoint in enumerate(endpoints):
                try:
                    socks[index].sendto(token + f"{args.runner}:{index}".encode(), endpoint)
                    peers[str(other)][key] += 1
                except OSError:
                    pass

    def receive(until, during):
        while time.time() < until:
            ready, _, _ = select.select(socks, [], [], 0.05)
            for index, sock in enumerate(socks):
                if sock not in ready:
                    continue
                while True:
                    try:
                        data, _ = sock.recvfrom(2048)
                    except OSError:
                        # Nothing more, or (on Windows) a closed port on the other side; neither is an answer.
                        break
                    if not data.startswith(token):
                        continue
                    try:
                        other, flow = data[len(token):].decode().split(":")
                        other, flow = str(int(other)), int(flow)
                    except ValueError:
                        continue
                    if other not in peers or flow != index:
                        continue
                    entry = peers[other]
                    if during:
                        entry["during_received"] += 1
                        seen_during[other].add(index)
                    else:
                        entry["received"] += 1
                        seen[other].add(index)
                        if entry["first_ms"] is None:
                            entry["first_ms"] = int(1000 * (time.time() - probing))

    while time.time() < probing + UDP_PROBE_SECONDS:
        send_all("sent")
        receive(time.time() + 0.25, False)
    for other in peers:
        peers[other]["flows_received"] = len(seen[other])
    result["peers"] = peers
    (folder / "result.json").write_text(json.dumps(result))

    # Go on at one packet a second per flow until the measuring is over: a pair whose flows
    # come at first and not later (or the other way round) is told by the two parts.
    end = time.time() + UDP_KEEP_SECONDS
    while not (folder / "stop").exists() and time.time() < end:
        send_all("during_sent")
        receive(time.time() + 1.0, True)
    for other in peers:
        peers[other]["during_flows_received"] = len(seen_during[other])
    for sock in socks:
        sock.close()
    (folder / "final.json").write_text(json.dumps(result))


def udp_start(args):
    """Starts the check in the background, and returns once the address it will tell is written.

    The socket has to stay open until the others have sent to it, so the check
    outlives this step; `udp-wait` is where it is waited for. Whatever goes wrong
    here, the step succeeds: it only means this runner's pairs are not known.
    """
    folder = udp_folder(args.work)
    folder.mkdir(parents=True, exist_ok=True)
    detach = (
        {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
        if os.name == "nt" else {"start_new_session": True}
    )
    log = open(folder / "serve.log", "wb")
    process = subprocess.Popen(
        [sys.executable, __file__, "udp-serve", "--work", args.work, "--runner", str(args.runner),
         "--server-env", args.server_env],
        stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, **detach,
    )
    (folder / "serve.pid").write_text(str(process.pid))
    deadline = time.time() + 60
    while time.time() < deadline and process.poll() is None and not (folder / "udp.enc").exists():
        time.sleep(0.5)
    print("udp: " + ("the address is written" if (folder / "udp.enc").exists() else "no address"))


def udp_wait(args):
    """Waits for the check that `udp-start` began, and says how many of the others it reached."""
    folder = udp_folder(args.work)
    deadline = time.time() + UDP_WAIT_SECONDS + UDP_PROBE_SECONDS + 120
    while time.time() < deadline and not (folder / "result.json").exists():
        time.sleep(1)
    try:
        result = json.loads((folder / "result.json").read_text())
    except (OSError, ValueError):
        print("udp: no result")
        return
    reached = [r for r, peer in result["peers"].items() if peer["received"] > 0]
    silent = [r for r, peer in result["peers"].items() if peer["received"] == 0]
    print(
        f"udp: heard from {len(reached)} of {len(result['peers'])} runners"
        + (f", nothing from {','.join(sorted(silent))}" if silent else "")
        + (f", no address for {','.join(map(str, result['unknown']))}" if result["unknown"] else "")
    )


def read_time(path):
    try:
        user, system, elapsed, rss_kb = Path(path).read_text().split()[-4:]
    except (OSError, ValueError):
        return {}
    found = {"elapsed_seconds": float(elapsed)}
    if user != "-":
        found["cpu_seconds"] = float(user) + float(system)
        found["max_rss_mb"] = int(rss_kb) / 1024
    return found


def tone_tables(frequency, rate):
    count = int(WINDOW_SECONDS * rate)
    step = 2 * math.pi * frequency / rate
    return (
        [math.cos(step * n) for n in range(count)],
        [math.sin(step * n) for n in range(count)],
    )


def read_tail(path, rate, seconds):
    """The last `seconds` of what a participant played: one channel, decimated.

    The file is raw f32 stereo from the start of the session.
    """
    frames = Path(path).stat().st_size // 8
    first = max(0, frames - int(seconds * rate))
    data = array.array("f")
    with open(path, "rb") as handle:
        handle.seek(first * 8)
        data.frombytes(handle.read((frames - first) * 8))
    left = data[0::2]
    usable = len(left) // DECIMATE * DECIMATE
    return [sum(group) for group in zip(*(left[k:usable:DECIMATE] for k in range(DECIMATE)))]


def levels_per_window(samples, rate, sources):
    """The level of each source's tone in every 100 ms window of `samples`."""
    decimated_rate = rate / DECIMATE
    size = int(WINDOW_SECONDS * decimated_rate)
    levels = {}
    for source in sources:
        cosine, sine = tone_tables(tone_of(source), decimated_rate)
        found = []
        for w in range(len(samples) // size):
            chunk = samples[w * size:(w + 1) * size]
            # `chunk` holds sums of DECIMATE samples, hence the division by it.
            found.append(
                2 * math.hypot(sum(map(mul, chunk, cosine)), sum(map(mul, chunk, sine))) / size / DECIMATE
            )
        levels[source] = found
    return levels


def turn_windows(speaker, offset, slot, tail_windows, start_to_end):
    """The windows a speaker's turn covers, without its edges.

    Window w of a tail of `tail_windows` covers the file seconds
    `[w, w + 1) * WINDOW_SECONDS - tail`, counted from the end of the file, which
    is `offset` seconds off from the time the run was meant to end.
    """
    guard = min(GUARD_SECONDS, slot / 4)
    from_s = speaker * slot + guard - start_to_end + offset
    until_s = (speaker + 1) * slot - guard - start_to_end + offset
    tail = tail_windows * WINDOW_SECONDS
    first = math.ceil((from_s + tail) / WINDOW_SECONDS)
    last = math.floor((until_s + tail) / WINDOW_SECONDS)
    return list(range(max(first, 0), min(last, tail_windows)))


def analyze(work, index, room):
    """What participant `index` heard of each of the others in their turns."""
    out = work / "out" / name_of(index)
    total, slot = room["total"], room["slot"]
    layout = room["layout"]
    result = {"id": index, "runner": runner_of(layout, index)}
    result.update(read_time(str(out) + ".time"))
    report = {}
    try:
        report = json.loads(Path(str(out) + ".json").read_text())
    except (OSError, ValueError):
        pass
    rate = report.get("audio", {}).get("sample_rate", 48000)
    sources = [j for j in range(total) if j != index]
    # From the first turn's start to the time the run was meant to end.
    start_to_end = total * slot + TAIL_SECONDS
    tail_seconds = start_to_end + QUIET_SECONDS + SEARCH_SECONDS
    played = Path(str(out) + ".f32")
    levels, tail_windows = {}, 0
    if played.exists():
        samples = read_tail(played, rate, tail_seconds)
        levels = levels_per_window(samples, rate, sources)
        tail_windows = len(samples) // int(WINDOW_SECONDS * rate / DECIMATE)

    def windows_of(speaker, offset):
        return [
            w for w in turn_windows(speaker, offset, slot, tail_windows, start_to_end)
            if w < len(levels.get(speaker, []))
        ]

    # The end of the file is not exactly the time the run was meant to end;
    # take the shift (in 100 ms steps) that puts the most sound into the turns.
    best, best_offset = -1.0, 0.0
    steps = int(SEARCH_SECONDS / WINDOW_SECONDS)
    for step in range(-steps, steps + 1):
        offset = step * WINDOW_SECONDS
        score = sum(levels[speaker][w] for speaker in sources for w in windows_of(speaker, offset))
        if score > best:
            best, best_offset = score, offset
    heard, level, leak = {}, {}, {}
    for speaker in sources:
        series = levels.get(speaker, [])
        expected = max(1, len(turn_windows(speaker, best_offset, slot, tail_windows, start_to_end)))
        present = [series[w] for w in windows_of(speaker, best_offset) if series[w] >= PRESENT_AMPLITUDE]
        heard[str(speaker)] = min(1.0, len(present) / expected)
        level[str(speaker)] = statistics.median(present) if present else 0.0
        elsewhere = [w for other in sources if other != speaker for w in windows_of(other, best_offset)]
        loud = [w for w in elsewhere if series[w] >= PRESENT_AMPLITUDE]
        leak[str(speaker)] = len(loud) / len(elsewhere) if elsewhere else 0.0
    result.update({"heard": heard, "level": level, "leak": leak, "offset": best_offset})
    result.update({
        "os": os.environ.get("RUNNER_OS", report.get("os", "")).lower(),
        "version": report.get("jamjam_version"),
        "commit": os.environ.get("GITHUB_SHA", "")[:10] or None,
    })
    links = []
    for peer in report.get("peers", []):
        other = participant_of(peer.get("name", ""))
        if other is None:
            continue
        network, playout = peer.get("network", {}), peer.get("playout", {})
        facts = peer.get("link") or {}
        route = facts.get("route")
        if runner_of(layout, other) == result["runner"] or route == "loopback":
            scope = "machine"
        elif route == "lan":
            scope = "network"
        else:
            scope = "direct"
        if scope != "machine" and not playout.get("frames_played"):
            # Up on an address that never answered (or not up at all): nothing was heard on it.
            scope = "unreached"
        address = peer.get("addr")
        links.append({
            "peer": other,
            "packets_received": network.get("packets_received", 0),
            "rtt_ms": network.get("rtt_ms"),
            "loss": network.get("packet_loss_rate", 0.0),
            "played": playout.get("frames_played", 0),
            "concealed": playout.get("frames_concealed", 0),
            "late": playout.get("frames_late", 0),
            # The kind of route is told by the address; the address itself is not kept.
            "scope": scope,
            "route": route,
            "route_confirmed": facts.get("route_confirmed"),
            "family": facts.get("family"),
            "connect_ms": facts.get("connect_ms"),
            "first_audio_ms": facts.get("first_audio_ms"),
            "route_moves": facts.get("route_moves"),
            "route_reason": facts.get("route_reason"),
            "first_sent_unix_ms": facts.get("first_sent_unix_ms"),
            "first_heard_unix_ms": facts.get("first_heard_unix_ms"),
            "candidates_lan": facts.get("candidates_lan"),
            "candidates_public": facts.get("candidates_public"),
            "sent_to_other": facts.get("sent_to_other"),
            "punch_tries": facts.get("punch_tries"),
            "punched_first": facts.get("punched_first"),
            "connected_after_seconds": peer.get("connected_after_seconds"),
            "buffer_ms": peer.get("buffer_ms"),
            "delay_frames": playout.get("delay_frames"),
            "end_to_end_ms": peer.get("end_to_end_ms"),
            "rtt_spread_ms": peer.get("rtt_spread_ms"),
            "end_to_end_spread_ms": peer.get("end_to_end_spread_ms"),
            "address_hash": hashed(address.rsplit(":", 1)[0].strip("[]")) if address else None,
        })
    # A peer that left and came back, or whose link was opened again, has several: the one
    # that carried the most is the link, the others are counted.
    best = {}
    opened = {}
    for link in links:
        opened[link["peer"]] = opened.get(link["peer"], 0) + 1
        if link["peer"] not in best or link["played"] > best[link["peer"]]["played"]:
            best[link["peer"]] = link
    links = [dict(best[peer], opened=opened[peer]) for peer in sorted(best)]
    result["links"] = links
    came_up = [link["connected_after_seconds"] for link in links if link["connected_after_seconds"] is not None]
    result["all_linked_seconds"] = max(came_up) if len(came_up) == total - 1 else None
    return result


def describe(result):
    heard = list(result["heard"].values())
    low = min(heard) if heard else 0.0
    cpu = ""
    if result.get("elapsed_seconds") and "cpu_seconds" in result:
        cpu = f", cpu {100 * result['cpu_seconds'] / result['elapsed_seconds']:.0f}%"
    return f"{name_of(result['id'])}: heard {sum(1 for h in heard if h > 0)} of {len(heard)} speakers, lowest share {low:.2f}{cpu}"


def udp_status(udp, first, second):
    """Whether every UDP flow between two runners came, in both directions, before the turns and while they went on.

    `udp` maps a runner to what it found. "open" (every flow came) and "impaired" (some
    were lost) are told only when both sent to the other and counted what came;
    otherwise it is None.
    """
    for here, there in ((first, second), (second, first)):
        peer = ((udp.get(here) or {}).get("peers") or {}).get(str(there))
        if peer is None:
            return None
        flows = peer.get("flows", 1)
        if peer["flows_received"] < flows or peer.get("during_flows_received", flows) < flows:
            return "impaired"
    return "open"


def first_packet_lines(results, min_heard):
    """Per pair of participants on different runners: which end sent its first packet first, and how it ended.

    The gap is the first packet of the lower-numbered end minus that of the higher-numbered one, so a
    negative gap is the lower one first. A pair that was not heard, or that had to send again after
    a quiet while, is listed; the others are counted.
    """
    by_id = {result["id"]: result for result in results}
    rows = []
    total = 0
    for result in results:
        for link in result["links"]:
            other = by_id.get(link["peer"])
            if other is None or other["runner"] == result["runner"] or result["id"] > other["id"]:
                continue
            back = next((l for l in other["links"] if l["peer"] == result["id"]), None)
            if back is None:
                continue
            total += 1
            sent_here, sent_there = link.get("first_sent_unix_ms"), back.get("first_sent_unix_ms")
            gap = sent_here - sent_there if sent_here is not None and sent_there is not None else None
            heard_here = result["heard"].get(str(other["id"]), 0.0)
            heard_there = other["heard"].get(str(result["id"]), 0.0)
            tries = (link.get("punch_tries") or 0) + (back.get("punch_tries") or 0)
            if min(heard_here, heard_there) < min_heard or tries:
                rows.append((result, other, link, back, gap, heard_here, heard_there, tries))
    if not total:
        return []
    out = [f"First packets between participants on different runners: {total} pairs, {len(rows)} listed (not heard, or sent again after a quiet)."]
    for result, other, link, back, gap, heard_here, heard_there, tries in rows:
        gap_text = f"{gap:+d} ms" if gap is not None else "unknown"
        out.append(
            f"  {name_of(result['id'])} {result.get('os') or '?'} - {name_of(other['id'])} {other.get('os') or '?'}: "
            f"first packet gap {gap_text}, heard {heard_here:.2f} / {heard_there:.2f}, "
            f"routes {link.get('route')} ({link.get('route_reason')}) / {back.get('route')} ({back.get('route_reason')}), "
            f"candidates {link.get('candidates_lan')}+{link.get('candidates_public')} / {back.get('candidates_lan')}+{back.get('candidates_public')}, "
            f"sent to other addresses {link.get('sent_to_other')} / {back.get('sent_to_other')}, "
            f"sent again {link.get('punch_tries')} / {back.get('punch_tries')}, "
            f"connected after {link.get('connected_after_seconds')} / {back.get('connected_after_seconds')} s"
        )
    return out


def summary(args):
    results = []
    for path in sorted(Path(args.results).rglob("p*.json")):
        results.append(json.loads(path.read_text()))
    by_id = {result["id"]: result for result in results}
    udp = {}
    systems = {}
    for result in results:
        udp.setdefault(result["runner"], (result.get("runner_facts") or {}).get("udp") or {})
        systems[result["runner"]] = result.get("os") or "?"
    failures = []
    lines = []
    same_runner, across_runners, leaks = [], [], []
    by_system = {}
    blocked, mismatches = [], []
    for index in range(args.total):
        result = by_id.get(index)
        if result is None:
            failures.append(f"{name_of(index)} has no result")
            lines.append(f"{name_of(index)}: no result")
            continue
        for speaker, share in result["heard"].items():
            other = by_id.get(int(speaker))
            status = udp_status(udp, result["runner"], other["runner"]) if other and other["runner"] != result["runner"] else None
            if status == "impaired" and share < args.min_heard:
                # UDP flows between these runners were lost with or without the app: the environment, not the app.
                blocked.append((index, int(speaker), share))
                continue
            if status == "open" and share < args.min_heard:
                mismatches.append(f"{name_of(index)} did not hear {name_of(int(speaker))} although every UDP flow between their runners came")
            (same_runner if other and other["runner"] == result["runner"] else across_runners).append(share)
            if other:
                by_system.setdefault((result.get("os") or "?", other.get("os") or "?"), []).append(share)
            if share < args.min_heard:
                failures.append(f"{name_of(index)} heard {name_of(int(speaker))} in {share:.2f} of its turn")
        for speaker, share in result["leak"].items():
            leaks.append(share)
            if share > 0:
                failures.append(f"{name_of(index)} heard {name_of(int(speaker))} in others' turns ({share:.2f})")
        rtts = [link["rtt_ms"] for link in result["links"] if link["rtt_ms"] is not None]
        played = sum(link["played"] for link in result["links"])
        concealed = sum(link["concealed"] for link in result["links"])
        concealed_share = concealed / (played + concealed) if played + concealed else 0.0
        cpu = (
            100 * result["cpu_seconds"] / result["elapsed_seconds"]
            if result.get("elapsed_seconds") and "cpu_seconds" in result else float("nan")
        )
        lines.append(
            f"{name_of(index)} runner {result['runner']} {result.get('os') or '?'}: "
            f"lowest {min(result['heard'].values(), default=0.0):.2f} "
            f"links {sum(1 for link in result['links'] if link['played'] > 0)} "
            f"concealed {100 * concealed_share:.1f}% "
            f"rtt median {statistics.median(rtts) if rtts else float('nan'):.1f} ms "
            f"cpu {cpu:.0f}% rss {result.get('max_rss_mb', float('nan')):.0f} MB "
            f"shift {result['offset']:+.1f} s"
        )
    lines.append("")
    for label, shares in (("same runner", same_runner), ("across runners", across_runners)):
        if shares:
            lines.append(f"{label}: {len(shares)} pairs, lowest {min(shares):.2f}, mean {statistics.mean(shares):.2f}")
        else:
            lines.append(f"{label}: no pairs")
    for (listening, speaking), shares in sorted(by_system.items()):
        lines.append(
            f"{listening} hears {speaking}: {len(shares)} pairs, lowest {min(shares):.2f}, mean {statistics.mean(shares):.2f}"
        )
    if leaks:
        lines.append(f"heard in others' turns: highest {max(leaks):.2f}")
    runner_pairs = sorted({tuple(sorted((a, b))) for a in udp for b in udp if a != b})
    verdicts = {pair: udp_status(udp, *pair) for pair in runner_pairs}
    if any(status for status in verdicts.values()):
        lines.append("")
        lines.append("UDP flows between runners, sent without the app (before the turns / during them):")
        for (a, b), status in verdicts.items():
            peer_a = ((udp.get(a) or {}).get("peers") or {}).get(str(b)) or {}
            peer_b = ((udp.get(b) or {}).get("peers") or {}).get(str(a)) or {}

            def came(peer):
                return f"{peer.get('flows_received')}/{peer.get('flows')} / {peer.get('during_flows_received')}/{peer.get('flows')}"

            counts = f" (flows that came: {came(peer_a)} and {came(peer_b)})" if status else ""
            lines.append(f"  runner {a} {systems.get(a)} - runner {b} {systems.get(b)}: {status or 'not checked'}{counts}")
    order = first_packet_lines(results, args.min_heard)
    if order:
        lines.append("")
        lines.extend(order)
    if blocked:
        pairs = len({tuple(sorted((a, b))) for a, b in ((by_id[i]["runner"], by_id[j]["runner"]) for i, j, _ in blocked)})
        lines.append(
            f"NOT MEASURED because of the environment: {len(blocked)} listener-speaker pairs on {pairs} pairs of runners, "
            "where UDP flows were lost even without the app"
        )
    for mismatch in mismatches:
        lines.append(f"  {mismatch}")
    if len({result["runner"] for result in results}) > 1 and not across_runners:
        failures.append("no pair of different runners could be measured")
    verdict = "FAIL" if failures else "PASS"
    lines.append(f"{verdict}: {args.total} participants, every speaker needs {args.min_heard:.2f} of their turn")
    for failure in failures[:20]:
        lines.append(f"  {failure}")
    text = "\n".join(lines)
    print(text)
    summary_file = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary_file:
        with open(summary_file, "a") as handle:
            handle.write("```\n" + text + "\n```\n")
    sys.exit(1 if failures else 0)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)

    def common(sub):
        sub.add_argument("--work", required=True)
        sub.add_argument("--server-env", default="JAMJAM_SERVER_URL_BETA")

    host = commands.add_parser("start-host")
    common(host)
    host.add_argument("--binary", required=True)
    host.add_argument("--layout", required=True, help="participants on each runner, such as 2,2,1")
    host.add_argument("--slot", type=int, required=True, help="seconds each participant speaks")
    host.set_defaults(handler=start_host)

    get = commands.add_parser("fetch")
    common(get)
    get.add_argument("--timeout", type=int, default=600)
    get.set_defaults(handler=fetch)

    for name, handler in (("run", run), ("join", join), ("measure", measure)):
        sub = commands.add_parser(name)
        common(sub)
        sub.add_argument("--runner", type=int, required=True)
        if name != "measure":
            sub.add_argument("--binary", required=True)
        sub.set_defaults(handler=handler)

    for name, handler in (("udp-start", udp_start), ("udp-serve", udp_serve), ("udp-wait", udp_wait)):
        sub = commands.add_parser(name)
        common(sub)
        if name != "udp-wait":
            sub.add_argument("--runner", type=int, required=True)
        sub.set_defaults(handler=handler)

    watch = commands.add_parser("supervise")
    watch.add_argument("--out", required=True)
    watch.add_argument("--start-file", required=True)
    watch.add_argument("--index", type=int, required=True)
    watch.add_argument("--total", type=int, required=True)
    watch.add_argument("--slot", type=int, required=True)
    watch.add_argument("command", nargs=argparse.REMAINDER)
    watch.set_defaults(handler=supervise)

    tally = commands.add_parser("summary")
    tally.add_argument("--results", required=True)
    tally.add_argument("--total", type=int, required=True)
    tally.add_argument("--min-heard", type=float, default=DEFAULT_MIN_HEARD)
    tally.set_defaults(handler=summary)

    args = parser.parse_args()
    args.handler(args)


if __name__ == "__main__":
    main()
