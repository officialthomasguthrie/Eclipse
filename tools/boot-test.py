#!/usr/bin/env python3
"""Boots an image through the flake's vm app and checks that the system comes up. The boot job in ci
runs this.

Usage: boot-test.py <eclipse-vm> <image.raw> <passfile> [--timeout 300] [--log serial.log] [--splash splash.png]

<eclipse-vm> is the program from `nix build .#vm` (result/bin/eclipse-vm). It adds the persist
partition to the image with the passphrase from the passfile (persist-image.sh, through sudo), then
boots the image as an nvme drive. Everything goes through the serial console: the luks prompt, the
autologin shell, a few commands. The serial output is printed as it arrives and kept in the log file.

With --splash the test also takes a screendump through the qemu monitor while the luks prompt is up
and checks that the Totality splash is on screen: the light disc and the black disc from
nix/totality/plymouth against the gray background. The dump is saved as a png.
"""

import argparse
import glob
import json
import math
import os
import socket
import struct
import sys
import tempfile
import time
import zlib

import pexpect

# the fish prompt is user@host with colour codes in between
PROMPT = r"eclipse(\x1b\[[0-9;]*m)*@(\x1b\[[0-9;]*m)*eclipse"
PASSPHRASE = r"(?i)passphrase[^\r\n]*:"


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
    ap.add_argument("--timeout", type=int, default=300, help="seconds for the whole boot")
    ap.add_argument("--log", default="serial.log")
    ap.add_argument("--memory", default="4096")
    ap.add_argument("--qmp", help="unix socket for the qemu monitor")
    ap.add_argument("--splash", help="take a screendump at the luks prompt, check it, save it as this png")
    args = ap.parse_args()
    with open(args.passfile, encoding="utf-8") as f:
        passphrase = f.read()

    work = tempfile.mkdtemp(prefix="eclipse-boot-")
    if args.splash and not args.qmp:
        args.qmp = os.path.join(work, "qmp.sock")

    # the app picks kvm or tcg and the firmware. what follows its options replaces its defaults
    cmd = [
        os.path.abspath(args.vm),
        "--image", os.path.abspath(args.image),
        "--persist", os.path.abspath(args.passfile),
        "-smp", "2",
        "-m", args.memory,
        "-vga", "std",
        "-display", "none",
        "-monitor", "none",
        "-serial", "stdio",
        "-no-reboot",
    ]
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

    # 1. the luks prompt, answered over serial. a second prompt means the passphrase was refused.
    expect([PASSPHRASE], "the luks passphrase prompt")
    ok("passphrase prompt")

    # 1a. the splash. cryptsetup waits for us, so the screen is stable
    if args.splash:
        time.sleep(3)
        ppm = os.path.join(work, "splash.ppm")
        try:
            qmp(args.qmp, {"execute": "screendump", "arguments": {"filename": ppm}})
            width, height, rgb = read_ppm(ppm)
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

    # 3. down
    child.send("sudo systemctl poweroff\r")
    try:
        child.expect(pexpect.EOF, timeout=90)
    except pexpect.TIMEOUT:
        print("\nboot-test: poweroff did not end qemu, killing it", flush=True)
        child.terminate(force=True)
    print(f"\nboot-test: PASSED in {since()}", flush=True)


if __name__ == "__main__":
    main()
