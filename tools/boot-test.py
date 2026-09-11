#!/usr/bin/env python3
"""Boots an image through the flake's vm app and checks that the system comes up. The boot job in ci
runs this.

Usage: boot-test.py <eclipse-vm> <image.raw> <passfile> [--models dir] [--timeout 600] [--log serial.log]
       [--splash splash.png] [--desktop desktop.png] [--corona]

<eclipse-vm> is the program from `nix build .#vm` (result/bin/eclipse-vm). It adds the persist
partition to the image with the passphrase from the passfile (persist-image.sh, through sudo), then
boots the image as an nvme drive. Everything goes through the serial console: the luks prompt, the
autologin shell, a few commands, the default apps on the path, the host profile syzygy wrote and
what `eclipse host` and `eclipse doctor` print. The serial output is printed as it arrives and kept
in the log file.

With --models the files in that directory go into the @models subvolume before boot. The test waits
on the system bus until aurad has loaded the model it picked for syzygy's tier, checks that it is the
one in the directory, asks the local api for a short completion and asks aura a question over the bus
and through `eclipse ai`.
The local api has to refuse the same completion when the request comes with a web page's Origin or
Host header, and the owner must not reach llama-server's socket behind it.

With --splash the test also takes a screendump through the qemu monitor while the luks prompt is up
and checks that the Totality splash is on screen: the light disc and the black disc from
nix/totality/plymouth against the gray background. The dump is saved as a png.

With --desktop the test checks that greetd is up and takes a screendump of the running session: umbra
paints its background gray over the whole screen, a console would show black with text. The vm has a
virtio gpu for this, umbra renders on it in software.

With --corona the desktop check expects corona's panel along the top of that screen: umbra reports a
layer surface with its namespace, and the screendump has the panel gray, the field inside it and the
desktop gray below. The test then types into the field from the serial shell with `corona --enter`
and looks again: a nushell pipeline puts three rows under the field, a command with arguments it does
not know puts an error line there, and `corona --escape` leaves the panel the height it started at.
With --models as well, a question goes through `corona --do`, which prints aura's answer, and then
into the field, where the answer shows up as rows under it.
"""

import argparse
import glob
import json
import math
import os
import re
import socket
import struct
import sys
import tempfile
import time
import tomllib
import zlib

import pexpect

# the fish prompt is user@host with colour codes in between
PROMPT = r"eclipse(\x1b\[[0-9;]*m)*@(\x1b\[[0-9;]*m)*eclipse"
PASSPHRASE = r"(?i)passphrase[^\r\n]*:"
# fish marks every command line it runs: osc 133;C when it starts and 133;D;<status> when it is
# done. it also repaints the prompt whenever the journal writes to the console, so a prompt is not
# where a command's output ends, these marks are
COMMAND_START = r"\x1b\]133;C[^\x07\x1b]*(?:\x07|\x1b\\)"
COMMAND_END = r"\x1b\]133;D;(\d+)(?:\x07|\x1b\\)"
ESCAPES = re.compile(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-9;?>=]*[A-Za-z]|\x1b[=>]")


def without_console(output):
    """What a command printed, without the journal's lines that reach the serial console while it
    runs and without blank lines at either end."""
    lines = [line for line in output.splitlines() if not re.match(r"\s*\[\s*\d+\.\d+\] ", line)]
    return "\n".join(lines).strip("\n")


def first(paths):
    for pattern in paths:
        found = sorted(glob.glob(pattern))
        if found:
            return found[0]
    return None


def qmp(path, *commands):
    """Run monitor commands over the qmp socket and return their replies."""
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.settimeout(30)
    sock.connect(path)
    buf = b""
    replies = []

    def read_reply():
        nonlocal buf
        while True:
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                if not line.strip():
                    continue
                msg = json.loads(line)
                if "return" in msg or "error" in msg or "QMP" in msg:
                    return msg
            chunk = sock.recv(65536)
            if not chunk:
                raise RuntimeError("qmp socket closed")
            buf += chunk

    read_reply()  # the greeting
    for command in ({"execute": "qmp_capabilities"},) + commands:
        sock.sendall(json.dumps(command).encode() + b"\n")
        reply = read_reply()
        if "error" in reply:
            raise RuntimeError(f"qmp {command['execute']}: {reply['error']}")
        replies.append(reply["return"])
    sock.close()
    return replies[1:]


