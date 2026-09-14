#!/usr/bin/env python3
"""Boots an image through the flake's vm app and checks that the system comes up. The boot job in ci
runs this.

Usage: boot-test.py <eclipse-vm> <image> <passfile> [--models dir] [--exchange size] [--timeout 600]
       [--log serial.log] [--splash splash.png] [--desktop desktop.png] [--corona] [--updates updates.img]
       [--backup backup.img] [--clone clone.img] [--first-boot]

With --first-boot eclipse-flash writes the drive without persist, the way it writes one on macOS and
Windows, and the drive makes persist when it first starts. The test answers its questions over serial: a
passphrase that is too short and two that differ are asked for again, then the passphrase from the
passfile goes in twice. The drive goes on to the shell without asking again, and gets the checks below
up to the drive's own: the slots, luks2 with argon2id, the subvolumes, the owner's home and the exchange
partition. Persist has one key slot and the system runs with the machine id in @var. After a reboot the
drive asks systemd-cryptsetup's question, not the first boot's, and opens the same persist with the same
passphrase: the same uuids, machine id, key slots and partitions. The test ends there.

<eclipse-vm> is the program from `nix build .#vm` (result/bin/eclipse-vm). It writes the image, .raw or
.raw.zst, onto a drive in a file with eclipse-flash (through sudo), with the passphrase from the passfile
for persist, then boots the drive as an nvme drive. Everything goes through the serial console: the luks prompt,
the autologin shell, a few commands, the default apps on the path, the a/b slots, the host profile
syzygy wrote and what `eclipse host` and `eclipse doctor` print. The serial output is printed as it
arrives and kept in the log file.

Timeline: the test takes a snapshot of home with `eclipse snapshot take`, changes one file and
deletes another, finds the snapshot through `eclipse snapshot` and on the bus, and restores both from
it. The deleted file comes back as the owner's; the changed one stays as it is without --replace and
is the copy from the snapshot with it. The hourly timer's service runs once and adds a snapshot. Then
`vault prune` with one hour, one day and two weeks drops the snapshots named by hand for January that
fall past those limits and keeps the one that does not.

Backup: with --backup the vm gets another drive, an empty ext4 file system labelled backup. The test
mounts it, chooses a folder on it with `sudo vault target`, which prints the password, and unmounts it
again. `eclipse backup now` backs up home; vault mounts the disk by its uuid by itself. One file is
changed and another deleted, and both come back from the backup through `eclipse backup restore` the
way they do from a snapshot. Then the test mounts the disk again: rustic refuses the repository with a
wrong password and opens it with the printed one, and grep finds the file's text in none of its files.

Clone: with --clone the vm gets an empty scsi disk that says it is removable. Last of all the test
writes a file to home and runs `sudo eclipse clone`. Vault refuses the drive the system runs from, the
backup drive, which is not removable, and a serial that is not the disk's, and writes nothing. Then it
clones onto the removable disk with a passphrase of its own. Slot a of the clone holds the running
version under the running slot's uuids and its store matches the usrhash, slot b is empty, the first
drive's passphrase does not open the clone's persist, and the first drive's header over the clone's
data reads as no file system, so the two volume keys differ. After the poweroff qemu starts again with
only the clone. Its luks prompt refuses the first drive's passphrase and takes the clone's, the file is
in home, and the clone boots the version it was made from, from its own esp and slot a, with a machine
id of its own and none of the first drive's snapshots.

The drive: the vm app writes it from the image into a sparse file with eclipse-flash, with an exchange
partition when --exchange gives its size. Persist has to be luks2 with argon2id, the settings a person
gets, with every subvolume and the owner's home, and the exchange partition an exfat labelled EXCHANGE
of that size. A clone of the drive gets an exchange partition of the same size.

The slots: systemd-boot started the uki with a boot counter in its name, the boot reached
boot-complete.target and the counter is gone, systemd-sysupdate lists the running version as
installed, /usr runs from slot a, and slot b's two partitions are there and empty.

With --updates the vm gets a second drive, an ext4 file system labelled updates with the update files
of two newer versions: next (nix build .#update) and broken (nix build .#broken-update), a version
after next whose boot check always fails. After the other checks the test mounts next where
systemd-sysupdate reads updates and installs that version: its store and verity partitions in slot b
with the uuids from the file names, its uki on the esp with three tries. The vm reboots and the slots
are checked again for the new version: systemd-boot started its uki, the boot was marked good,
sysupdate lists both versions with the new one current, and /usr runs from slot b.

Then the rollback. sysupdate installs broken over the oldest version, in slot a, and the vm boots it
three times. Each of those boots comes up to the shell, the check fails, nothing marks the boot good,
and systemd-boot has taken one more try off its uki: +2-1, +1-2, +0-3. The fourth boot runs next from
slot b again, and sysupdate still lists broken as installed.

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
import functools
import glob
import http.server
import json
import math
import os
import re
import socket
import struct
import sys
import tempfile
import threading
import time
import tomllib
import zlib

import pexpect

# the fish prompt is user@host with colour codes in between
PROMPT = r"eclipse(\x1b\[[0-9;]*m)*@(\x1b\[[0-9;]*m)*eclipse"
PASSPHRASE = r"(?i)passphrase[^\r\n]*:"
# what vault-first-boot asks on a drive written without persist
CHOOSE = r"Choose a passphrase"
AGAIN = r"Type the passphrase again"
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


def version_key(version):
    """Sorts versions like 0.2.0 and 0.10.0 by their numbers."""
    return tuple(int(part) for part in version.split("."))


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
# the console, from nix/modules/umbra.nix: ghostty's background, the height the window rule gives
# the window in logical pixels, and the app id the bind shows and hides
CONSOLE = (40, 40, 40)
CONSOLE_HEIGHT = 400
CONSOLE_APP_ID = "dev.eclipse.Console"
# the lock screen, from crates/umbra-lock/src/draw.rs: its gray, the inside of the field, the ring
# around it, the sentence for a refused password, and the field's size in logical pixels
LOCK = (30, 30, 30)
LOCK_FIELD = (46, 46, 46)
ACCENT = (120, 174, 237)
REFUSED = (224, 109, 109)
LOCK_FIELD_SIZE = (280, 32)
LOCK_RING = 2
# the owner's password from nix/profiles/base.nix, and one that is not it
PASSWORD = "eclipse"
WRONG_PASSWORD = "wrongpassword"
# the passphrase the test gives the clone's persist, not the first drive's
CLONE_PASSPHRASE = "clone-test-5213"
# gpt partition types from the discoverable partitions specification: the esp, /usr on x86-64 and
# its verity data. slot a and slot b each have a store and a verity partition
ESP_TYPE = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"
USR_TYPE = "8484680c-9521-48c6-9c11-b0720656f69e"
USR_VERITY_TYPE = "77ff5f63-e7b6-4633-acf4-1565b864c0e6"
# where the transfers in nix/image/ab-sysupdate.nix read a new version from, and the tries they give
# its uki
UPDATES = "/var/lib/eclipse/updates"
TRIES = 3
# where the test mounts the updates drive, and the unit that keeps the broken version from being good
UPDATES_DRIVE = "/run/updates-drive"
NEVER_GOOD = "never-good.service"


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


def check_console(width, height, rgb):
    """Find the console in a screendump: corona's panel along the top, under it a run of rows that
    are mostly the console's background, and the desktop under that. The terminal shows one line
    of text, the prompt, and nothing a shell printed before it. Returns (ok, lines to print)."""
    gray = black = 0
    panel_rows = 0
    console_top, console_rows, console_width = -1, 0, 0
    text_rows = []
    for y in range(height):
        row = y * width * 3
        row_panel = row_console = row_text = 0
        for x in range(width):
            px = rgb[row + x * 3 : row + x * 3 + 3]
            if near(px, CONSOLE, 1):
                row_console += 1
                continue
            # what is not close to the console's gray is text. the desktop and the panel grays are
            # close to it. lines start at the left edge, the pointer sits in the middle of the screen
            if x < width / 4 and not near(px, CONSOLE, 12):
                row_text += 1
            if near(px, DESKTOP, 3):
                gray += 1
            elif near(px, MOON, 8):
                black += 1
            elif near(px, PANEL, 3) or near(px, FIELD, 3):
                row_panel += 1
        if row_panel > width / 2 and panel_rows == y:
            panel_rows += 1
        # the first run of rows that are mostly the console's gray. a line of text in the terminal
        # covers only some of a row
        if row_console > width / 2 and (console_top < 0 or console_top + console_rows == y):
            if console_top < 0:
                console_top = y
            console_rows += 1
            console_width = max(console_width, row_console)
            if row_text:
                text_rows.append(y)
    total = width * height
    scale = panel_rows / PANEL_HEIGHT if panel_rows else 1
    wanted = CONSOLE_HEIGHT * scale
    below = total - (panel_rows + console_rows) * width
    # a line of DejaVu Sans Mono 11 is 17 rows, the prompt starts a few rows under the window's top
    one_line = bool(text_rows) and text_rows[0] - console_top <= 12 * scale and text_rows[-1] - text_rows[0] < 24 * scale
    checks = [
        ("no console black", black <= 0.02 * total, f"{black} of {total}"),
        ("the panel is along the top", 0.9 * PANEL_HEIGHT <= panel_rows <= 3 * PANEL_HEIGHT, f"{panel_rows} rows"),
        ("the console starts under the panel", console_top >= 0 and panel_rows <= console_top <= panel_rows + 16 * scale,
         f"first row {console_top}, the panel ends at {panel_rows}"),
        ("the console is as tall as the window rule says", 0.95 * wanted <= console_rows <= 1.05 * wanted,
         f"{console_rows} rows, expected about {wanted:.0f}"),
        ("the console is as wide as the screen", console_width >= 0.9 * width, f"{console_width} of {width} in its widest row"),
        ("the desktop background covers the rest", gray >= 0.9 * below, f"{gray} of {below}"),
        ("the terminal shows only its prompt", one_line,
         f"text in rows {text_rows[0]} to {text_rows[-1]}, the console starts at {console_top}" if text_rows else "no text"),
    ]
    lines = [f"console: {width}x{height}"]
    ok = True
    for name, passed, detail in checks:
        lines.append(f"console: {'ok  ' if passed else 'FAIL'} {name}: {detail}")
        ok = ok and passed
    return ok, lines


def check_lock(width, height, rgb, refused=False):
    """Find the lock screen in a screendump: its gray over the whole screen, the field in the middle
    with the blue ring around it, and none of the desktop, corona's panel or the console. With
    refused, the red sentence is under the field, without it there is none. Returns (ok, lines to
    print)."""
    ground = desktop = console = ring = red = field = 0
    left, top, right, bottom = width, height, -1, -1
    for y in range(height):
        row = y * width * 3
        row_field, row_left, row_right = 0, width, -1
        for x in range(width):
            px = rgb[row + x * 3 : row + x * 3 + 3]
            if near(px, LOCK, 2):
                ground += 1
            elif near(px, LOCK_FIELD, 2):
                row_field += 1
                row_left, row_right = min(row_left, x), max(row_right, x)
            elif near(px, DESKTOP, 1):
                desktop += 1
            elif near(px, CONSOLE, 1):
                console += 1
            elif near(px, ACCENT, 24):
                ring += 1
            elif near(px, REFUSED, 24):
                red += 1
        # a row of the field has a long run of its gray. the edges of the text above and under it
        # pass through that gray in a few pixels
        if row_field >= 100:
            field += row_field
            left, right = min(left, row_left), max(right, row_right)
            top, bottom = min(top, y), max(bottom, y)
    total = width * height
    # the inside of the field is the field less its ring. corona's field would stretch the box to
    # the top of the screen
    inner = (LOCK_FIELD_SIZE[0] - 2 * LOCK_RING, LOCK_FIELD_SIZE[1] - 2 * LOCK_RING)
    box = (right - left + 1, bottom - top + 1) if right >= 0 else (0, 0)
    scale = max(1, round(box[0] / inner[0]))
    ring_wanted = 2 * (LOCK_FIELD_SIZE[0] + LOCK_FIELD_SIZE[1]) * LOCK_RING * scale * scale
    checks = [
        ("the lock screen's gray covers the screen", ground >= 0.95 * total, f"{ground} of {total}"),
        ("nothing of the desktop", desktop <= 0.002 * total, f"{desktop} desktop gray pixels"),
        ("nothing of the console", console <= 0.002 * total, f"{console} console gray pixels"),
        ("the field is in the middle", right >= 0 and abs((left + right) / 2 - width / 2) <= 4 * scale
         and abs((top + bottom) / 2 - height / 2) <= 4 * scale,
         f"from {left},{top} to {right},{bottom} on {width}x{height}"),
        ("the field is as big as the lock screen draws it", abs(box[0] - inner[0] * scale) <= 4 * scale
         and abs(box[1] - inner[1] * scale) <= 4 * scale and field >= 0.8 * box[0] * box[1],
         f"{box[0]}x{box[1]}, {field} field pixels, expected about {inner[0] * scale}x{inner[1] * scale}"),
        ("the blue ring is around it", 0.6 * ring_wanted <= ring <= 1.5 * ring_wanted,
         f"{ring}, expected about {ring_wanted}"),
    ]
    if refused:
        checks.append(("the sentence under the field says the password was refused", red >= 100, f"{red} red pixels"))
    else:
        checks.append(("no sentence about a refused password", red <= 20, f"{red} red pixels"))
    lines = [f"lock: {width}x{height}"]
    ok = True
    for name, passed, detail in checks:
        lines.append(f"lock: {'ok  ' if passed else 'FAIL'} {name}: {detail}")
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
    ap.add_argument("image", help="the image, .raw or .raw.zst, that eclipse-flash writes onto the drive the vm boots")
    ap.add_argument("passfile")
    ap.add_argument("--models", help="directory with gguf files for the models subvolume, enables the aura check")
    ap.add_argument("--exchange", help="give the drive an exchange partition of this size, like 1G, and check it")
    ap.add_argument("--first-boot", action="store_true", help="write the drive without persist, choose the passphrase "
                    "at its first boot, check what it made and boot it again")
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
    ap.add_argument("--updates", help="an ext4 image labelled updates with a newer version's update files, "
                    "install them and reboot into that version")
    ap.add_argument("--backup", help="an empty ext4 image labelled backup, back up home onto it and restore from it")
    ap.add_argument("--clone", help="an empty file of at least 24G, clone the drive onto it as a removable disk "
                    "and boot the clone")
    args = ap.parse_args()
    with open(args.passfile, encoding="utf-8") as f:
        passphrase = f.read()

    work = tempfile.mkdtemp(prefix="eclipse-boot-")
    if (args.splash or args.desktop or args.updates or args.first_boot) and not args.qmp:
        args.qmp = os.path.join(work, "qmp.sock")

    # the app picks kvm or tcg and the firmware. what follows its options replaces its defaults.
    # the gpu is virtio: the firmware draws the splash on it and umbra opens it as a drm device.
    # with --first-boot eclipse-flash leaves persist out and the drive asks for the passphrase
    drive = ["--first-boot"] if args.first_boot else ["--persist", os.path.abspath(args.passfile)]
    if args.models:
        drive += ["--models", os.path.abspath(args.models)]
    if args.exchange:
        drive += ["--exchange", args.exchange]
    cmd = [
        os.path.abspath(args.vm),
        "--image", os.path.abspath(args.image),
        *drive,
        "-smp", "2",
        "-m", args.memory,
        "-device", "virtio-vga",
        "-display", "none",
        "-monitor", "none",
        "-serial", "stdio",
        "-no-reboot",
        # qemu's user network. the vm reaches the host's loopback at 10.0.2.2, where the network switch
        # step runs a server of its own
        "-nic", "user,model=virtio-net-pci",
    ]
    if args.qmp:
        cmd += ["-qmp", f"unix:{args.qmp},server,nowait"]
    if args.updates:
        # a second nvme drive. nothing on the system mounts it, the test does
        cmd += ["-drive", f"if=none,id=updates,format=raw,file={os.path.abspath(args.updates)}",
                "-device", "nvme,drive=updates,serial=updates"]
    if args.backup:
        # and one for backups. vault mounts it by the uuid of its file system
        cmd += ["-drive", f"if=none,id=backup,format=raw,file={os.path.abspath(args.backup)}",
                "-device", "nvme,drive=backup,serial=backup"]
    if args.clone:
        # and the disk the clone goes onto: a scsi disk that says it is removable, the way a stick in a
        # card reader does, since vault clones onto nothing else. zeros written to it stay holes in the file
        cmd += ["-device", "virtio-scsi-pci,id=scsi",
                "-drive", f"if=none,id=clone,format=raw,discard=unmap,detect-zeroes=unmap,file={os.path.abspath(args.clone)}",
                "-device", "scsi-hd,bus=scsi.0,drive=clone,serial=clone,removable=on"]
    print("boot-test: " + " ".join(cmd), flush=True)

    start = time.monotonic()
    deadline = start + args.timeout
    child = pexpect.spawn(cmd[0], cmd[1:], encoding="utf-8", codec_errors="replace", dimensions=(40, 160))
    # the clone boots in a second qemu, whose output goes on in the same log
    tee = Tee(args.log)
    child.logfile_read = tee

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

    def unlock():
        """Answer the luks prompt that is up and wait for the autologin shell."""
        child.send(passphrase + "\r")
        for attempt in range(3):
            if expect([PROMPT, PASSPHRASE], "the autologin shell") == 0:
                break
            if attempt == 2:
                fail("the passphrase was refused three times")
            print("\nboot-test: passphrase prompt again, retrying", flush=True)
            child.send(passphrase + "\r")
        ok("shell")

    def choose():
        """Answer the first boot's questions for a new passphrase: one too short, two that differ, then
        the passphrase twice. The drive makes persist, opens it and goes on to the autologin shell
        without asking again."""
        # a person takes a while to choose a passphrase. systemd gives up on a device after 90 s, and
        # the persist partition only comes once the passphrase is in
        print("\nboot-test: waiting 100 s before answering, as a person choosing a passphrase would", flush=True)
        time.sleep(100)
        child.send("short77\r")
        expect([rf"at least 8 characters\. {CHOOSE}"], "the question again after a passphrase that is too short")
        child.send(passphrase + "\r")
        expect([AGAIN], "the question to type the passphrase again")
        child.send(passphrase + "-other\r")
        expect([rf"not the same\. {CHOOSE}"], "the question again after two passphrases that differ")
        child.send(passphrase + "\r")
        expect([AGAIN], "the question to type the passphrase again")
        child.send(passphrase + "\r")
        if expect([PROMPT, CHOOSE, PASSPHRASE], "the autologin shell after the first boot made persist") != 0:
            fail("the first boot asked for a passphrase again after it had one")
        ok("shell, after the first boot refused a short passphrase and two that differ and made persist")

    # 1. the luks prompt, answered over serial. a second prompt means the passphrase was refused. a
    # drive written with --first-boot asks for a new passphrase instead
    if args.first_boot:
        expect([CHOOSE], "the first boot's question for a new passphrase")
        ok("the first boot asks for a new passphrase")
    else:
        expect([PASSPHRASE], "the luks passphrase prompt")
        ok("passphrase prompt")

    # 1a. the splash. whatever asks waits for us, so the screen is stable
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

    if args.first_boot:
        choose()
    else:
        unlock()

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

    # 2b. the slots. systemd-boot started the uki with its boot counter, boot-complete.target was
    # reached and systemd-bless-boot took the counter off the file name, sysupdate finds this version
    # installed, /usr runs from slot a, and slot b's two partitions wait empty behind it
    def image_version():
        status, output = run("grep '^IMAGE_VERSION=' /etc/os-release", "the image version")
        found = re.search(r'^IMAGE_VERSION="?([^"\s]+)"?\s*$', without_console(output), re.M)
        if status != 0 or not found:
            fail(f"/etc/os-release has no IMAGE_VERSION: {without_console(output).strip()!r}")
        return found.group(1)

    def started_by_systemd_boot(uki):
        """Check that systemd-boot started this uki and return its product name and version."""
        _, output = run("sudo bootctl status --no-pager", "bootctl status")
        printed = without_console(output)
        print(f"\nboot-test: bootctl status printed:\n{printed}", flush=True)
        loader = re.search(r"Current Boot Loader:\s*\n\s*Product:\s*(systemd-boot \S+)", printed)
        if not loader:
            fail("bootctl says this boot was not started by systemd-boot")
        entry = re.search(r"Current Entry:\s*(\S+)", printed)
        if not entry or entry.group(1) != uki:
            fail(f"systemd-boot started {entry.group(1) if entry else 'no entry'}, expected {uki}")
        return loader.group(1)

    def unit_state(unit):
        _, output = run(f"systemctl is-active {unit}", f"the state of {unit}")
        found = re.search(r"^(active|inactive|failed|activating|deactivating)\s*$", without_console(output), re.M)
        return found.group(1) if found else without_console(output).strip()

    def assessment():
        """What systemd-bless-boot says about this boot: good, bad, indeterminate, dirty, or clean
        when the uki had no counter."""
        _, output = run("sudo /run/current-system/systemd/lib/systemd/systemd-bless-boot status",
                        "the assessment of this boot")
        found = re.search(r"^(good|bad|indeterminate|clean|dirty)\s*$", without_console(output), re.M)
        return found.group(1) if found else without_console(output).strip()

    def ukis_on_esp(wanted, what):
        _, output = run("sudo ls -1 /boot/EFI/Linux", what)
        ukis = sorted(without_console(output).split())
        if ukis != sorted(wanted):
            fail(f"the esp holds {ukis} {what}, expected {sorted(wanted)}")

    def boot_drive():
        """(name, label, type, size) of each partition on the drive this boot came from, in order."""
        _, output = run("lsblk -brno NAME,PARTLABEL,PARTTYPE,SIZE /dev/(lsblk -no PKNAME /dev/disk/by-designator/esp)",
                        "the partitions of the boot drive")
        printed = without_console(output)
        print(f"\nboot-test: lsblk printed:\n{printed}", flush=True)
        parts = [row.split() for row in printed.splitlines()]
        return [(name, label, kind.lower(), int(size)) for name, label, kind, size in (p for p in parts if len(p) == 4)]

    def usr_from(slot, store):
        _, output = run("sudo veritysetup status usr", "the verity device under /usr")
        data = re.search(r"data device:\s*(\S+)", without_console(output))
        if not data or data.group(1) != f"/dev/{store}":
            fail(f"/usr runs from {data.group(1) if data else without_console(output).strip()!r}, "
                 f"expected slot {slot} on /dev/{store}")

    def check_slots(slot="a", other=None, failed=None, counted=True):
        """Check how this boot came up on the a/b layout and return the running version. slot is the
        slot it should run from, other the version in the other slot, None while that one is empty.
        failed is a version whose uki used up its tries and keeps its counter on the esp. counted
        says whether systemd-boot counted this boot: a uki marked good on an earlier boot has no
        counter left, then nothing marks this boot and the test starts boot-complete.target itself."""
        version = image_version()
        uki = f"eclipse_{version}.efi"
        installed = sorted((v for v in (version, other) if v), key=version_key)

        _, output = run("ls /dev/disk/by-designator/", "udev's names for the partitions of the boot drive")
        print(f"\nboot-test: /dev/disk/by-designator holds:\n{without_console(output)}", flush=True)
        loader = started_by_systemd_boot(uki)

        if counted:
            # the boot is marked good once syzygy and greetd are up, a little after the shell
            deadline = time.monotonic() + 120
            while True:
                state = unit_state("systemd-bless-boot")
                if state == "active":
                    break
                if state == "failed" or time.monotonic() > deadline:
                    _, output = run("systemctl status --no-pager systemd-bless-boot boot-complete.target",
                                    "why the boot was not marked good")
                    print(f"\nboot-test: systemctl status printed:\n{without_console(output)}", flush=True)
                    fail(f"systemd-bless-boot is {state} after {since()}, the boot was never marked good")
                time.sleep(3)
            blessed = f"the boot was marked good at {since()} and the counter is gone"
            verdict = assessment()
            if verdict != "good":
                fail(f"systemd-bless-boot says {verdict!r}, expected good")
        else:
            verdict = assessment()
            if verdict != "clean":
                fail(f"systemd-bless-boot says {verdict!r}, expected clean for a uki without a counter")
            state = unit_state("systemd-bless-boot")
            if state != "inactive":
                fail(f"systemd-bless-boot is {state} on a boot that was not counted, expected inactive")
            status, output = run("sudo timeout 120 systemctl start boot-complete.target", "boot-complete.target")
            if status != 0 or unit_state("boot-complete.target") != "active":
                fail(f"boot-complete.target could not be reached: {without_console(output).strip()!r}")
            blessed = f"its uki has no counter and boot-complete.target was reached at {since()}"
        ukis_on_esp([f"eclipse_{v}+0-{TRIES}.efi" if v == failed else f"eclipse_{v}.efi" for v in installed],
                    "with the counters of good boots gone")

        # current is the newest version installed, which is not the running one after a rollback
        _, output = run("sudo systemd-sysupdate --offline --json=short list", "systemd-sysupdate list")
        found = re.search(r'^\{"current.*\}\s*$', without_console(output), re.M)
        listing = json.loads(found.group(0)) if found else {}
        if listing.get("current") != installed[-1] or sorted(listing.get("all", []), key=version_key) != installed:
            fail(f"systemd-sysupdate lists {without_console(output).strip()[-600:]!r}, expected {installed[-1]} current "
                 f"and {', '.join(installed)} installed")

        # esp, slot a, slot b in partition order, then the exchange partition when the drive has one,
        # and persist
        parts = boot_drive()
        gib = 1024**3
        wanted = [("esp", ESP_TYPE, gib)]
        for held in ((version, other) if slot == "a" else (other, version)):
            wanted += [
                (f"store-verity_{held}" if held else "_empty", USR_VERITY_TYPE, gib),
                (f"store_{held}" if held else "_empty", USR_TYPE, 8 * gib),
            ]
        tail = (["exchange"] if args.exchange else []) + ["persist"]
        if [p[1:] for p in parts[:5]] != wanted or [p[1] for p in parts[5:]] != tail:
            fail(f"the boot drive's partitions are {[p[1:] for p in parts]}, expected {wanted} and then {', '.join(tail)}")

        store = parts[2 if slot == "a" else 4][0]
        usr_from(slot, store)
        rest = f"slot {'b' if slot == 'a' else 'a'} holds {other}" if other else "slot b is empty"
        ok(f"{loader} started {uki}, {blessed}, sysupdate lists {', '.join(installed)} installed and "
           f"{installed[-1]} current, /usr runs from slot {slot} on {store}, {rest}")
        return version

    def check_failed_boot(version, good, done):
        """Check a boot of a version whose boot check always fails, from slot a, with good in slot b.
        systemd-boot started its uki and has taken done tries off it, the check failed, nothing
        marked the boot good and the uki keeps its counter."""
        running = image_version()
        if running != version:
            fail(f"boot {done} came up running {running}, expected {version}")
        uki = f"eclipse_{version}.efi"
        loader = started_by_systemd_boot(uki)

        deadline = time.monotonic() + 120
        while (state := unit_state(NEVER_GOOD)) != "failed":
            if time.monotonic() > deadline:
                fail(f"{NEVER_GOOD} is {state} after {since()}, expected failed")
            time.sleep(3)
        for unit in ("boot-complete.target", "systemd-bless-boot"):
            state = unit_state(unit)
            if state != "inactive":
                fail(f"{unit} is {state} after {NEVER_GOOD} failed, expected inactive")
        # the file keeps the name systemd-boot gave it before starting it. with no tries left the
        # boot is already as bad as a counter can say
        left = TRIES - done
        verdict = assessment()
        if verdict != ("dirty" if left == 0 else "indeterminate"):
            fail(f"systemd-bless-boot says {verdict!r} on boot {done}, expected {'dirty' if left == 0 else 'indeterminate'}")
        counter = f"eclipse_{version}+{left}-{done}.efi"
        ukis_on_esp([f"eclipse_{good}.efi", counter], f"on boot {done} of {version}")

        parts = boot_drive()
        labels = [p[1] for p in parts[1:5]]
        wanted = [f"store-verity_{version}", f"store_{version}", f"store-verity_{good}", f"store_{good}"]
        if labels != wanted:
            fail(f"the slots hold {labels}, expected {wanted}")
        usr_from("a", parts[2][0])
        ok(f"{loader} started {uki} as {counter}, {NEVER_GOOD} failed and the boot was not marked good, "
           f"/usr runs from slot a on {parts[2][0]}")

    running = check_slots()

    # 2c. the drive eclipse-flash wrote. persist is luks2 with argon2id, the settings a person gets, and
    # its btrfs has every subvolume and the owner's home. with --exchange the exchange partition is an
    # exfat labelled EXCHANGE, as big as asked
    _, output = run("sudo cryptsetup luksDump /dev/disk/by-partlabel/persist", "the luks header of persist")
    dump = without_console(output)
    if not re.search(r"^Version:\s*2\s*$", dump, re.M) or not re.search(r"PBKDF:\s*argon2id\s*$", dump, re.M):
        fail(f"persist is not luks2 with argon2id: {dump.strip()[-600:]!r}")
    _, output = run("sudo btrfs subvolume list /persist", "the subvolumes of persist")
    found = re.findall(r"\spath (@\w+)\s*$", without_console(output), re.M)
    missing = [name for name in ("@home", "@var", "@flatpak", "@models", "@hosts", "@snapshots") if name not in found]
    if missing:
        fail(f"persist has no {', '.join(missing)}: {without_console(output).strip()!r}")
    _, output = run("stat -c home=%U:%G /home/eclipse", "the owner's home")
    if "home=eclipse:users" not in output:
        fail(f"/home/eclipse is not the owner's: {without_console(output).strip()!r}")
    exchange_bytes = None
    if args.exchange:
        unit = {"G": 1024**3, "M": 1024**2}[args.exchange[-1].upper()]
        exchange_bytes = int(args.exchange[:-1]) * unit
        _, output = run("sudo blkid -p -o export /dev/disk/by-partlabel/exchange; and sudo blockdev --getsize64 "
                        "/dev/disk/by-partlabel/exchange", "the exchange partition")
        found = without_console(output)
        if not re.search(r"^TYPE=exfat\s*$", found, re.M) or not re.search(r"^LABEL=EXCHANGE\s*$", found, re.M) \
                or not re.search(rf"^{exchange_bytes}\s*$", found, re.M):
            fail(f"the exchange partition is not an exfat of {exchange_bytes} bytes labelled EXCHANGE: {found.strip()!r}")
    maker = "the first boot" if args.first_boot else "eclipse-flash"
    ok(f"{maker} made persist luks2 with argon2id, every subvolume and the owner's home"
       + (f", and an exfat exchange partition of {args.exchange}" if args.exchange else ""))

    # 2d. a drive written with --first-boot. persist has one key slot, the system runs with the machine
    # id in @var, and vault-first-boot said what it made. the next boot asks systemd-cryptsetup's
    # question, not the first boot's, opens the same persist with the same passphrase and makes nothing
    if args.first_boot:
        uuid = r"^\s*([0-9a-fA-F-]{36})\s*$"
        machine_id = r"^\s*([0-9a-f]{32})\s*$"

        def one_line(command, what, pattern):
            status, output = run(command, what)
            found = re.search(pattern, without_console(output), re.M)
            if status != 0 or not found:
                fail(f"{what}: {without_console(output).strip()!r}")
            return found.group(1)

        def made(what):
            """What the first boot made, to compare after the next boot."""
            _, output = run("sudo cryptsetup luksDump /dev/disk/by-partlabel/persist", f"the key slots of persist {what}")
            found = {
                "slots": re.findall(r"^\s+(\d+): luks2\s*$", without_console(output), re.M),
                "luks": one_line("sudo cryptsetup luksUUID /dev/disk/by-partlabel/persist", f"the luks uuid {what}",
                                 uuid).lower(),
                "btrfs": one_line("sudo blkid -s UUID -o value /dev/mapper/persist", f"the btrfs uuid {what}", uuid).lower(),
                "machine": one_line("cat /etc/machine-id", f"the machine id {what}", machine_id),
                "partitions": [(name, label, size) for name, label, _, size in boot_drive()],
            }
            if args.exchange:
                found["exchange"] = one_line("sudo blkid -s UUID -o value /dev/disk/by-partlabel/exchange",
                                             f"the uuid of the exchange partition {what}",
                                             r"^\s*([0-9A-F]{4}-[0-9A-F]{4})\s*$")
            return found

        def said(what):
            _, output = run("sudo journalctl -b -o cat --no-pager -u vault-first-boot", f"what vault-first-boot said {what}")
            printed = without_console(output)
            print(f"\nboot-test: vault-first-boot {what}:\n{printed}", flush=True)
            return printed

        first = made("after the first boot")
        if first["slots"] != ["0"]:
            fail(f"persist has the key slots {first['slots']} after the first boot, expected one")
        in_var = one_line("sudo cat /persist/@var/lib/eclipse/machine-id", "the machine id in @var", machine_id)
        if in_var != first["machine"]:
            fail(f"the system runs with the machine id {first['machine']}, and @var holds {in_var}")
        printed = said("on the first boot")
        if "Made persist on " not in printed:
            fail("vault-first-boot did not say it made persist")
        if args.exchange and "Formatting the exchange partition." not in printed:
            fail("vault-first-boot did not say it formatted the exchange partition")
        ok(f"persist on {first['partitions'][-1][0]} has one key slot, and the system runs with the machine id in @var")

        try:
            qmp(args.qmp, {"execute": "set-action", "arguments": {"reboot": "reset"}})
        except (OSError, RuntimeError) as e:
            fail(f"qmp set-action reboot=reset: {e}")
        child.send("sudo systemctl reboot\r")
        if expect([CHOOSE, PASSPHRASE], "the passphrase prompt of the second boot") == 0:
            fail("the second boot asked for a new passphrase, it did not find the persist the first boot made")
        ok("passphrase prompt of the second boot")
        unlock()
        second = made("after the second boot")
        if second != first:
            fail(f"the second boot does not find what the first made: {first} before, {second} after")
        printed = said("on the second boot")
        if "Making persist." in printed or "Formatting the exchange partition." in printed:
            fail("vault-first-boot made something again on the second boot")
        ok("the second boot opened the same persist with the same passphrase and made nothing new")

        child.send("sudo systemctl poweroff\r")
        try:
            child.expect(pexpect.EOF, timeout=90)
        except pexpect.TIMEOUT:
            print("\nboot-test: poweroff did not end qemu, killing it", flush=True)
            child.terminate(force=True)
        print(f"\nboot-test: PASSED in {since()}", flush=True)
        return

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

        def look(what, png, seconds, console=False, lock=None, journals=(), **shape):
            """Screendump until the panel has the shape we asked for, or the console is open, or the
            lock screen is up (lock says whether it has refused a password), or give up and save
            it. On a failure the journal of each tag in journals is printed."""
            deadline = time.monotonic() + seconds
            while True:
                try:
                    width, height, rgb = screendump(args.qmp, work, "desktop")
                except (OSError, RuntimeError) as e:
                    fail(f"screendump: {e}")
                if lock is not None:
                    good, lines = check_lock(width, height, rgb, refused=lock)
                elif console:
                    good, lines = check_console(width, height, rgb)
                else:
                    good, lines = check_desktop(width, height, rgb, corona=args.corona, **shape)
                if good or time.monotonic() > deadline:
                    break
                time.sleep(2 if shape or console or lock is not None else 5)
            write_png(png, width, height, rgb)
            print("\nboot-test: " + "\nboot-test: ".join(lines), flush=True)
            if not good:
                for tag in journals:
                    _, output = run(f"journalctl -b -t {tag} --no-pager -n 40 -o cat", f"the {tag} journal")
                    print(f"\nboot-test: journalctl -t {tag} printed:\n{without_console(output)}", flush=True)
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

            # 5c. the console. Mod+Grave runs toggle-console with the arguments in
            # nix/modules/umbra.nix, and umbra msg runs the same action without the key. the first
            # time it starts ghostty, after that it hides and shows that same window
            status, printed = run("ghostty +validate-config", "ghostty's settings")
            printed = without_console(printed).strip()
            if status != 0 or printed:
                fail(f"ghostty does not take the settings file the image writes: {printed!r}")
            toggle = (f"umbra msg action toggle-console --app-id {CONSOLE_APP_ID} -- "
                      f"systemd-cat -t console ghostty --class={CONSOLE_APP_ID}")

            def console_window():
                """(id, pid) of the console's window in umbra's list, or None when it is not there."""
                status, output = run("umbra msg --json windows", "umbra's windows")
                output = without_console(output).replace("\n", "")
                if status != 0 or "[" not in output:
                    fail(f"umbra msg windows exited with {status}: {output.strip()[-300:]!r}")
                window = re.search(r'\{"id":(\d+),"title":(?:null|"(?:[^"\\]|\\.)*"),"app_id":"'
                                   + re.escape(CONSOLE_APP_ID) + r'","pid":(\d+)', output)
                return (int(window.group(1)), int(window.group(2))) if window else None

            def children(pid, what):
                _, output = run(f"pgrep -P {pid}", what)
                return re.findall(r"^\s*(\d+)\s*$", without_console(output), re.M)

            run(toggle, "the show action")
            look("the console", f"{stem}-console{extension}", 60, console=True, journals=("console", "umbra"))
            window = console_window()
            if not window:
                fail("umbra lists no console window after the show action")
            window_id, pid = window
            programs = children(pid, "the program in the console")
            if not programs:
                fail(f"ghostty {pid} runs nothing in the console")
            shell = programs[0]
            ok(f"the console is open under the panel, window {window_id}, ghostty {pid}, shell {shell}")

            run(toggle, "the hide action")
            look("the desktop and the panel with the console hidden", f"{stem}-console-hidden{extension}", 20, rows=0)
            if console_window():
                fail("umbra still lists the console window after the hide action")
            status, _ = run(f"kill -0 {shell}", "the shell in the hidden console")
            if status != 0:
                fail(f"the console's shell {shell} ended when the console was hidden")
            ok(f"the console is hidden and its shell {shell} still runs")

            run(toggle, "the show action again")
            look("the console again", f"{stem}-console-again{extension}", 20, console=True, journals=("console", "umbra"))
            again = console_window()
            if again != window:
                fail(f"the console came back as {again}, expected window {window_id} of ghostty {pid}")
            if shell not in children(pid, "the program in the console again"):
                fail(f"the console's shell {shell} is gone after showing it again")
            run(toggle, "the hide action again")
            look("the panel back without the console", f"{stem}-console-closed{extension}", 20, rows=0)
            ok(f"the same console came back with shell {shell} and went away again")

            # 5e. the lock screen. logind signals the session greetd opened when it is asked to lock
            # it, the listener umbra started runs umbra-lock, and the password goes in on the vm's
            # keyboard through the monitor. the console is open while the session is locked and
            # has to come back as it was
            _, output = run("for s in (loginctl list-sessions --no-legend | string trim | string split -f1 ' '); "
                            "if test (loginctl show-session $s -p Service --value) = greetd; "
                            "echo session=$s class=(loginctl show-session $s -p Class --value); end; end",
                            "the session greetd opened")
            found = re.search(r"session=(\S+) class=(\S+)", output)
            if not found:
                fail(f"logind lists no session from greetd: {without_console(output).strip()[-400:]!r}")
            session, session_class = found.group(1), found.group(2)
            if session_class != "user":
                fail(f"greetd's session {session} is a {session_class} session, logind locks only user sessions")
            _, output = run("grep '^N:' /proc/bus/input/devices", "the input devices")
            print(f"\nboot-test: the vm's input devices:\n{without_console(output)}", flush=True)

            def locked_hint(wanted, what):
                """Wait up to ten seconds for logind's LockedHint on the session to say wanted."""
                hint = None
                for _ in range(10):
                    _, output = run(f"loginctl show-session {session} -p LockedHint --value", "the locked hint")
                    found = re.search(r"^(yes|no)$", without_console(output), re.M)
                    hint = found.group(1) if found else without_console(output).strip()
                    if hint == wanted:
                        return
                    time.sleep(1)
                fail(f"logind says LockedHint={hint} for session {session} {what}, expected {wanted}")

            def press(*keys, what):
                """Press keys on the vm's keyboard. Each item is a list of qemu key codes held together."""
                commands = [{"execute": "send-key", "arguments": {"keys": [{"type": "qcode", "data": code} for code in held]}}
                            for held in keys]
                try:
                    qmp(args.qmp, *commands)
                except (OSError, RuntimeError) as e:
                    fail(f"typing {what}: {e}")

            def type_line(text, what):
                press(*([c] for c in text), ["ret"], what=what)

            run(toggle, "the show action before locking")
            look("the console before locking", f"{stem}-lock-console{extension}", 20, console=True, journals=("console", "umbra"))
            status, output = run(f"loginctl lock-session {session}", "loginctl lock-session")
            if status != 0:
                fail(f"loginctl lock-session {session} exited with {status}: {without_console(output).strip()!r}")
            look("the lock screen", f"{stem}-lock{extension}", 30, lock=False, journals=("lock", "umbra"))
            locked_hint("yes", "with the lock screen up")
            ok(f"loginctl locked session {session}, the lock screen covers the console and the panel")

            type_line(WRONG_PASSWORD, "a wrong password")
            look("the lock screen refusing a wrong password", f"{stem}-lock-refused{extension}", 30, lock=True,
                 journals=("lock", "umbra"))
            locked_hint("yes", "after a wrong password")
            ok("a wrong password was refused and the session stayed locked")

            type_line(PASSWORD, "the owner's password")
            look("the console after unlocking", f"{stem}-lock-unlocked{extension}", 30, console=True, journals=("lock", "umbra"))
            locked_hint("no", "after the owner's password")
            if console_window() != window:
                fail(f"the console came back as {console_window()} after unlocking, expected window {window_id} of ghostty {pid}")
            if shell not in children(pid, "the program in the console after unlocking"):
                fail(f"the console's shell {shell} is gone after unlocking")
            run(toggle, "the hide action after unlocking")
            look("the desktop and the panel after unlocking", f"{stem}-lock-desktop{extension}", 20, rows=0)
            ok(f"the owner's password unlocked it, the console came back with shell {shell}")

            # Mod+L on the same keyboard runs umbra-lock from the bind
            press(["meta_l", "l"], what="Mod+L")
            look("the lock screen from Mod+L", f"{stem}-lock-key{extension}", 30, lock=False, journals=("lock", "umbra"))
            type_line(PASSWORD, "the owner's password")
            look("the desktop and the panel after unlocking again", f"{stem}-lock-desktop-again{extension}", 30, rows=0,
                 journals=("lock", "umbra"))
            locked_hint("no", "after unlocking the lock from Mod+L")
            ok("Mod+L locked the session and the owner's password unlocked it")

            # 5d. a question for aura, from the terminal first, which prints the answer here, and
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

    # 6. timeline. vault answers on the bus and a timer takes a snapshot of home every hour. take one,
    # change a file and delete another, find the snapshot through eclipse snapshot and on the bus,
    # and restore both from it
    _, output = run("systemctl is-active vault vault-timeline.timer", "the vault units")
    states = re.findall(r"^(active|inactive|failed|activating)\s*$", without_console(output), re.M)
    if states != ["active", "active"]:
        fail(f"vault and its timer are {states or without_console(output).strip()!r}, expected both active")
    _, output = run("systemctl show -p TimersCalendar --value vault-timeline.timer", "the timer's schedule")
    if "OnCalendar=*-*-* *:00:00" not in without_console(output):
        fail(f"vault-timeline.timer does not run every hour: {without_console(output).strip()!r}")

    snapshots = "/persist/@snapshots/home"
    notes, todo = "/home/eclipse/timeline/notes.txt", "/home/eclipse/timeline/todo.txt"

    def snapshot_list(what):
        """The names `eclipse snapshot` prints, oldest first."""
        status, output = run("eclipse snapshot", f"eclipse snapshot {what}")
        printed = without_console(output)
        print(f"\nboot-test: eclipse snapshot {what} printed:\n{printed}", flush=True)
        if status != 0:
            fail(f"eclipse snapshot exited with {status} {what}")
        return re.findall(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ)\s*$", printed, re.M)

    def contents(path):
        status, output = run(f"cat {path}", f"what is in {path}")
        return without_console(output) if status == 0 else f"nothing, cat exited with {status}"

    def restore(options, what):
        status, output = run(f"eclipse snapshot restore {options}", what)
        printed = without_console(output)
        print(f"\nboot-test: eclipse snapshot restore {options} printed:\n{printed}", flush=True)
        return status, printed

    status, output = run(f"mkdir -p (dirname {notes}); and printf 'First draft\\n' > {notes}; "
                         f"and printf 'Buy milk\\n' > {todo}", "the files for the snapshot")
    if status != 0:
        fail(f"the files for the snapshot could not be written: {without_console(output).strip()!r}")
    status, output = run("eclipse snapshot take", "eclipse snapshot take")
    taken = re.search(r"^Took snapshot (\S+Z)\.\s*$", without_console(output), re.M)
    if status != 0 or not taken:
        fail(f"eclipse snapshot take exited with {status}: {without_console(output).strip()!r}")
    snapshot = taken.group(1)
    _, output = run(f"sudo btrfs property get -ts {snapshots}/{snapshot} ro", "whether the snapshot is read only")
    if "ro=true" not in output:
        fail(f"{snapshots}/{snapshot} is not a read-only snapshot: {without_console(output).strip()!r}")
    ok(f"took snapshot {snapshot}, read only under {snapshots}")

    status, _ = run(f"printf 'Second draft\\n' > {notes}; and rm {todo}", "changing one file and deleting the other")
    if status != 0:
        fail("the files could not be changed")
    names = snapshot_list("after the changes")
    if snapshot not in names:
        fail(f"eclipse snapshot lists {names}, without {snapshot}")
    _, output = run("busctl --system --json=short call dev.eclipse.Vault /dev/eclipse/Vault dev.eclipse.Vault List",
                    "the snapshots on the bus")
    found = re.search(r'\{"type":"as","data":\[(\[[^\]]*\])\]\}', output)
    on_bus = json.loads(found.group(1)) if found else without_console(output).strip()
    if on_bus != names:
        fail(f"the bus lists {on_bus!r}, eclipse snapshot lists {names}")
    # the snapshot keeps home's permissions, so the owner reads their own files in it
    if "Buy milk" not in contents(f"{snapshots}/{snapshot}/eclipse/timeline/todo.txt"):
        fail("the owner cannot read the deleted file in the snapshot")
    ok(f"eclipse snapshot and the bus list {len(names)} snapshots with {snapshot}")

    status, printed = restore(f"{snapshot} {todo}", "restoring the deleted file")
    if status != 0 or f"Restored {todo} from {snapshot}." not in printed:
        fail(f"restoring the deleted file exited with {status}")
    if "Buy milk" not in contents(todo):
        fail(f"{todo} did not come back as it was")
    _, output = run(f"stat -c owner=%U:%a {todo}", "the owner of the restored file")
    if "owner=eclipse:644" not in output:
        fail(f"the restored file is not the owner's own: {without_console(output).strip()!r}")
    # without a terminal to ask on, a file that changed stays as it is
    status, printed = restore(f"{snapshot} {notes} </dev/null", "restoring the changed file without --replace")
    if status != 1 or f"{notes} has changed since this snapshot." not in printed or "--replace" not in printed:
        fail(f"restoring the changed file without --replace exited with {status}, expected 1 and a sentence")
    if "Second draft" not in contents(notes):
        fail(f"{notes} was overwritten without --replace")
    status, printed = restore(f"--replace {snapshot} {notes}", "restoring the changed file with --replace")
    if status != 0 or f"Replaced {notes} with the copy from {snapshot}." not in printed:
        fail(f"restoring the changed file with --replace exited with {status}")
    if "First draft" not in contents(notes):
        fail(f"{notes} is not the copy from the snapshot after --replace")
    status, printed = restore(f"{snapshot} {notes}", "restoring a file that is the same")
    if status != 0 or "Nothing was restored." not in printed:
        fail(f"restoring a file that is the same as in the snapshot exited with {status}")
    ok(f"restored {todo}, and {notes} only with --replace")

    # 6a. the schedule and the rules. the timer's service takes a snapshot the way the hour does. then
    # snapshots named by hand for January: 2026-01-05 and 2026-01-12 are Mondays. keeping one hour, one
    # day and two weeks keeps this week's first and the first of the week of the 12th, and drops the
    # rest of January
    status, output = run("sudo systemctl start vault-timeline.service", "the timer's snapshot")
    if status != 0:
        _, log = run("journalctl -u vault-timeline --no-pager -n 20", "the timer's log")
        fail(f"vault-timeline.service failed: {without_console(log).strip()[-800:]!r}")
    timed = [name for name in snapshot_list("after the timer's snapshot") if name not in names]
    if len(timed) != 1 or timed[0] <= snapshot:
        fail(f"vault-timeline.service added {timed}, expected one snapshot after {snapshot}")
    ok(f"vault-timeline.service took {timed[0]}")

    by_hand = ["2026-01-05T09:00:00Z", "2026-01-12T09:00:00Z", "2026-01-13T09:00:00Z", "2026-01-13T10:00:00Z"]
    status, output = run("; and ".join(f"sudo btrfs subvolume snapshot -r /persist/@home {snapshots}/{name}"
                                       for name in by_hand), "snapshots named by hand")
    before = snapshot_list("with the snapshots named by hand")
    if status != 0 or not set(by_hand) <= set(before):
        fail(f"the snapshots named by hand are not all there: {before}")
    status, output = run("sudo vault prune --hourly 1 --daily 1 --weekly 2", "the retention rules")
    printed = without_console(output)
    print(f"\nboot-test: vault prune printed:\n{printed}", flush=True)
    dropped = re.findall(r"^Dropped snapshot (\S+Z)\.\s*$", printed, re.M)
    past = {"2026-01-05T09:00:00Z", "2026-01-13T09:00:00Z", "2026-01-13T10:00:00Z"}
    if status != 0 or not past <= set(dropped) or "2026-01-12T09:00:00Z" in dropped or before[-1] in dropped:
        fail(f"vault prune exited with {status} and dropped {dropped}, expected {sorted(past)} and not "
             f"2026-01-12T09:00:00Z or the newest")
    left = snapshot_list("after the retention rules")
    if left != sorted(set(before) - set(dropped)):
        fail(f"eclipse snapshot lists {left} after the rules dropped {dropped} out of {before}")
    ok(f"the retention rules dropped {len(dropped)} of {len(before)} snapshots and kept {', '.join(left)}")

    # 6b. backup. the drive labelled backup is an empty ext4 disk. the test mounts it the way a desktop
    # would, chooses a folder on it and unmounts it: from then on vault finds the disk by uuid and
    # mounts it itself. back up home, change a file and delete another, restore both from the backup,
    # then look at the repository on the disk
    if args.backup:
        disk = "/run/backup-disk"
        folder = f"{disk}/Eclipse"
        letter, plan = "/home/eclipse/backup/letter.txt", "/home/eclipse/backup/plan.txt"
        words = "Kept in the backup 4127"

        def backup_cli(options, what):
            status, output = run(f"eclipse backup {options}", what)
            printed = without_console(output)
            print(f"\nboot-test: eclipse backup {options} printed:\n{printed}", flush=True)
            return status, printed

        def with_disk(what):
            status, output = run(f"sudo mkdir -p {disk}; and sudo mount /dev/disk/by-label/backup {disk}", what)
            if status != 0:
                fail(f"the backup disk could not be mounted for {what}: {without_console(output).strip()!r}")

        status, output = run(f"mkdir -p (dirname {letter}); and printf '{words}\\n' > {letter}; "
                             f"and printf 'Plan A\\n' > {plan}", "the files for the backup")
        if status != 0:
            fail(f"the files for the backup could not be written: {without_console(output).strip()!r}")
        with_disk("choosing the backup folder")
        status, output = run(f"sudo vault target {folder}", "sudo vault target")
        printed = without_console(output)
        print(f"\nboot-test: vault target printed:\n{printed}", flush=True)
        found = re.search(r"The password of these backups is ([0-9a-z]{5}(?:-[0-9a-z]{5}){4})\.", printed)
        if status != 0 or f"Backups of home go to {folder} now." not in printed or not found:
            fail(f"sudo vault target exited with {status} without the folder and a password")
        password = found.group(1)
        _, output = run("sudo stat -c key=%a:%U /var/lib/eclipse/vault/backup.key", "who can read the password")
        if "key=600:root" not in output:
            fail(f"the backup password is not only root's: {without_console(output).strip()!r}")
        status, _ = run(f"sudo umount {disk}", "unmounting the backup disk")
        if status != 0:
            fail("the backup disk could not be unmounted")
        ok(f"backups go to {folder}, with a password only root reads")

        status, printed = backup_cli("now", "backing up home")
        made = re.search(r"^Backed up home as ([0-9a-f]{8}) at (\S+Z)\.\s*$", printed, re.M)
        if status != 0 or not made:
            _, log = run("journalctl -u vault --no-pager -n 20", "vault's log")
            fail(f"eclipse backup now exited with {status}: {without_console(log).strip()[-800:]!r}")
        backup = made.group(1)
        _, output = run("echo left=(count (sudo ls -A /persist/@snapshots/backup))", "the snapshot the backup read")
        if "left=0" not in output:
            fail(f"the snapshot the backup read is still there: {without_console(output).strip()!r}")
        status, printed = backup_cli("list", "the backups")
        if status != 0 or not re.search(rf"^{backup}  {made.group(2)}\s*$", printed, re.M):
            fail(f"eclipse backup list exited with {status} without {backup} at {made.group(2)}")
        _, output = run("busctl --system --json=short call dev.eclipse.Vault /dev/eclipse/Vault dev.eclipse.Vault Backups",
                        "the backups on the bus")
        if f'"{backup}' not in output:
            fail(f"the bus does not list backup {backup}: {without_console(output).strip()!r}")
        ok(f"backed up home as {backup} at {made.group(2)}, and the snapshot it read is gone")

        status, _ = run(f"printf 'Plan B\\n' > {plan}; and rm {letter}", "changing one file and deleting the other")
        if status != 0:
            fail("the files could not be changed")
        status, printed = backup_cli(f"restore {backup} {letter}", "restoring the deleted file from the backup")
        if status != 0 or f"Restored {letter} from backup {backup}." not in printed:
            fail(f"restoring the deleted file from the backup exited with {status}")
        if words not in contents(letter):
            fail(f"{letter} did not come back from the backup as it was")
        _, output = run(f"stat -c owner=%U:%a {letter}", "the owner of the file from the backup")
        if "owner=eclipse:644" not in output:
            fail(f"the file from the backup is not the owner's own: {without_console(output).strip()!r}")
        status, printed = backup_cli(f"restore {backup} {plan} </dev/null", "restoring the changed file without --replace")
        if status != 1 or f"{plan} has changed since this backup." not in printed or "--replace" not in printed:
            fail(f"restoring the changed file from the backup without --replace exited with {status}, expected 1")
        if "Plan B" not in contents(plan):
            fail(f"{plan} was overwritten from the backup without --replace")
        status, printed = backup_cli(f"restore --replace {backup} {plan}", "restoring the changed file with --replace")
        if status != 0 or f"Replaced {plan} with the copy from backup {backup}." not in printed:
            fail(f"restoring the changed file from the backup with --replace exited with {status}")
        if "Plan A" not in contents(plan):
            fail(f"{plan} is not the copy from the backup after --replace")
        ok(f"restored {letter} from backup {backup}, and {plan} only with --replace")

        # the repository is rustic's, encrypted: a wrong password opens nothing, the printed one opens
        # it, and the text of the file is in none of its files
        with_disk("looking at the repository")
        _, output = run(f"sudo ls {folder}", "the repository's files")
        if not all(part in output for part in ("config", "data", "index", "keys", "snapshots")):
            fail(f"{folder} does not hold a rustic repository: {without_console(output).strip()!r}")
        status, output = run(f"sudo rustic -r {folder} --password not-the-password --no-cache --no-progress snapshots",
                             "the repository with a wrong password")
        if status == 0 or "incorrect" not in without_console(output):
            fail(f"rustic opened the repository with a wrong password, status {status}")
        status, output = run(f"sudo rustic -r {folder} --password {password} --no-cache --no-progress snapshots --json",
                             "the repository with the printed password")
        if status != 0 or f'"id": "{backup}' not in output:
            fail(f"the printed password does not open the repository, status {status}")
        status, output = run(f"sudo grep -r -l -F '{words}' {folder}", "the file's text in the repository")
        if status != 1:
            fail(f"grep exited with {status} looking for the file's text in the repository: {without_console(output).strip()!r}")
        status, _ = run(f"sudo umount {disk}", "unmounting the backup disk again")
        if status != 0:
            fail("the backup disk could not be unmounted again")
        ok("rustic refuses the repository with a wrong password and opens it with the printed one, "
           "and the file's text is in none of its files")

    # 6c. penumbra. `eclipse run --sandbox` runs a command in bwrap, under landlock rules and a seccomp
    # filter. it gets the folder it runs in and the system's programs, nothing else of the owner's: not
    # the rest of home, not /persist, not a disk of the vm. what it writes outside its folder is gone
    # when it ends, and home as a whole goes in only read only
    home, sandbox = "/home/eclipse", "/home/eclipse/sandbox"
    secret, secret_words = "/home/eclipse/private.txt", "Kept out of the sandbox 5813"

    def sandboxed(command, what):
        status, output = run(command, what)
        printed = without_console(output)
        print(f"\nboot-test: {command} printed:\n{printed}", flush=True)
        return status, printed

    def said(printed, word):
        return re.search(rf"^{re.escape(word)}\s*$", printed, re.M) is not None

    status, output = run(f"mkdir -p {sandbox}; and printf '{secret_words}\\n' > {secret}", "the files for the sandbox")
    if status != 0:
        fail(f"the files for the sandbox could not be written: {without_console(output).strip()!r}")
    _, output = run("lsblk --nodeps --noheadings --output NAME", "the disks of the vm")
    disks = re.findall(r"^\s*((?:nvme|sd|vd)\w+)\s*$", without_console(output), re.M)
    if not disks:
        fail(f"lsblk lists no disks in the vm: {without_console(output).strip()!r}")

    # the folder it runs in is the one it gets
    status, printed = sandboxed(f"cd {sandbox}; and eclipse run --sandbox sh -c 'echo made > made.txt; "
                                f"grep -E \"^(NoNewPrivs|Seccomp):\" /proc/self/status; echo dev:; ls -A /dev; "
                                f"echo home:; ls -A {home}'", "a command in a sandbox")
    run("cd ~", "going home again")
    if status != 0:
        fail(f"eclipse run --sandbox exited with {status}")
    if not re.search(r"^NoNewPrivs:\s+1\s*$", printed, re.M) or not re.search(r"^Seccomp:\s+2\s*$", printed, re.M):
        fail("the sandboxed command does not run with no new privileges and a seccomp filter")
    listed = re.search(r"^dev:\s*$(.*)^home:\s*$(.*)", printed, re.M | re.S)
    if not listed:
        fail("the sandboxed command did not list /dev and home")
    devices = listed.group(1).split()
    seen = [name for name in devices if name.startswith(tuple(disks)) or name in ("disk", "mapper", "block")
            or name.startswith(("dm-", "loop"))]
    if "null" not in devices or seen:
        fail(f"/dev in the sandbox has {seen or devices}, expected no disks and a null device")
    if listed.group(2).split() != ["sandbox"]:
        fail(f"home in the sandbox holds {listed.group(2).split()}, expected only the folder it runs in")
    _, output = run(f"stat -c owner=%U:%a {sandbox}/made.txt; and cat {sandbox}/made.txt", "the file the sandbox made")
    if "owner=eclipse:644" not in output or not said(without_console(output), "made"):
        fail(f"the sandbox did not make {sandbox}/made.txt as the owner: {without_console(output).strip()!r}")
    ok(f"eclipse run --sandbox ran in {sandbox} with a seccomp filter, no disk in /dev and nothing else of home")

    # what it cannot reach. home and /tmp in the sandbox are empty and its own, the rest is not there
    status, printed = sandboxed(
        f"eclipse run --sandbox --folder {sandbox} sh -c 'test -e /persist && echo persist-there; "
        f"test -e /sys/block && echo sys-there; test -e /var/lib/eclipse && echo var-there; "
        f"cat {secret} && echo secret-read; cat /dev/{disks[0]} > /dev/null && echo disk-read; "
        f"echo out > {home}/outside.txt && echo home-written; echo out > /tmp/outside.txt && echo tmp-written; "
        f"echo renamed > /proc/self/comm && echo proc-written; unshare --user true; echo finished'",
        "what a sandbox cannot reach")
    if status != 0 or not said(printed, "finished"):
        fail(f"the sandboxed command exited with {status} before it finished")
    reached = [word for word in ("persist-there", "sys-there", "var-there", "secret-read", "disk-read", "proc-written")
               if said(printed, word)]
    if reached or secret_words in printed:
        fail(f"the sandbox reached what it must not: {reached or 'the words of ' + secret}")
    if not said(printed, "home-written") or not said(printed, "tmp-written"):
        fail("the sandbox could not write into its own empty home and /tmp")
    if "Operation not permitted" not in printed:
        fail("unshare in the sandbox was not refused by the seccomp filter")
    status, _ = run(f"test -e {home}/outside.txt -o -e /tmp/outside.txt", "whether what the sandbox wrote outside is there")
    if status == 0:
        fail("what the sandbox wrote outside its folder is still there after it ended")
    ok(f"the sandbox found no /persist, /sys or /var, could not read {secret} or /dev/{disks[0]} or write to /proc, "
       f"was refused a user namespace, and what it wrote outside {sandbox} was gone")

    # home as a whole, read only
    status, printed = sandboxed(f"eclipse run --sandbox --folder {sandbox} --read {home} sh -c 'cat {secret}; "
                                f"echo changed > {secret} && echo secret-written; echo new > {sandbox}/new.txt "
                                f"&& echo folder-written'", "a sandbox with home read only")
    if secret_words not in printed or said(printed, "secret-written") or not said(printed, "folder-written"):
        fail(f"with --read {home} the sandbox did not read {secret}, or wrote to it, or could not write to its folder")
    if secret_words not in contents(secret):
        fail(f"{secret} changed after a sandbox had it read only")
    ok(f"with --read {home} the sandbox read {secret} and could not change it")

    # what eclipse run refuses before anything runs
    for command, words in ((f"eclipse run --sandbox --folder {sandbox} --read /persist true",
                            "/persist cannot go into a sandbox."),
                           (f"eclipse run --sandbox --folder {sandbox} --read /dev/{disks[0]} true",
                            f"/dev/{disks[0]} cannot go into a sandbox."),
                           (f"eclipse run --sandbox --folder {home} true", f"{home} is all of your home folder."),
                           ("cd ~; and eclipse run --sandbox true", "that is all of your home folder."),
                           (f"sudo eclipse run --sandbox --folder {sandbox} true", "not as root."),
                           ("eclipse run true", "--sandbox is needed")):
        status, printed = sandboxed(command, f"what {command} refuses")
        if status not in (1, 2) or words not in " ".join(printed.split()):
            fail(f"{command} exited with {status} without saying {words!r}")
    ok("eclipse run refused /persist, a disk, all of home, root and a command without --sandbox")

    # 6d. the network switch. penumbra keeps one for each app that runs in a sandbox, named after its
    # command or by --name. off cuts the network of the app's sandboxes that run now and of every one it
    # starts later, loopback included, and on gives it back. what is off stays off when penumbra starts
    # again. the vm reaches a server this test runs on the host through qemu's user network, at 10.0.2.2
    served = tempfile.mkdtemp(prefix="eclipse-net-")
    net_words = "Reached the test server 2718"
    with open(os.path.join(served, "net.txt"), "w", encoding="utf-8") as f:
        f.write(net_words + "\n")

    class Quiet(http.server.SimpleHTTPRequestHandler):
        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), functools.partial(Quiet, directory=served))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://10.0.2.2:{server.server_address[1]}/net.txt"
    fetch = f"curl -s -m 4 {url}"
    fetcher = "/home/eclipse/fetcher"

    def spaced(printed):
        return " ".join(printed.split())

    for _ in range(20):
        status, output = run(fetch, "the test server from the vm")
        if status == 0 and net_words in output:
            break
        time.sleep(3)
    else:
        _, output = run("ip -brief address; nmcli device", "the network of the vm")
        fail(f"the vm does not reach the test server at {url}, curl exited with {status}: "
             f"{without_console(output).strip()!r}")
    status, output = run("systemctl is-active penumbra", "whether penumbra runs")
    if status != 0:
        fail(f"penumbra is not running: {without_console(output).strip()!r}")
    status, printed = sandboxed("eclipse net", "the apps before any is off")
    if status != 0 or "Every app has the network" not in spaced(printed):
        fail(f"eclipse net exited with {status} before any app was off, or did not say every app has the network")
    run(f"mkdir -p {fetcher}", "the folder for the fetching sandboxes")
    status, printed = sandboxed(f"eclipse run --sandbox --folder {fetcher} --name fetcher {fetch}",
                                "a sandbox that reaches the test server")
    if status != 0 or net_words not in printed:
        fail(f"a sandbox did not reach the test server at {url}, it exited with {status}")
    ok(f"penumbra runs, no app is off, and a sandbox reaches the test server at {url}")

    # a sandbox that goes on running. each time the test tells it to, it fetches and writes down what it got
    steps = ("for step in 1 2 3; do while ! test -e go-$step; do sleep 0.2; done; "
             f"curl -s -m 4 {url} > got-$step; echo $? > status-$step; done")
    status, output = run(f"eclipse run --sandbox --folder {fetcher} --name fetcher sh -c '{steps}' "
                         f"< /dev/null > {fetcher}/fetcher.log 2>&1 &; disown", "a sandbox that goes on running")
    if status != 0:
        fail(f"the sandbox that goes on running could not be started: {without_console(output).strip()!r}")

    def fetched(step):
        """Tells the running sandbox to fetch once more. Returns curl's exit status and what it got."""
        run(f"touch {fetcher}/go-{step}", f"telling the sandbox to fetch for step {step}")
        until = time.monotonic() + 40
        while time.monotonic() < until:
            _, output = run(f"cat {fetcher}/status-{step}", f"whether the sandbox fetched for step {step}")
            done = re.search(r"^(\d+)\s*$", without_console(output), re.M)
            if done:
                _, got = run(f"cat {fetcher}/got-{step}", f"what the sandbox got in step {step}")
                return int(done.group(1)), without_console(got)
            time.sleep(1)
        _, output = run(f"cat {fetcher}/fetcher.log", "what the running sandbox printed")
        fail(f"the running sandbox did not fetch for step {step}: {without_console(output).strip()!r}")

    code, got = fetched(1)
    if code != 0 or net_words not in got:
        fail(f"the running sandbox did not reach the test server before its network was off, curl exited with {code}")
    _, output = run("systemctl --user list-units --plain --no-legend 'app-penumbra-fetcher-*'", "the running sandbox's scope")
    units = re.findall(r"app-penumbra-fetcher-\d+\.scope", without_console(output))
    print(f"\nboot-test: the user manager lists {units}", flush=True)
    if len(units) != 1:
        fail(f"the user manager lists {units} for fetcher, expected the one scope of the running sandbox")
    status, printed = sandboxed("eclipse net off fetcher", "turning fetcher's network off while it runs")
    if status != 0 or "The network is off for fetcher, also in the sandbox it runs in now." not in spaced(printed):
        fail(f"eclipse net off fetcher exited with {status} without saying it cut the running sandbox")
    status, printed = sandboxed("eclipse net", "the apps with fetcher off")
    if status != 0 or not re.search(r"^fetcher\s+Off\s+1\s*$", printed, re.M):
        fail(f"eclipse net does not list fetcher off with one sandbox running: {printed.strip()!r}")
    _, output = run("sudo nft list table inet penumbra", "penumbra's table")
    table = without_console(output)
    print(f"\nboot-test: sudo nft list table inet penumbra printed:\n{table}", flush=True)
    if units[0] not in table:
        fail(f"penumbra's table does not hold {units[0]}")
    cut, got = fetched(2)
    if cut == 0 or net_words in got:
        fail("the running sandbox reached the test server after its network was turned off")
    status, printed = sandboxed("eclipse net on fetcher", "turning fetcher's network on while it runs")
    if status != 0 or "The network is on for fetcher, also in the sandbox it runs in now." not in spaced(printed):
        fail(f"eclipse net on fetcher exited with {status} without saying it gave the running sandbox the network back")
    code, got = fetched(3)
    if code != 0 or net_words not in got:
        fail(f"the running sandbox did not reach the test server after its network was on again, curl exited with {code}")
    ok(f"eclipse net off cut the network of {units[0]} while it ran (curl exited with {cut}), and eclipse net on gave it back")

    # the next sandbox of an app that is off starts without the network. other apps keep theirs
    status, printed = sandboxed("eclipse net off fetcher", "turning fetcher's network off")
    if status != 0 or "The network is off for fetcher" not in spaced(printed):
        fail(f"eclipse net off fetcher exited with {status}")
    status, printed = sandboxed(f"eclipse run --sandbox --folder {fetcher} --name fetcher {fetch}",
                                "a new sandbox of fetcher while its network is off")
    if status == 0 or net_words in printed or "The network is off for fetcher." not in spaced(printed):
        fail(f"a new sandbox of fetcher exited with {status} while its network was off, or did not say it was off")
    status, printed = sandboxed(f"eclipse run --sandbox --folder {fetcher} {fetch}", "a sandbox of curl")
    if status != 0 or net_words not in printed:
        fail(f"a sandbox of curl did not reach the test server while fetcher's network was off, it exited with {status}")
    if args.models:
        # aura's local api on 127.0.0.1. a sandbox without the network has no loopback either
        loopback = "curl -s -o /dev/null -m 4 -w 'code=%{http_code}' http://127.0.0.1:11434/v1/models"
        codes = []
        for app in ("fetcher", "curl"):
            _, printed = sandboxed(f"eclipse run --sandbox --folder {fetcher} --name {app} {loopback}",
                                   f"aura's local api from a sandbox of {app}")
            found = re.search(r"code=(\d{3})", printed)
            codes.append(found.group(1) if found else None)
        if codes[0] != "000" or codes[1] in (None, "000"):
            fail(f"aura's local api answered a sandbox of fetcher with {codes[0]} and one of curl with {codes[1]}, "
                 "expected no answer and an answer")
    ok("a new sandbox of fetcher started without the network while one of curl reached the test server"
       + (", and only curl's reached aura's local api" if args.models else ""))

    # aura is not an app of the switch. its unit keeps it off the network, which holds for anything in its cgroup
    status, _ = run("systemctl is-active aura", "whether aura runs")
    if status == 0:
        status, printed = sandboxed(f"sudo sh -c 'echo $$ > /sys/fs/cgroup/system.slice/aura.service/cgroup.procs; "
                                    f"exec {fetch}'", "the test server from aura's cgroup")
        if status == 0 or net_words in printed:
            fail("a process in aura's cgroup reached the test server")
        ok(f"a process in aura's cgroup does not reach the test server, curl exited with {status}")

    # what is off stays off when penumbra starts again
    status, output = run("sudo systemctl restart penumbra; and systemctl is-active penumbra", "restarting penumbra")
    if status != 0:
        fail(f"penumbra did not start again: {without_console(output).strip()!r}")
    _, output = run("sudo cat /var/lib/eclipse/penumbra/network-off", "the apps penumbra keeps off")
    if not said(without_console(output), "fetcher"):
        fail(f"penumbra's file does not hold fetcher: {without_console(output).strip()!r}")
    status, printed = sandboxed("eclipse net", "the apps after penumbra started again")
    if status != 0 or not re.search(r"^fetcher\s+Off\s+\d+\s*$", printed, re.M):
        fail(f"eclipse net does not list fetcher off after penumbra started again: {printed.strip()!r}")
    status, printed = sandboxed(f"eclipse run --sandbox --folder {fetcher} --name fetcher {fetch}",
                                "a sandbox of fetcher after penumbra started again")
    if status == 0 or net_words in printed:
        fail(f"a sandbox of fetcher reached the test server after penumbra started again, it exited with {status}")
    ok("fetcher's network stayed off when penumbra started again")

    # what the switch refuses
    for command, words, codes in (("eclipse net off 'no/such'", "cannot be the name of an app.", (2,)),
                                  (f"eclipse run --sandbox --folder {fetcher} --name 'a b' true",
                                   "cannot be the name of an app.", (2,)),
                                  ("penumbra start -- true", "is not in one. Nothing was run.", (126,))):
        status, printed = sandboxed(command, f"what {command} refuses")
        if status not in codes or words not in spaced(printed):
            fail(f"{command} exited with {status} without saying {words!r}")
    status, printed = sandboxed("sudo -u nobody busctl call dev.eclipse.Penumbra /dev/eclipse/Penumbra "
                                "dev.eclipse.Penumbra SetNetwork sb fetcher true", "the switch turned by nobody")
    _, listed = sandboxed("eclipse net", "the apps after nobody tried the switch")
    if status == 0 or not re.search(r"^fetcher\s+Off\s+\d+\s*$", listed, re.M):
        fail(f"nobody turned fetcher's network on, busctl exited with {status}")
    status, printed = sandboxed("eclipse net on fetcher", "turning fetcher's network on")
    on_status, printed = sandboxed(f"eclipse run --sandbox --folder {fetcher} --name fetcher {fetch}",
                                   "a sandbox of fetcher with its network on")
    if status != 0 or on_status != 0 or net_words not in printed:
        fail(f"fetcher did not reach the test server after eclipse net on, which exited with {status}")
    server.shutdown()
    ok("eclipse net refused a name that is not an app's, penumbra refused a start outside a sandbox's scope, "
       "nobody could not turn the switch, and fetcher's network came back")

    # 7. the update. the second drive holds two newer versions' files. systemd-sysupdate checks them
    # against SHA256SUMS, writes the store and its verity partition into the free slot under the
    # uuids in their names and puts the uki on the esp with three tries. then the vm reboots into it
    if args.updates:

        def install(directory, running, slot):
            """Install the version in this directory of the updates drive while running runs. Its
            partitions have to land in slot under the uuids in the file names, running stays in the
            other slot, and the uki is on the esp with all its tries. Returns the new version."""
            status, output = run(f"sudo mkdir -p {UPDATES_DRIVE} {UPDATES}; "
                                 f"and sudo mount -o ro /dev/disk/by-label/updates {UPDATES_DRIVE}; "
                                 f"and sudo mount --bind -o ro {UPDATES_DRIVE}/{directory} {UPDATES}; and ls -1 {UPDATES}",
                                 f"the update files in {directory}")
            names = without_console(output).split()
            if status != 0:
                fail(f"{directory} on the updates drive could not be mounted on {UPDATES}: {without_console(output).strip()!r}")
            print(f"\nboot-test: {UPDATES} holds:\n" + "\n".join(names), flush=True)
            new = next((found.group(1) for found in (re.fullmatch(r"eclipse_([^_]+)\.efi", name) for name in names)
                        if found), None)
            if not new or version_key(new) <= version_key(running):
                fail(f"{directory} on the updates drive has no uki of a version after {running}: {names}")

            def uuid_in_name(kind):
                for name in names:
                    found = re.fullmatch(rf"eclipse_{re.escape(new)}_([0-9a-fA-F-]{{36}})\.{kind}(?:\.zst)?", name)
                    if found:
                        return found.group(1).lower()
                fail(f"{directory} on the updates drive has no {kind} file for {new}: {names}")

            verity_uuid, store_uuid = uuid_in_name("verity"), uuid_in_name("store")

            # the store is about 6G, written from the zstd file on the other drive
            started = time.monotonic()
            _, output = run("sudo systemd-sysupdate --verify=no update 2>&1 | tail -n 40; echo update-status=$pipestatus[1]",
                            f"systemd-sysupdate update to {new}")
            took = time.monotonic() - started
            printed = without_console(output)
            print(f"\nboot-test: systemd-sysupdate update printed:\n{printed}", flush=True)
            found = re.search(r"update-status=(\d+)", printed)
            if not found or found.group(1) != "0":
                fail(f"systemd-sysupdate update exited with {found.group(1) if found else 'no status'}")

            # sysupdate's current is the newest version installed, not the one running. a version
            # older than running was removed to make room
            _, output = run("sudo systemd-sysupdate --offline --json=short list", "systemd-sysupdate list after the update")
            found = re.search(r'^\{"current.*\}\s*$', without_console(output), re.M)
            listing = json.loads(found.group(0)) if found else {}
            if listing.get("current") != new or sorted(listing.get("all", [])) != sorted([running, new]):
                fail(f"systemd-sysupdate lists {without_console(output).strip()[-600:]!r} after the update, expected "
                     f"{new} current and {running} installed next to it")

            # all tries left and none done. systemd-boot takes one off each time it starts the file
            fresh = f"eclipse_{new}+{TRIES}-0.efi"
            ukis_on_esp([f"eclipse_{running}.efi", fresh], "after the update")

            # the table on the drive itself, udev may not have read the new labels yet
            _, output = run("sudo sfdisk --dump /dev/(lsblk -no PKNAME /dev/disk/by-designator/esp)",
                            "the partition table after the update")
            table = [(name, uuid.lower()) for uuid, name in
                     re.findall(r'uuid=([0-9A-Fa-f-]{36}), name="([^"]*)"', without_console(output))]
            wanted = [(f"store-verity_{new}", verity_uuid), (f"store_{new}", store_uuid)]
            written, kept = (table[1:3], table[3:5]) if slot == "a" else (table[3:5], table[1:3])
            if written != wanted or [name for name, _ in kept] != [f"store-verity_{running}", f"store_{running}"]:
                fail(f"the partitions after the update are {table}, expected {wanted} in slot {slot} and {running} "
                     f"in the other")
            run(f"sudo umount {UPDATES} {UPDATES_DRIVE}", "unmounting the updates drive")
            ok(f"systemd-sysupdate installed {new} in {took:.0f}s: verity {verity_uuid} and store {store_uuid} in "
               f"slot {slot}, {fresh} on the esp")
            return new

        # -no-reboot ends qemu when the guest reboots. for the reboots here the vm resets instead
        def reboot_action(action):
            try:
                qmp(args.qmp, {"execute": "set-action", "arguments": {"reboot": action}})
            except (OSError, RuntimeError) as e:
                fail(f"qmp set-action reboot={action}: {e}")

        def reboot(what):
            child.send("sudo systemctl reboot\r")
            expect([PASSPHRASE], f"the luks passphrase prompt {what}")
            ok(f"passphrase prompt {what}")
            unlock()

        new = install("next", running, "b")
        reboot_action("reset")
        reboot("after the update")
        reboot_action("shutdown")
        after = check_slots(slot="b", other=running)
        if after != new:
            fail(f"the vm came back running {after}, expected {new}")
        ok(f"rebooted into {new} from slot b, {running} stays in slot a")

        # 7a. the rollback. broken's boot check always fails. sysupdate writes it over running, the
        # oldest version, in slot a. none of its boots is marked good, so each start takes a try off
        # its uki, and once it has none left systemd-boot starts new from slot b again
        broken = install("broken", new, "a")
        reboot_action("reset")
        for done in range(1, TRIES + 1):
            reboot(f"for boot {done} of {broken}")
            check_failed_boot(broken, new, done)
        reboot(f"after {broken} used up its tries")
        reboot_action("shutdown")
        after = check_slots(slot="b", other=broken, failed=broken, counted=False)
        if after != new:
            fail(f"the vm came back running {after} after {TRIES} failed boots, expected {new}")
        ok(f"{broken} failed {TRIES} boots and {new} started again from slot b, sysupdate still lists {broken}")

    # 8. the clone. the vm has an empty scsi disk that says it is removable, the way a card reader or
    # a usb bridge does. eclipse clone refuses the drive this system runs from, a disk that is not
    # removable and a serial that is not the disk's, then writes the running drive onto the removable
    # disk with a passphrase of its own. before the vm goes down the test reads what it wrote: the
    # partition table, the store against its verity tree and the luks header. step 10 boots it
    if args.clone:
        clone_letter = "/home/eclipse/clone/letter.txt"
        clone_words = "Written before the clone 7051"
        status, output = run(f"mkdir -p (dirname {clone_letter}); and printf '{clone_words}\\n' > {clone_letter}",
                             "the file for the clone")
        if status != 0:
            fail(f"the file for the clone could not be written: {without_console(output).strip()!r}")
        cloned = image_version()

        def one_line(command, what, pattern):
            """The first match of pattern in what a command printed."""
            status, output = run(command, what)
            found = re.search(pattern, without_console(output), re.M)
            if status != 0 or not found:
                fail(f"{what}: {command} exited with {status}: {without_console(output).strip()[-400:]!r}")
            return found.group(1)

        uuid = r"^\s*([0-9a-fA-F-]{8,36})\s*$"
        boot = one_line("lsblk --noheadings --output PKNAME /dev/disk/by-designator/esp", "the boot drive", r"^\s*(\S+)\s*$")
        esp_uuid = one_line("lsblk --noheadings --output UUID /dev/disk/by-designator/esp", "the esp's uuid", uuid)
        machine = one_line("cat /etc/machine-id", "the machine id", r"^\s*([0-9a-f]{32})\s*$")
        usrhash = one_line("cat /proc/cmdline", "the usrhash", r"usrhash=([0-9a-f]{64})")
        # by-designator/usr is the verity device, not a partition. veritysetup names the two partitions
        running_store = one_line("sudo veritysetup status usr", "the store /usr runs from", r"data device:\s*(\S+)")
        running_verity = one_line("sudo veritysetup status usr", "the verity partition /usr runs from",
                                  r"hash device:\s*(\S+)")
        store_uuid = one_line(f"lsblk --noheadings --output PARTUUID {running_store}",
                              "the store's partition uuid", uuid).lower()
        verity_uuid = one_line(f"lsblk --noheadings --output PARTUUID {running_verity}",
                               "the verity partition's uuid", uuid).lower()
        first_persist = one_line(f"lsblk --list --noheadings --output PATH,PARTLABEL /dev/{boot}",
                                 "the persist partition of the boot drive", r"^\s*(\S+)\s+persist\s*$")
        first_luks = one_line(f"sudo cryptsetup luksUUID {first_persist}", "the uuid of persist", uuid).lower()
        first_snapshots = snapshot_list("before the clone")
        key_hash = None
        if args.backup:
            key_hash = one_line("sudo sha256sum /var/lib/eclipse/vault/backup.key", "the hash of the backup password",
                                r"^([0-9a-f]{64})\s")

        # the disks by serial. the one removable disk is the clone's
        _, output = run("lsblk --nodeps --bytes --pairs --output PATH,NAME,SERIAL,RM,TRAN,SIZE", "the disks of the vm")
        printed = without_console(output)
        print(f"\nboot-test: lsblk printed:\n{printed}", flush=True)
        disks = [fields for fields in (dict(re.findall(r'(\w+)="([^"]*)"', line)) for line in printed.splitlines())
                 if "PATH" in fields]
        removable = [disk for disk in disks if disk.get("RM") == "1"]
        if len(removable) != 1:
            fail(f"the vm has {len(removable)} removable disks, expected the one for the clone")
        target = removable[0]["PATH"]
        serial = removable[0].get("SERIAL") or removable[0]["NAME"]
        by_id = one_line(f"for link in /dev/disk/by-id/*; if test (realpath $link) = {target}; echo link=$link; end; end",
                         "the clone's disk in /dev/disk/by-id", r"^link=(\S+)\s*$")

        def clone_cli(disk, typed, what):
            status, output = run(f"printf '%s\\n' '{CLONE_PASSPHRASE}' | sudo eclipse clone --serial '{typed}' {disk}", what)
            printed = without_console(output)
            print(f"\nboot-test: sudo eclipse clone --serial {typed} {disk} printed:\n{printed}", flush=True)
            return status, printed

        status, printed = clone_cli(f"/dev/{boot}", "eclipse", "a clone onto the drive this system runs from")
        if status != 1 or "is the drive this system runs from." not in printed:
            fail(f"eclipse clone onto the running drive exited with {status}, expected 1 and a refusal")
        if args.backup:
            backup_disk = next((disk["PATH"] for disk in disks if disk.get("SERIAL") == "backup"), None)
            if not backup_disk:
                fail("lsblk lists no disk with the serial backup")
            status, printed = clone_cli(backup_disk, "backup", "a clone onto a disk that is not removable")
            if status != 1 or "is neither removable nor on USB." not in printed:
                fail(f"eclipse clone onto the backup disk exited with {status}, expected 1 and a refusal")
        status, printed = clone_cli(by_id, f"not-{serial}", "a clone with a serial that is not the disk's")
        if status != 1 or f"is not the serial of {target}. Nothing was written." not in printed:
            fail(f"eclipse clone with a wrong serial exited with {status}, expected 1 and a refusal")
        _, output = run(f"lsblk --noheadings --list --output NAME {target}", "the clone's disk after the refusals")
        if len(without_console(output).split()) != 1:
            fail(f"{target} has partitions after eclipse clone refused it: {without_console(output).strip()!r}")
        ok("eclipse clone refused the running drive, a disk that is not removable and a wrong serial, and wrote nothing")

        started = time.monotonic()
        status, printed = clone_cli(by_id, serial, "eclipse clone")
        took = time.monotonic() - started
        if status != 0 or f"is a second drive now, with version {cloned} " not in printed:
            fail(f"eclipse clone exited with {status}")
        _, output = run("sudo ls -A /persist/@snapshots/clone", "the snapshots the clone sent")
        if without_console(output).strip():
            fail(f"the clone left snapshots behind: {without_console(output).strip()!r}")
        ok(f"eclipse clone wrote {cloned} onto {target} ({by_id}) in {took:.0f}s")

        # slot a holds the running version under the uuids its uki looks for, slot b is empty
        _, output = run(f"sudo sfdisk --dump {target}", "the clone's partition table")
        table = re.findall(r'^(\S+) : start=\s*\d+, size=\s*(\d+), type=([0-9A-Fa-f-]{36}), uuid=([0-9A-Fa-f-]{36}), '
                           r'name="([^"]*)"', without_console(output), re.M)
        print(f"\nboot-test: the clone's partitions: {table}", flush=True)
        sectors = 1024**3 // 512
        wanted = [("esp", ESP_TYPE, sectors), (f"store-verity_{cloned}", USR_VERITY_TYPE, sectors),
                  (f"store_{cloned}", USR_TYPE, 8 * sectors), ("_empty", USR_VERITY_TYPE, sectors),
                  ("_empty", USR_TYPE, 8 * sectors)]
        tail = (["exchange"] if args.exchange else []) + ["persist"]
        if [(name, kind.lower(), int(size)) for _, size, kind, _, name in table[:5]] != wanted \
                or [row[4] for row in table[5:]] != tail:
            fail(f"the clone's partitions are {table}, expected {wanted} and then {', '.join(tail)}")
        if (table[1][3].lower(), table[2][3].lower()) != (verity_uuid, store_uuid):
            fail(f"the clone's slot a has the uuids {table[1][3]} and {table[2][3]}, the running slot "
                 f"{verity_uuid} and {store_uuid}")
        clone_verity, clone_store, clone_persist = table[1][0], table[2][0], table[-1][0]
        if args.exchange:
            # as big as the first drive's, and an empty exfat of its own
            _, output = run(f"sudo blkid -p -o export {table[5][0]}", "the clone's exchange partition")
            found = without_console(output)
            if int(table[5][1]) * 512 != exchange_bytes or not re.search(r"^TYPE=exfat\s*$", found, re.M) \
                    or not re.search(r"^LABEL=EXCHANGE\s*$", found, re.M):
                fail(f"the clone's exchange partition is not an exfat of {exchange_bytes} bytes: {found.strip()!r}")
        status, output = run(f"sudo veritysetup verify {clone_store} {clone_verity} {usrhash}",
                             "the clone's store against its verity tree")
        if status != 0:
            fail(f"the clone's store does not match the usrhash: {without_console(output).strip()[-400:]!r}")
        ok(f"the clone's slot a holds {cloned} under the running uuids, its store matches the usrhash, slot b is empty")

        # persist. the first drive's passphrase does not open the clone's header and the clone's does.
        # the first drive's header opens with its own passphrase over the clone's data, and what that
        # reads is not a file system: the volume keys differ
        header = "/run/first-persist.header"
        status, _ = run(f"sudo rm -f {header}; and sudo cryptsetup luksHeaderBackup {first_persist} --header-backup-file {header}",
                        "the first drive's luks header")
        if status != 0:
            fail("the first drive's luks header could not be saved")

        def opens(options, secret, what, name=""):
            status, _ = run(f"printf '%s' '{secret}' | sudo cryptsetup open {options} --key-file - {clone_persist} {name}", what)
            return status == 0

        def signature(name):
            _, output = run(f"sudo blkid -p -o export /dev/mapper/{name}; sudo cryptsetup close {name}",
                            f"what {name} reads as")
            return without_console(output)

        if opens("--test-passphrase", passphrase, "the clone's header with the first drive's passphrase"):
            fail("the first drive's passphrase opens the clone's persist")
        if not opens("--test-passphrase", CLONE_PASSPHRASE, "the clone's header with its own passphrase"):
            fail("the passphrase the clone was made with does not open its persist")
        if not opens(f"--readonly --header {header}", passphrase, "the clone's data under the first drive's header",
                     "first-key"):
            fail("the first drive's saved header does not open with its passphrase")
        found = signature("first-key")
        if re.search(r"^TYPE=", found, re.M):
            fail(f"the clone's persist reads as {found!r} with the first drive's volume key")
        if not opens("--readonly", CLONE_PASSPHRASE, "the clone's data under its own header", "clone-key"):
            fail("the clone's persist does not open read only with its passphrase")
        found = signature("clone-key")
        if not re.search(r"^TYPE=btrfs\s*$", found, re.M) or not re.search(r"^LABEL=persist\s*$", found, re.M):
            fail(f"the clone's persist is not the btrfs labelled persist: {found!r}")
        clone_luks = one_line(f"sudo cryptsetup luksUUID {clone_persist}", "the uuid of the clone's persist", uuid).lower()
        if clone_luks == first_luks:
            fail(f"the clone's persist has the first drive's luks uuid {first_luks}")
        ok(f"the clone's persist {clone_luks} opens only with its own passphrase and has a volume key of its own")

    # 9. down
    def power_off():
        child.send("sudo systemctl poweroff\r")
        try:
            child.expect(pexpect.EOF, timeout=90)
        except pexpect.TIMEOUT:
            print("\nboot-test: poweroff did not end qemu, killing it", flush=True)
            child.terminate(force=True)

    power_off()

    # 10. the clone by itself. qemu starts again with only the clone's disk as its drive. the first
    # drive's passphrase is refused and the clone's opens it, the file from home is there, and the clone
    # runs the version that ran when it was made, from its own esp and slot a
    if args.clone:
        child.close()
        cmd = [
            os.path.abspath(args.vm),
            "--image", os.path.abspath(args.clone),
            "-smp", "2",
            "-m", args.memory,
            "-device", "virtio-vga",
            "-display", "none",
            "-monitor", "none",
            "-serial", "stdio",
            "-no-reboot",
        ]
        print("\nboot-test: " + " ".join(cmd), flush=True)
        child = pexpect.spawn(cmd[0], cmd[1:], encoding="utf-8", codec_errors="replace", dimensions=(40, 160))
        child.logfile_read = tee
        expect([PASSPHRASE], "the luks passphrase prompt of the clone")
        ok("passphrase prompt of the clone")
        child.send(passphrase + "\r")
        if expect([PROMPT, PASSPHRASE], "the clone to answer the first drive's passphrase") == 0:
            fail("the first drive's passphrase opened the clone")
        ok("the clone refused the first drive's passphrase")
        child.send(CLONE_PASSPHRASE + "\r")
        if expect([PROMPT, PASSPHRASE], "the autologin shell on the clone") == 1:
            fail("the clone refused the passphrase it was made with")
        ok("shell on the clone")

        if clone_words not in contents(clone_letter):
            fail(f"{clone_letter} is not on the clone")
        _, output = run(f"stat -c owner=%U:%a {clone_letter}", "the owner of the file on the clone")
        if "owner=eclipse:644" not in output:
            fail(f"the file on the clone is not the owner's own: {without_console(output).strip()!r}")
        ok(f"{clone_letter} is on the clone, the owner's own")

        after = check_slots(slot="a", counted=False)
        if after != cloned:
            fail(f"the clone runs {after}, expected {cloned}, the version that ran when it was made")
        if one_line("lsblk --noheadings --output UUID /dev/disk/by-designator/esp", "the clone's esp uuid", uuid) == esp_uuid:
            fail(f"the clone booted from an esp with the first drive's uuid {esp_uuid}")
        if one_line("cat /etc/machine-id", "the clone's machine id", r"^\s*([0-9a-f]{32})\s*$") == machine:
            fail(f"the clone has the first drive's machine id {machine}")
        if one_line("sudo cryptsetup luksUUID /dev/disk/by-partlabel/persist", "the uuid of persist on the clone",
                    uuid).lower() != clone_luks:
            fail(f"the clone unlocked a persist that is not {clone_luks}")
        _, output = run("sudo find /persist/@snapshots -mindepth 1 -maxdepth 2", "the snapshots on the clone")
        carried = [name for name in first_snapshots if name in output]
        if carried:
            fail(f"the clone has the first drive's snapshots {carried}")
        if key_hash and one_line("sudo sha256sum /var/lib/eclipse/vault/backup.key", "the backup password on the clone",
                                 r"^([0-9a-f]{64})\s") != key_hash:
            fail("the clone does not have the backup password of the first drive")
        ok(f"the clone booted {cloned} from its own esp, with a machine id of its own, none of the first drive's "
           f"snapshots{' and its backup password' if key_hash else ''}")
        power_off()

    print(f"\nboot-test: PASSED in {since()}", flush=True)


if __name__ == "__main__":
    main()
