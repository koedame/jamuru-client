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

The output of this script is the only thing that reaches the log: pass or fail
and numbers per participant. What the server answers, what `jamuru` prints and
where the participants connected from are never printed.

Standard library only, so that CI installs nothing it does not pin.
"""

import argparse
import array
import json
import math
import os
import signal
import statistics
import subprocess
import sys
import time
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


def name_of(index):
    return f"p{index:02d}"


def tone_of(index):
    return TONE_BASE_HZ + TONE_STEP_HZ * index


def server_url(env_name):
    url = os.environ.get(env_name, "").strip()
    if not url:
        sys.exit(f"{env_name} is not set")
    return url


def schedule(start_at, total, slot):
    """When each participant speaks, and when the run ends.

    Returns `(turns, end_at)`; `turns[i]` is the `(from, until)` of participant i.
    """
    turns = [(start_at + i * slot, start_at + (i + 1) * slot) for i in range(total)]
    return turns, start_at + total * slot + TAIL_SECONDS


def launch(binary, work, index, command, server, room, turn, quiet_at, end_at):
    """Starts participant `index`; it speaks in `turn`, then leaves at `end_at`.

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
        name_of(index),
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
        "--quiet-at", str(quiet_at),
        "--speak-from", str(turn[0]),
        "--speak-until", str(turn[1]),
        "--end-at", str(end_at),
        "--", *jamuru,
    ]
    env = dict(os.environ, HOME=str(home), XDG_CONFIG_HOME=str(home / ".config"))
    log = open(str(out) + ".log", "wb")
    process = subprocess.Popen(
        args,
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=subprocess.STDOUT,
        env=env,
        start_new_session=True,
    )
    return process, out


def supervise(args):
    """Runs a command through its turn and until `--end-at`, and writes what it cost.

    The session reads its commands from standard input: `/mute` and `/unmute`
    switch what it sends.
    """
    started = time.time()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    child = subprocess.Popen(command, stdin=subprocess.PIPE)
    steps = [(args.quiet_at, b"/mute\n"), (args.speak_from, b"/unmute\n"), (args.speak_until, b"/mute\n")]
    interrupted_at = None
    while True:
        done, _, usage = os.wait4(child.pid, os.WNOHANG)
        if done:
            break
        now = time.time()
        while steps and now >= steps[0][0]:
            try:
                child.stdin.write(steps.pop(0)[1])
                child.stdin.flush()
            except OSError:
                steps.clear()
        if interrupted_at is None and now >= args.end_at:
            child.send_signal(signal.SIGINT)
            interrupted_at = now
        elif interrupted_at is not None and now >= interrupted_at + 30:
            child.kill()
        time.sleep(0.1)
    Path(args.out + ".time").write_text(
        f"{usage.ru_utime} {usage.ru_stime} {time.time() - started} {usage.ru_maxrss}\n"
    )


def encrypt(plain, destination, key):
    subprocess.run(
        ["openssl", "enc", "-aes-256-cbc", "-pbkdf2", "-salt", "-pass", "env:ROOM_CHECK_KEY",
         "-out", str(destination)],
        input=plain.encode(),
        env=dict(os.environ, ROOM_CHECK_KEY=key),
        check=True,
    )


def decrypt(source, key):
    done = subprocess.run(
        ["openssl", "enc", "-d", "-aes-256-cbc", "-pbkdf2", "-pass", "env:ROOM_CHECK_KEY",
         "-in", str(source)],
        env=dict(os.environ, ROOM_CHECK_KEY=key),
        capture_output=True,
        check=True,
    )
    return done.stdout.decode()


def start_host(args):
    """Creates the room as participant 0 and publishes how to join it."""
    server = server_url(args.server_env)
    work = Path(args.work)
    start_at = time.time() + args.lead
    turns, end_at = schedule(start_at, args.total, args.slot)
    process, out = launch(
        Path(args.binary), work, 0, "create-room", server, None, turns[0], start_at - QUIET_SECONDS, end_at
    )
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
    room = {"code": code, "start_at": start_at, "slot": args.slot, "total": args.total}
    (work / "room.json").write_text(json.dumps(room))
    published = work / "rendezvous"
    published.mkdir(exist_ok=True)
    encrypt(json.dumps(room), published / "room.enc", server)
    print(f"host: room created, the others have {int(start_at - time.time())} s to join")


def fetch(args):
    """Waits for the host's file in this workflow run and reads it."""
    server = server_url(args.server_env)
    work = Path(args.work)
    work.mkdir(parents=True, exist_ok=True)
    repo, run_id = os.environ["GITHUB_REPOSITORY"], os.environ["GITHUB_RUN_ID"]
    attempt = os.environ["GITHUB_RUN_ATTEMPT"]
    name = f"room-rendezvous-{attempt}"
    deadline = time.time() + args.timeout
    while time.time() < deadline:
        listed = subprocess.run(
            ["gh", "api", f"repos/{repo}/actions/runs/{run_id}/artifacts?name={name}",
             "--jq", ".artifacts[0].id // empty"],
            capture_output=True, text=True,
        )
        artifact = listed.stdout.strip()
        if listed.returncode == 0 and artifact:
            archive = work / "rendezvous.zip"
            with open(archive, "wb") as handle:
                done = subprocess.run(
                    ["gh", "api", f"repos/{repo}/actions/artifacts/{artifact}/zip"],
                    stdout=handle,
                )
            if done.returncode == 0:
                with zipfile.ZipFile(archive) as bundle:
                    bundle.extract("room.enc", work)
                (work / "room.json").write_text(decrypt(work / "room.enc", server))
                print("fetch: got the room")
                return
        time.sleep(5)
    print("fetch: the host published nothing in time")
    sys.exit(1)


