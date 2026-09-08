# Eclipse OS

Your computer, in your pocket.

Eclipse is a full personal computer on a USB stick. Plug it into almost any PC, boot from it, and you're at your own desk: your desktop, your files, your tools, your settings, your own local AI. Shut down, pull the stick, and the machine you borrowed is exactly as you found it.

The host lends you its screen, keyboard, CPU, RAM, GPU and Wi-Fi. Everything else is yours and travels with you.

## Why

I got tired of carrying a laptop around to use a computer that already exists wherever I'm going. Laptops age. The stuff on them shouldn't have to. Eclipse splits the two: the hardware is whatever's in front of you, the computer is in your pocket.

It's not Tails. Tails is built to forget. Eclipse is built to remember.

## What you get

- The same desktop on every machine, about 20 seconds after power on.
- Everything encrypted. Lose the stick and it's noise to whoever finds it.
- Nothing ever written to the host's disks. Not swap, not logs, not caches.
- It remembers each machine it has been in. Second time on the same laptop, it's already set up for that screen and keyboard.
- Aura, a local AI that runs on whatever GPU or CPU the host has. No account, no cloud, works on a plane. It can search your files by meaning, explain the error in your terminal, and change system settings when you ask.
- Every dev toolchain, any version, without installing anything permanently.
- Updates that can't brick it. Two system slots; a bad one rolls back on its own.
- Hourly snapshots you can scrub back through.
- Ghost mode: boot without your data at all, for machines you don't trust.
- It's still a flash drive. There's a normal partition Windows and macOS can see.

## How it works

The system is a signed, read-only image. Your data lives on a separate encrypted volume. On boot, Eclipse fingerprints the host, loads what it knows about it, and starts a Wayland desktop that adapts to whatever screen it finds. The AI runs from weights on the stick.

The parts are named after an eclipse:

| | |
|---|---|
| Totality | the system image and the boot sequence. The boot screen is an eclipse, and the desktop appears at totality. |
| Umbra | the compositor |
| Corona | the shell: one bar for apps, commands and plain English |
| Aura | the local AI |
| Syzygy | host detection and memory |
| Penumbra | app sandboxing |
| Vault | snapshots, backups, cloning |

## Hardware

Any 64-bit PC with UEFI and a USB 3 port. 4 GB of RAM minimum, 8 GB or more recommended. Intel Macs work. Apple Silicon Macs don't and won't.

Use a fast stick. Cheap flash boots but crawls and dies. SSD-class sticks or an NVMe drive in a USB enclosure are what it's built for. 64 GB minimum, 256 GB recommended.

## Status

Early. It boots to a console today. The desktop, the shell and Aura land over the coming months. Releases will show up here when there's something worth flashing.

## Building it

`nix build .#image` builds the whole thing and needs an x86-64 Linux builder. `cargo build` builds the tools. `just` lists the rest.

## License

GPL-3.0-or-later. `libeclipse` is Apache-2.0 so you can embed it.

Copyright 2026 Thomas Guthrie.