def read_ppm(path):
    """Parse a binary ppm (P6) into (width, height, bytes of rgb triples)."""
    data = open(path, "rb").read()
    fields = []
    pos = 0
    while len(fields) < 4:
        while data[pos : pos + 1].isspace():
            pos += 1
        if data[pos : pos + 1] == b"#":
            pos = data.index(b"\n", pos)
            continue
        end = pos
        while not data[end : end + 1].isspace():
            end += 1
        fields.append(data[pos:end])
        pos = end
    pos += 1
    if fields[0] != b"P6" or fields[3] != b"255":
        raise RuntimeError(f"unexpected ppm header {fields}")
    width, height = int(fields[1]), int(fields[2])
    return width, height, data[pos : pos + width * height * 3]


def write_png(path, width, height, rgb):
    raw = bytearray()
    stride = width * 3
    for y in range(height):
        raw.append(0)
        raw.extend(rgb[y * stride : (y + 1) * stride])

    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n")
        f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)))
        f.write(chunk(b"IDAT", zlib.compress(bytes(raw), 6)))
        f.write(chunk(b"IEND", b""))


# what the theme draws, from nix/totality/plymouth: background #1e1e1e, sun #cccccc, moon #000000
BACKGROUND = (30, 30, 30)
SUN = (204, 204, 204)
MOON = (0, 0, 0)
RING = 0.03
# what umbra paints with no window open, the background from nix/modules/umbra.nix
DESKTOP = (36, 36, 36)
# corona's panel, from crates/corona/src/ui.rs: panel gray, field gray, and the sizes in logical
# pixels. the panel is the field's row plus whatever the result list and the error line need
PANEL = (30, 30, 30)
FIELD = (46, 46, 46)
PANEL_HEIGHT = 32
FIELD_SIZE = (480, 24)
ROW_HEIGHT = 22
ERROR_HEIGHT = 22
BOTTOM_PAD = 4
LIST_ROWS = 8
# what the field and the list ask corona to type, and how many rows the pipeline prints
RESULT_LINE = "echo [eclipse eclipse eclipse]"
RESULT_ROWS = 3
ERROR_LINE = "wifi dance"
# a question with a short answer. the model runs on the cpu, next to umbra's software renderer
QUESTION = "What is the capital of France?"


def near(pixel, color, tolerance):
    return all(abs(a - b) <= tolerance for a, b in zip(pixel, color))


def check_splash(width, height, rgb):
    """Count the theme's colours in a screendump. Returns (ok, lines to print)."""
    size = height // 5  # the theme scales the discs to a fifth of the screen height
    cx, cy = width / 2, height / 2
    sun_radius = size / 2 - 1
    moon_radius = sun_radius - RING * size
    sun_area = math.pi * sun_radius**2
    moon_area = math.pi * moon_radius**2
    ring_area = sun_area - moon_area

    background = black = light_in_sun = black_in_sun = 0
    for y in range(height):
        row = y * width * 3
        for x in range(width):
            px = rgb[row + x * 3 : row + x * 3 + 3]
            if near(px, BACKGROUND, 8):
                background += 1
                continue
            is_black = near(px, MOON, 8)
            if is_black:
                black += 1
            if math.hypot(x + 0.5 - cx, y + 0.5 - cy) <= sun_radius + 1:
                if is_black:
                    black_in_sun += 1
                elif near(px, SUN, 12):
                    light_in_sun += 1

    total = width * height
    checks = [
        ("background covers most of the screen", background >= 0.85 * total, f"{background} of {total}"),
        ("the sun is where the theme puts it", 0.85 * sun_area <= light_in_sun + black_in_sun <= 1.1 * sun_area,
         f"{light_in_sun} light + {black_in_sun} black, expected about {sun_area:.0f}"),
        ("at least the ring of the sun shows", light_in_sun >= 0.6 * ring_area, f"{light_in_sun}, ring is {ring_area:.0f}"),
        ("the moon is on screen", 0.8 * moon_area <= black <= 1.2 * moon_area, f"{black}, expected about {moon_area:.0f}"),
    ]
    lines = [f"splash: {width}x{height}, disc size {size}"]
    ok = True
    for name, passed, detail in checks:
        lines.append(f"splash: {'ok  ' if passed else 'FAIL'} {name}: {detail}")
        ok = ok and passed
    return ok, lines


def panel_height(rows, error):
    """How tall corona's panel is with this many result rows and with or without the error line."""
    under = rows * ROW_HEIGHT + (ERROR_HEIGHT if error else 0)
    return PANEL_HEIGHT + under + BOTTOM_PAD if under else PANEL_HEIGHT


