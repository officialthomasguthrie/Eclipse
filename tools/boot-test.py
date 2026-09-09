#!/usr/bin/env python3
"""Boots an image in qemu and checks that the system comes up. The boot job in ci runs this.

Usage: boot-test.py <image.raw> <passfile> [--timeout 300] [--log serial.log] [--qmp sock]

The image needs its persist partition already (persist-image.sh). Everything goes through the serial
console: the luks prompt, the autologin shell, a few commands. The serial output is printed as it
arrives and kept in the log file.
"""

import argparse
import glob
import os
import shutil
import sys
import tempfile
import time

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
    ap.add_argument("image")
    ap.add_argument("passfile")
    ap.add_argument("--timeout", type=int, default=300, help="seconds for the whole boot")
    ap.add_argument("--log", default="serial.log")
    ap.add_argument("--memory", default="4096")
    ap.add_argument("--qmp", help="unix socket for the qemu monitor")
    ap.add_argument(
        "--ovmf-code",
        default=first(
            [
                "/usr/share/OVMF/OVMF_CODE_4M.fd",
                "/usr/share/OVMF/OVMF_CODE.fd",
                "/usr/share/edk2/x64/OVMF_CODE.4m.fd",
                "/usr/share/edk2-ovmf/OVMF_CODE.fd",
            ]
        ),
    )
    ap.add_argument(
        "--ovmf-vars",
        default=first(
            [
                "/usr/share/OVMF/OVMF_VARS_4M.fd",
                "/usr/share/OVMF/OVMF_VARS.fd",
                "/usr/share/edk2/x64/OVMF_VARS.4m.fd",
                "/usr/share/edk2-ovmf/OVMF_VARS.fd",
            ]
        ),
    )
    args = ap.parse_args()
    if not args.ovmf_code or not args.ovmf_vars:
        sys.exit("no ovmf firmware found, pass --ovmf-code and --ovmf-vars")
    with open(args.passfile, encoding="utf-8") as f:
        passphrase = f.read()

    work = tempfile.mkdtemp(prefix="eclipse-boot-")
    vars_copy = os.path.join(work, "vars.fd")
    shutil.copy(args.ovmf_vars, vars_copy)
    os.chmod(vars_copy, 0o644)

    kvm = os.access("/dev/kvm", os.R_OK | os.W_OK)
    cmd = [
        "qemu-system-x86_64",
        "-machine", "q35,accel=" + ("kvm" if kvm else "tcg"),
        "-cpu", "host" if kvm else "max",
        "-smp", "2",
        "-m", args.memory,
        "-drive", f"if=pflash,format=raw,readonly=on,file={args.ovmf_code}",
        "-drive", f"if=pflash,format=raw,file={vars_copy}",
        "-device", "qemu-xhci",
        "-drive", f"if=none,id=usb0,format=raw,file={args.image}",
        "-device", "usb-storage,drive=usb0",
        "-vga", "std",
        "-display", "none",
        "-monitor", "none",
        "-serial", "stdio",
        "-no-reboot",
    ]
    if args.qmp:
        cmd += ["-qmp", f"unix:{args.qmp},server,nowait"]
    print("boot-test: " + " ".join(cmd), flush=True)
    print("boot-test: kvm " + ("yes" if kvm else "no, this will be slow"), flush=True)

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