def pid_alive(pid):
    try:
        os.kill(pid, 0)
    except OSError:
        return False
    return True


def run(args):
    """Starts this runner's participants and measures them once the turns are over."""
    server = server_url(args.server_env)
    work = Path(args.work)
    room = json.loads((work / "room.json").read_text())
    total, slot = room["total"], room["slot"]
    turns, end_at = schedule(room["start_at"], total, slot)
    own = [
        args.runner * args.per_runner + k
        for k in range(args.per_runner)
        if args.runner * args.per_runner + k < total
    ]
    processes = []
    for index in own:
        if index == 0:
            continue
        processes.append(launch(
            Path(args.binary), work, index, "join-room", server, room["code"],
            turns[index], room["start_at"] - QUIET_SECONDS, end_at,
        )[0])
    for process in processes:
        process.wait()
    host_pid = work / "host.pid"
    if host_pid.exists():
        pid = int(host_pid.read_text())
        while pid_alive(pid) and time.time() < end_at + 30:
            time.sleep(1)

    results = work / "results"
    results.mkdir(exist_ok=True)
    for index in own:
        result = analyze(work, index, args.per_runner, room)
        (results / f"{name_of(index)}.json").write_text(json.dumps(result))
        print(describe(result))


def read_time(path):
    try:
        user, system, elapsed, rss_kb = Path(path).read_text().split()[-4:]
    except (OSError, ValueError):
        return {}
    return {
        "cpu_seconds": float(user) + float(system),
        "elapsed_seconds": float(elapsed),
        "max_rss_mb": int(rss_kb) / 1024,
    }


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


def analyze(work, index, per_runner, room):
    """What participant `index` heard of each of the others in their turns."""
    out = work / "out" / name_of(index)
    total, slot = room["total"], room["slot"]
    result = {"id": index, "runner": index // per_runner}
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
    links = []
    for peer in report.get("peers", []):
        name = peer.get("name", "")
        if not (name.startswith("p") and name[1:].isdigit()):
            continue
        network, playout = peer.get("network", {}), peer.get("playout", {})
        links.append({
            "peer": int(name[1:]),
            "packets_received": network.get("packets_received", 0),
            "rtt_ms": network.get("rtt_ms"),
            "loss": network.get("packet_loss_rate", 0.0),
            "played": playout.get("frames_played", 0),
            "concealed": playout.get("frames_concealed", 0),
            "late": playout.get("frames_late", 0),
        })
    result["links"] = links
    return result


def describe(result):
    heard = list(result["heard"].values())
    low = min(heard) if heard else 0.0
    cpu = ""
    if result.get("elapsed_seconds"):
        cpu = f", cpu {100 * result['cpu_seconds'] / result['elapsed_seconds']:.0f}%"
    return f"{name_of(result['id'])}: heard {sum(1 for h in heard if h > 0)} of {len(heard)} speakers, lowest share {low:.2f}{cpu}"


def summary(args):
    results = []
    for path in sorted(Path(args.results).rglob("p*.json")):
        results.append(json.loads(path.read_text()))
    by_id = {result["id"]: result for result in results}
    failures = []
    lines = []
    same_runner, across_runners, leaks = [], [], []
    for index in range(args.total):
        result = by_id.get(index)
        if result is None:
            failures.append(f"{name_of(index)} has no result")
            lines.append(f"{name_of(index)}: no result")
            continue
        for speaker, share in result["heard"].items():
            other = by_id.get(int(speaker))
            (same_runner if other and other["runner"] == result["runner"] else across_runners).append(share)
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
            if result.get("elapsed_seconds") else float("nan")
        )
        lines.append(
            f"{name_of(index)} runner {result['runner']}: "
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
    if leaks:
        lines.append(f"heard in others' turns: highest {max(leaks):.2f}")
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
    host.add_argument("--total", type=int, required=True)
    host.add_argument("--slot", type=int, required=True, help="seconds each participant speaks")
    host.add_argument("--lead", type=int, required=True, help="seconds the others have to arrive")
    host.set_defaults(handler=start_host)

    get = commands.add_parser("fetch")
    common(get)
    get.add_argument("--timeout", type=int, default=600)
    get.set_defaults(handler=fetch)

    measure = commands.add_parser("run")
    common(measure)
    measure.add_argument("--binary", required=True)
    measure.add_argument("--runner", type=int, required=True)
    measure.add_argument("--per-runner", type=int, required=True)
    measure.set_defaults(handler=run)

    watch = commands.add_parser("supervise")
    watch.add_argument("--out", required=True)
    watch.add_argument("--quiet-at", type=float, required=True)
    watch.add_argument("--speak-from", type=float, required=True)
    watch.add_argument("--speak-until", type=float, required=True)
    watch.add_argument("--end-at", type=float, required=True)
    watch.add_argument("command", nargs=argparse.REMAINDER)
    watch.set_defaults(handler=supervise)

    join = commands.add_parser("summary")
    join.add_argument("--results", required=True)
    join.add_argument("--total", type=int, required=True)
    join.add_argument("--min-heard", type=float, default=DEFAULT_MIN_HEARD)
    join.set_defaults(handler=summary)

    args = parser.parse_args()
    args.handler(args)


if __name__ == "__main__":
    main()