def check_desktop(width, height, rgb, corona=False, rows=0, error=False):
    """Count the desktop gray and the console's black in a screendump, and with corona the panel
    along the top, the field in it and the result list under it. rows is a count, or (fewest, most)
    when the test cannot know how many rows there are: then any count in that range that fits the
    screen passes. Returns (ok, lines to print)."""
    gray = black = panel = field = 0
    panel_rows = 0
    for y in range(height):
        row = y * width * 3
        row_panel = row_field = 0
        for x in range(width):
            px = rgb[row + x * 3 : row + x * 3 + 3]
            if near(px, DESKTOP, 3):
                gray += 1
            elif near(px, MOON, 8):
                black += 1
            elif near(px, PANEL, 3):
                row_panel += 1
            elif near(px, FIELD, 3):
                row_field += 1
        panel += row_panel
        field += row_field
        # the panel is the run of rows from the top that are mostly its two grays
        if row_panel + row_field > width / 2 and panel_rows == y:
            panel_rows += 1
    total = width * height
    checks = [
        ("no console black", black <= 0.02 * total, f"{black} of {total}"),
    ]
    if corona:
        below = total - panel_rows * width

        def panel_checks(count):
            # the compositor may scale the panel, so its size on screen gives the scale
            wanted = panel_height(count, error)
            scale = panel_rows / wanted
            # the field, and under it one field-gray rectangle as tall as the rows it holds
            field_area = (FIELD_SIZE[0] * FIELD_SIZE[1] + FIELD_SIZE[0] * count * ROW_HEIGHT) * scale * scale
            return wanted, [
                ("the desktop background covers the rest", gray >= 0.95 * below, f"{gray} of {below}"),
                ("the panel is as tall as its contents", 0.9 * wanted <= panel_rows <= 3 * wanted and panel >= 0.3 * panel_rows * width,
                 f"{panel_rows} rows, expected about {wanted} for {count} result rows, {panel} panel pixels"),
                ("the field and the list are in it", 0.6 * field_area <= field <= 1.1 * field_area,
                 f"{field}, expected about {field_area:.0f} at scale {scale:.2f}"),
            ]

        fewest, most = rows if isinstance(rows, tuple) else (rows, rows)
        options = [panel_checks(count) for count in range(fewest, most + 1)]
        # the first count that fits, or when none does, the one closest to the panel on screen
        fits = [found for wanted, found in options if all(passed for _, passed, _ in found)]
        checks += fits[0] if fits else min(options, key=lambda option: abs(option[0] - panel_rows))[1]
    else:
        checks.insert(0, ("the desktop background covers the screen", gray >= 0.95 * total, f"{gray} of {total}"))
    lines = [f"desktop: {width}x{height}"]
    ok = True
    for name, passed, detail in checks:
        lines.append(f"desktop: {'ok  ' if passed else 'FAIL'} {name}: {detail}")
        ok = ok and passed
    return ok, lines


def screendump(qmp_path, work, name):
    """Take a screendump through the monitor and return (width, height, rgb)."""
    ppm = os.path.join(work, name + ".ppm")
    qmp(qmp_path, {"execute": "screendump", "arguments": {"filename": ppm}})
    return read_ppm(ppm)


class Tee:
    def __init__(self, path):
        self.file = open(path, "w", encoding="utf-8", errors="replace")

    def write(self, data):
        self.file.write(data)
        sys.stdout.write(data)

    def flush(self):
        self.file.flush()
        sys.stdout.flush()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("vm", help="the eclipse-vm program from nix build .#vm")
    ap.add_argument("image", help="a raw image without a persist partition yet")
    ap.add_argument("passfile")
    ap.add_argument("--models", help="directory with gguf files for the models subvolume, enables the aura check")
    ap.add_argument("--timeout", type=int, default=600, help="seconds for the whole test")
    ap.add_argument("--aura-timeout", type=int, default=120, help="seconds for aura to load the model")
    ap.add_argument("--answer-timeout", type=int, default=240, help="seconds for aura's answer to reach the field")
    ap.add_argument("--log", default="serial.log")
    ap.add_argument("--memory", default="4096")
    ap.add_argument("--qmp", help="unix socket for the qemu monitor")
    ap.add_argument("--splash", help="take a screendump at the luks prompt, check it, save it as this png")
    ap.add_argument("--desktop", help="take a screendump of the session, check it, save it as this png")
    ap.add_argument("--desktop-timeout", type=int, default=60, help="seconds for umbra to paint its first frame")
    ap.add_argument("--corona", action="store_true", help="expect corona's panel on the desktop")
    args = ap.parse_args()
    with open(args.passfile, encoding="utf-8") as f:
        passphrase = f.read()

    work = tempfile.mkdtemp(prefix="eclipse-boot-")
    if (args.splash or args.desktop) and not args.qmp:
        args.qmp = os.path.join(work, "qmp.sock")

    # the app picks kvm or tcg and the firmware. what follows its options replaces its defaults.
    # the gpu is virtio: the firmware draws the splash on it and umbra opens it as a drm device
    cmd = [
        os.path.abspath(args.vm),
        "--image", os.path.abspath(args.image),
        "--persist", os.path.abspath(args.passfile),
        "-smp", "2",
        "-m", args.memory,
        "-device", "virtio-vga",
        "-display", "none",
        "-monitor", "none",
        "-serial", "stdio",
        "-no-reboot",
    ]
    if args.models:
        cmd[5:5] = ["--models", os.path.abspath(args.models)]
    if args.qmp:
        cmd += ["-qmp", f"unix:{args.qmp},server,nowait"]
    print("boot-test: " + " ".join(cmd), flush=True)

    start = time.monotonic()
    deadline = start + args.timeout
    child = pexpect.spawn(cmd[0], cmd[1:], encoding="utf-8", codec_errors="replace", dimensions=(40, 160))
    child.logfile_read = Tee(args.log)

    def since():
        return f"{time.monotonic() - start:.0f}s"

    def fail(why):
        print(f"\nboot-test: FAILED after {since()}: {why}", flush=True)
        print(f"boot-test: the serial log is in {args.log}", flush=True)
        child.terminate(force=True)
        sys.exit(1)

    def expect(patterns, what):
        try:
            return child.expect(patterns, timeout=max(1, deadline - time.monotonic()))
        except pexpect.TIMEOUT:
            fail(f"timed out waiting for {what}")
        except pexpect.EOF:
            fail(f"qemu exited while waiting for {what}")

    def ok(what):
        print(f"\nboot-test: {what} at {since()}", flush=True)

    def run(command, what):
        """Run one command line in the serial shell. Returns its exit status and what it printed,
        without escape codes and carriage returns."""
        child.send(command + "\r")
        expect([COMMAND_START], f"the shell to start {what}")
        expect([COMMAND_END], what)
        status = int(child.match.group(1))
        return status, ESCAPES.sub("", child.before).replace("\r", "")

    # 1. the luks prompt, answered over serial. a second prompt means the passphrase was refused.
    expect([PASSPHRASE], "the luks passphrase prompt")
    ok("passphrase prompt")

    # 1a. the splash. cryptsetup waits for us, so the screen is stable
    if args.splash:
        time.sleep(3)
        try:
            width, height, rgb = screendump(args.qmp, work, "splash")
        except (OSError, RuntimeError) as e:
            fail(f"screendump: {e}")
        write_png(args.splash, width, height, rgb)
        good, lines = check_splash(width, height, rgb)
        print("\nboot-test: " + "\nboot-test: ".join(lines), flush=True)
        if not good:
            fail(f"the splash is not on screen, see {args.splash}")
        ok("splash")

    child.send(passphrase + "\r")
    for attempt in range(3):
        if expect([PROMPT, PASSPHRASE], "the autologin shell") == 0:
            break
        if attempt == 2:
            fail("the passphrase was refused three times")
        print("\nboot-test: passphrase prompt again, retrying", flush=True)
        child.send(passphrase + "\r")
    ok("shell")

    # 2. the system is ours
    child.send("eclipse --version\r")
    expect([r"eclipse \d+\.\d+\.\d+"], "eclipse --version output")
    version = child.after
    expect([PROMPT], "the prompt")

    child.send("echo phase=(cat /etc/eclipse/phase)\r")
    expect([r"phase=(\d+)\s"], "the phase")
    phase = child.match.group(1)
    expect([PROMPT], "the prompt")
    if phase != "0":
        fail(f"phase is {phase}, expected 0")

    child.send("findmnt -no SOURCE,FSTYPE /home\r")
    expect([r"/dev/mapper/persist\S*\s+btrfs"], "/home on persist")
    expect([PROMPT], "the prompt")
    ok(f"{version.strip()}, phase {phase}, home on persist")

    # 2a. the default apps are on the path, firefox has its policies, zed got its settings with
    # telemetry off, and podman runs rootless in the owner's ranges
    apps = ["firefox", "zeditor", "hx", "zellij", "ghostty", "fish", "podman", "docker"]
    _, output = run("for app in " + " ".join(apps) + "; command -q $app; or echo missing=$app; end; echo apps-done",
                    "the default apps on the path")
    missing = re.findall(r"missing=(\S+)", output)
    if missing or "apps-done" not in output:
        fail(f"not on the path: {' '.join(missing) or repr(output.strip())}")
    status, _ = run("grep -q DisableTelemetry /etc/firefox/policies/policies.json", "firefox's policies")
    if status != 0:
        fail("firefox has no policies file that turns telemetry off")
    status, output = run("cat ~/.config/zed/settings.json", "zed's settings")
    if status != 0 or '"metrics":false' not in output:
        fail(f"zed's settings do not turn telemetry off: {output.strip()!r}")
    status, output = run("podman info --format 'rootless={{.Host.Security.Rootless}}'", "podman info")
    if status != 0 or "rootless=true" not in output:
        fail(f"podman does not run rootless for the owner: {output.strip()[-600:]!r}")
    ok(f"{', '.join(apps)} on the path, firefox policies, zed settings, podman rootless")

    # 3. syzygy: the profile it wrote into @hosts, and the same answers on the system bus.
    # fish puts a bare \r before a command's output, so these anchor on the whitespace after the
    # value, not before it
    child.send("systemctl is-active syzygy\r")
    expect([r"(?<![\w-])(active|inactive|failed|activating)\s"], "the syzygy unit state")
    state = child.match.group(1)
    expect([PROMPT], "the prompt")
    if state != "active":
        fail(f"syzygy.service is {state}, expected active")

    hosts = "/var/lib/eclipse/hosts"
    child.send(f"cat {hosts}/current\r")
    expect([r"(?<![0-9a-f])([0-9a-f]{64})\s"], "the fingerprint in hosts/current")
    fingerprint = child.match.group(1)
    expect([PROMPT], "the prompt")

    # the profile is a delta over the defaults, so a virtual machine writes eleven lines with a
    # value on them: the four that say which machine this is, the two settings a qemu box does
    # not share with the defaults, and five for its one output. the class, the chassis, the
    # vendor and the scale are all the defaults, so they are not in the file at all.
    profile = f"{hosts}/{fingerprint}.toml"
    child.send(f"cat {profile}\r")
    expect([rf'fingerprint = "{fingerprint}"'], "the fingerprint in the profile")
    expect([r'host = "([^"]*)"'], "the machine name in the profile")
    machine = child.match.group(1)
    expect([r'gpu_path = "(\w+)"'], "the gpu path in the profile")
    file_gpu_path = child.match.group(1)
    expect([r'ai_tier = "(\w+)"'], "the ai tier in the profile")
    file_ai_tier = child.match.group(1)
    expect([r'connector = "([\w-]+)"'], "the output in the profile")
    file_connector = child.match.group(1)
    # a number with nothing after it matches as soon as its first digits arrive
    expect([r"width = (\d+)\s"], "the output width in the profile")
    file_width = child.match.group(1)
    expect([r"height = (\d+)\s"], "the output height in the profile")
    file_height = child.match.group(1)
    expect([PROMPT], "the prompt")

    def count(what, command):
        child.send(f"echo {what}=({command})\r")
        expect([rf"{what}=(\d+)\s"], f"the {what} count")
        value = int(child.match.group(1))
        expect([PROMPT], "the prompt")
        return value

    keys = count("keys", f"grep -c ' = ' {profile}")
    if keys != 11:
        fail(f"the profile has {keys} lines with a value on them, expected 11, not a delta")
    if count("class", f"grep -c '^class = ' {profile}") != 0:
        fail("the profile writes the class, which is the default and belongs to no machine")
    if file_gpu_path != "none":
        fail(f"the profile says gpu path {file_gpu_path}, expected none for a virtual machine")
    if file_ai_tier != "small":
        fail(f"the profile says ai tier {file_ai_tier}, expected small for a 4 GB machine")
    if (file_width, file_height) != ("1280", "800"):
        fail(f"the profile says the output is {file_width}x{file_height}, expected 1280x800")
    if count("scale", f"grep -c '^scale = ' {profile}") != 0:
        fail("the profile writes a scale, but a 32 by 20 cm 1280x800 panel is about 102 dpi")

    # the bus. the interface is read only, so the owner reads it without sudo
    bus, obj = "dev.eclipse.Syzygy", "/dev/eclipse/Syzygy"
    if count("bus", f"busctl --system list --no-pager --no-legend | grep -c '^{bus}'") != 1:
        fail(f"{bus} is not on the system bus")

    def prop(name, pattern):
        child.send(f"busctl --system get-property {bus} {obj} {bus} {name}\r")
        expect([pattern], f"the {name} property")
        match = child.match
        expect([PROMPT], "the prompt")
        return match

    if prop("Fingerprint", r's "([0-9a-f]{64})"').group(1) != fingerprint:
        fail("the fingerprint on the bus is not the one in hosts/current")
    klass = prop("Class", r's "(\w+)"').group(1)
    if klass != "borrowed":
        fail(f"the bus says class {klass}, expected the default borrowed")
    gpu_path = prop("GpuPath", r's "(\w+)"').group(1)
    if gpu_path != file_gpu_path:
        fail(f"the bus says gpu path {gpu_path}, the profile says {file_gpu_path}")
    ai_tier = prop("AiTier", r's "(\w+)"').group(1)
    if ai_tier != file_ai_tier:
        fail(f"the bus says ai tier {ai_tier}, the profile says {file_ai_tier}")

    # one virtual output. qemu gives it an edid, so the mode and the size are real; at 32 by 20
    # centimetres 1280x800 is about 102 dpi, which is under the line, so the scale is 1
    displays = prop("Displays", r"a\(suuu\) (\d+)([^\r\n]*)\r*\n")
    if displays.group(1) != "1":
        fail(f"the bus lists {displays.group(1)} outputs, expected 1:{displays.group(2)}")
    output = re.match(r'\s*"([\w-]+)" (\d+) (\d+) (\d+)', displays.group(2))
    if not output:
        fail(f"the output on the bus does not read as one:{displays.group(2)}")
    if output.group(1) != file_connector:
        fail(f"the bus calls the output {output.group(1)}, the profile calls it {file_connector}")
    if output.group(2, 3, 4) != (file_width, file_height, "1"):
        fail(f"the output on the bus is {output.group(2, 3, 4)}, the profile says "
             f"{file_width}x{file_height} at scale 1")
    ok(
        f"host profile {fingerprint[:12]}, {machine}, class {klass}, gpu {gpu_path}, "
        f"ai tier {ai_tier}, output {output.group(1)} scale {output.group(4)}, on the bus"
    )

    # 3a. `eclipse host` reads the same properties off the bus and prints a row for each
    host_rows = {
        "Fingerprint": fingerprint,
        "Class": klass,
        "Display": f"{output.group(1)}, {output.group(2)}x{output.group(3)}, scale {output.group(4)}",
        "GPU path": gpu_path,
        "AI tier": ai_tier,
    }
    status, printed = run("eclipse host", "eclipse host")
    printed = without_console(printed)
    print(f"\nboot-test: eclipse host printed:\n{printed}", flush=True)
    if status != 0:
        fail(f"eclipse host exited with {status}")
    rows = dict(re.findall(r"^(Fingerprint|Class|Display|GPU path|AI tier):[ \t]+(.*?)[ \t]*$", printed, re.M))
    for label, value in host_rows.items():
        if rows.get(label) != value:
            fail(f"eclipse host says {label} {rows.get(label)!r}, the bus says {value!r}")
    ok(f"eclipse host printed fingerprint {fingerprint[:12]} and ai tier {ai_tier}, as the bus did")

    # 4. aura. aurad reads the tier from syzygy, picks a model that is on the drive, runs
    # llama-server as its child and answers on the system bus. the name is there before the model
    # has loaded, so poll the State property
    if args.models:
        _, output = run("systemctl is-active aura", "the aura unit state")
        state = re.search(r"(?<![\w-])(active|inactive|failed|activating)\s", output)
        state = state.group(1) if state else output.strip()
        if state not in ("active", "activating"):
            fail(f"aura.service is {state}, expected active")

        aura, aura_path = "dev.eclipse.Aura", "/dev/eclipse/Aura"

        def aura_prop(name):
            """A string property of aura's, or None when the bus gave no answer."""
            status, output = run(f"busctl --system get-property {aura} {aura_path} {aura} {name}",
                                 f"aura's {name} property")
            value = re.search(r's "([^"\n]*)"', output)
            return value.group(1) if status == 0 and value else None

        aura_deadline = time.monotonic() + args.aura_timeout
        while True:
            aura_state = aura_prop("State")
            if aura_state == "ready":
                break
            if aura_state in ("none", "failed") or time.monotonic() > aura_deadline:
                why = aura_prop("Error")
                fail(f"aura is {aura_state or 'not on the bus'} after {since()}: {why}")
            time.sleep(5)

        # the only model on the drive is the one in --models, and the manifest says which id it is
        manifest_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "models", "manifest.toml")
        with open(manifest_path, "rb") as f:
            chat = tomllib.load(f)["chat"]
        on_drive = set(os.listdir(args.models))
        wanted_model = [m["id"] for m in chat if m["file"] in on_drive]
        aura_tier = aura_prop("Tier")
        aura_model = aura_prop("Model")
        ok(f"aura loaded {aura_model} for tier {aura_tier}")
        if aura_tier != ai_tier:
            fail(f"aura says the tier is {aura_tier}, syzygy says {ai_tier}")
        if [aura_model] != wanted_model:
            fail(f"aura runs {aura_model}, the models on the drive are {wanted_model}")

        # the local api that other programs use is the same server
        api = "localhost:11434"
        _, output = run(f"curl -s -o /dev/null -w 'health=%{{http_code}}\\n' {api}/health", "the aura health code")
        code = re.search(r"health=(\d{3})", output)
        if not code or code.group(1) != "200":
            fail(f"the local api says {output.strip()!r} on /health, but aura says the model is ready")

        body = '{"prompt":"The capital of France is","n_predict":4}'
        _, output = run(f"curl -s {api}/completion -d '{body}'", "a completion")
        content = re.search(r'"content":"([^"]+)"', output)
        if not content:
            fail(f"the local api gave no completion: {output.strip()!r}")
        ok(f"the local api completed {content.group(1)!r}")

        # a web page cannot use it. a browser sends an Origin header with anything a page asks
        # for, and a page that points its own name at 127.0.0.1 sends that name as the Host. the
        # model's socket behind the api is aura's alone
        def api_code(options, what):
            _, output = run(f"curl -s -o /dev/null -w 'code=%{{http_code}}\\n' {options}", what)
            code = re.search(r"code=(\d{3})", output)
            return code.group(1) if code else output.strip()

        for header, what in [
            ("Origin: https://example.com", "a completion a web page asked for"),
            ("Host: example.com:11434", "a completion for a name that is not the loopback address"),
        ]:
            code = api_code(f"-H '{header}' {api}/completion -d '{body}'", what)
            if code != "403":
                fail(f"the local api answered {what} with {code}, expected 403")
        code = api_code("--unix-socket /run/aura/llama.sock http://localhost/health", "llama-server's socket")
        if code != "000":
            fail(f"the owner reached llama-server's socket without the local api, it said {code}")
        ok("the local api refuses web pages, and only aura opens the model's socket")

        # and the question over the bus, as the owner, no sudo. Ask returns a kind and a text, and
        # busctl's json keeps both on one line with their quotes escaped
        _, output = run(f"busctl --system --json=short --timeout=240 call {aura} {aura_path} {aura} Ask s '{QUESTION}'",
                        "aura's answer on the bus")
        reply = re.search(r'"type":"ss","data":\["(\w+)","((?:[^"\\]|\\.)+)"\]\}', output)
        if not reply:
            fail(f"Ask on the bus gave no answer: {output.strip()!r}")
        kind, answer = reply.group(1), json.loads('"' + reply.group(2) + '"')
        if kind != "answer":
            fail(f"Ask on the bus said {kind} {answer!r} to {QUESTION!r}, expected an answer")
        ok(f"aura answered {QUESTION!r} on the bus with {answer!r}")

        # 4a. the same question through `eclipse ai`, which prints the answer, and `eclipse ai`
        # without one, which prints the properties the bus just gave
        status, printed = run(f'eclipse ai "{QUESTION}"', "aura's answer through eclipse ai")
        printed = without_console(printed)
        print(f'\nboot-test: eclipse ai "{QUESTION}" printed:\n{printed}', flush=True)
        if status != 0 or "paris" not in printed.lower():
            fail(f"eclipse ai exited with {status} and did not say Paris")
        ok(f"eclipse ai answered {printed!r}")

        status, printed = run("eclipse ai", "aura's state through eclipse ai")
        printed = without_console(printed)
        print(f"\nboot-test: eclipse ai printed:\n{printed}", flush=True)
        rows = dict(re.findall(r"^(State|Model|Tier):[ \t]+(.*?)[ \t]*$", printed, re.M))
        wanted = {"State": "ready", "Model": aura_model, "Tier": aura_tier}
        if status != 0 or rows != wanted:
            fail(f"eclipse ai says {rows}, the bus says {wanted}")
        ok("eclipse ai printed the state, model and tier the bus gave")

    # 4b. `eclipse doctor`: no check fails, and syzygy and aura each have a row. with the model
    # loaded, aura's row has to pass
    status, printed = run("eclipse doctor", "eclipse doctor")
    printed = without_console(printed)
    print(f"\nboot-test: eclipse doctor printed:\n{printed}", flush=True)
    rows = dict(re.findall(r"^(Syzygy|Aura|Persist|Memory|CPU|IO|System image)[ \t]+(Passed|Warning|Failed)[ \t]",
                           printed, re.M))
    if status != 0:
        fail(f"eclipse doctor exited with {status}")
    if rows.get("Syzygy") != "Passed":
        fail(f"eclipse doctor says Syzygy {rows.get('Syzygy')}, expected Passed")
    if "Aura" not in rows or (args.models and rows["Aura"] != "Passed"):
        fail(f"eclipse doctor says Aura {rows.get('Aura')}, expected Passed")
    ok("eclipse doctor: " + ", ".join(f"{name} {verdict}" for name, verdict in rows.items()))

    # 5. the desktop. greetd runs umbra on tty1 as the owner. umbra needs a moment to open the gpu
    # and paint its first frame, so the screendump is retried until it shows the background
    if args.desktop:
        child.send("systemctl is-active greetd\r")
        expect([r"(?<![\w-])(active|inactive|failed|activating)\s"], "the greetd unit state")
        state = child.match.group(1)
        expect([PROMPT], "the prompt")
        if state != "active":
            fail(f"greetd.service is {state}, expected active")

        def look(what, png, seconds, **shape):
            """Screendump until the panel has the shape we asked for, or give up and save it."""
            deadline = time.monotonic() + seconds
            while True:
                try:
                    width, height, rgb = screendump(args.qmp, work, "desktop")
                except (OSError, RuntimeError) as e:
                    fail(f"screendump: {e}")
                good, lines = check_desktop(width, height, rgb, corona=args.corona, **shape)
                if good or time.monotonic() > deadline:
                    break
                time.sleep(2 if shape else 5)
            write_png(png, width, height, rgb)
            print("\nboot-test: " + "\nboot-test: ".join(lines), flush=True)
            if not good:
                fail(f"{what} is not on screen, see {png}")
            ok(what)

        look("desktop", args.desktop, args.desktop_timeout)

        # 5a. the compositor knows corona's surface too. the session's ipc socket is in the
        # owner's runtime directory, the serial shell runs as the owner
        if args.corona:
            _, output = run("set -x NIRI_SOCKET (ls -t /run/user/(id -u)/niri.wayland-1.*.sock | head -n1); umbra msg --json layers",
                            "umbra's layer surfaces")
            if not re.search(r'"namespace":\s*"corona"', output):
                fail("umbra lists no layer surface named corona")
            ok("corona panel")

            # 5b. the field takes a line from the terminal, over the socket in the session's
            # runtime directory, and what the line printed lands in the list under it
            stem, extension = os.path.splitext(args.desktop)
            run("set -x XDG_RUNTIME_DIR /run/user/(id -u)", "the runtime directory")
            run(f'corona --enter "{RESULT_LINE}"', "a pipeline typed into the field")
            look("the result list", f"{stem}-corona{extension}", 20, rows=RESULT_ROWS)

            run(f'corona --enter "{ERROR_LINE}"', "a wrong command typed into the field")
            look("the error line", f"{stem}-corona-error{extension}", 20, rows=0, error=True)

            run("corona --escape", "escape in the field")
            look("the panel back at the field", f"{stem}-corona-empty{extension}", 20, rows=0)

            # 5c. a question for aura, from the terminal first, which prints the answer here, and
            # then typed into the field. the answer is as many rows as the model makes it, so the
            # list is only expected to have at least one
            if args.models:
                status, output = run(f'corona --do "{QUESTION}"', "aura's answer through corona")
                # the journal's lines on the console land in the output too
                said = "\n".join(line for line in output.splitlines()
                                 if line.strip() and not re.match(r"\s*\[\s*\d+\.\d+\] ", line))
                if status != 0 or not said:
                    fail(f"corona --do could not ask aura: {output.strip()!r}")
                ok(f"corona asked aura and printed {said!r}")

                run(f'corona --enter "{QUESTION}"', "a question typed into the field")
                look("the answer under the field", f"{stem}-corona-answer{extension}", args.answer_timeout,
                     rows=(1, LIST_ROWS))

    # 6. down
    child.send("sudo systemctl poweroff\r")
    try:
        child.expect(pexpect.EOF, timeout=90)
    except pexpect.TIMEOUT:
        print("\nboot-test: poweroff did not end qemu, killing it", flush=True)
        child.terminate(force=True)
    print(f"\nboot-test: PASSED in {since()}", flush=True)


if __name__ == "__main__":
    main()
