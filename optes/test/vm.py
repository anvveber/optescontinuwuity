#!/usr/bin/env python3
"""vm.py — a throwaway Debian 13 VM for testing the setup scripts.

The setup scripts change the firewall, sysctls, journald and apt sources of
the machine they run on. Run them in this VM, never on your own machine.

Usage:
  vm.py create            download the Debian 13 "nocloud" image, check it,
                          prepare it (root SSH key, bigger disk) and keep it
                          as the clean copy. Run once; takes 5-30 minutes.
  vm.py start [--keep]    boot a fresh copy of the clean disk (--keep: boot
                          the last copy again, with its changes)
  vm.py stop              power off
  vm.py ssh [command...]  SSH in as root
  vm.py copy <file...>    copy files to /root in the VM
  vm.py wake              wake the VM from suspend (after
                          "vm.py ssh systemctl suspend"), like opening a
                          laptop's lid
  vm.py status

Options (before the command):
  --dir <dir>    state directory (default ~/.cache/optes-vm)
  --port <n>     host port for the VM's SSH (default 2222)
  --memory <MB>  default 2048
  --cpus <n>     default 2
  --name <name>  run a second VM next to the first, from the same clean
                 disk: own disk copy, console and pid (default "main").
                 Give it its own --port.

Needs: qemu-system-x86_64, qemu-img, ssh, ssh-keygen, scp, Python 3.11+.
Environment: OPTES_VM_HTTP_MIRRORS=1 makes the VM's apt use http:// mirrors
(for build sandboxes whose proxy passes only plain HTTP).
Uses KVM if /dev/kvm is usable, otherwise plain emulation (slow).
The VM reaches the network through QEMU's user-mode NAT, so the host's
network sees its traffic (apt, and Tor if a test starts it).
"""

import argparse
import hashlib
import os
import re
import secrets
import shutil
import socket
import subprocess
import sys
import time
import urllib.request

IMAGE_BASE = "https://cloud.debian.org/images/cloud/trixie"
DISK_SIZE = "10G"


def die(msg):
    sys.exit(f"ERROR: {msg}")


def info(msg):
    print(f"==> {msg}", flush=True)


class VM:
    def __init__(self, args):
        self.dir = os.path.abspath(os.path.expanduser(args.dir))
        self.port = args.port
        self.memory = args.memory
        self.cpus = args.cpus
        self.key = os.path.join(self.dir, "id_ed25519")
        self.clean = os.path.join(self.dir, "clean.qcow2")
        sfx = "" if args.name == "main" else "-" + args.name
        self.work = os.path.join(self.dir, f"work{sfx}.qcow2")
        self.serial = os.path.join(self.dir, f"serial{sfx}.sock")
        self.pidfile = os.path.join(self.dir, f"qemu{sfx}.pid")
        self.monitor = os.path.join(self.dir, f"monitor{sfx}.sock")
        self.log = os.path.join(self.dir, f"qemu{sfx}.log")

    # ---- process ---------------------------------------------------------
    def pid(self):
        try:
            pid = int(open(self.pidfile).read().strip())
            os.kill(pid, 0)
            return pid
        except (OSError, ValueError):
            return None

    def boot(self, disk):
        if self.pid():
            die("the VM is already running (vm.py stop)")
        for f in (self.serial, self.pidfile, self.monitor):
            if os.path.exists(f):
                os.remove(f)
        kvm = os.access("/dev/kvm", os.R_OK | os.W_OK)
        cmd = ["qemu-system-x86_64", "-machine", "q35" + (",accel=kvm" if kvm else ""),
               "-cpu", "host" if kvm else "max", "-smp", str(self.cpus), "-m", str(self.memory),
               "-drive", f"file={disk},if=virtio,format=qcow2",
               "-netdev", f"user,id=n0,hostfwd=tcp:127.0.0.1:{self.port}-:22",
               "-device", "virtio-net-pci,netdev=n0",
               "-display", "none", "-serial", f"unix:{self.serial},server=on,wait=off",
               "-monitor", f"unix:{self.monitor},server=on,wait=off",
               "-pidfile", self.pidfile, "-daemonize"]
        log = open(self.log, "w")
        subprocess.run(cmd, check=True, stdout=log, stderr=log)
        info(f"booting ({'KVM' if kvm else 'no KVM: slow'}), SSH on 127.0.0.1:{self.port}")

    def ssh_cmd(self, *extra):
        return ["ssh", "-i", self.key, "-p", str(self.port), "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR",
                "-o", "ConnectTimeout=20", *extra, "root@127.0.0.1"]

    def wait_ssh(self, timeout):
        end = time.time() + timeout
        while time.time() < end:
            if subprocess.run(self.ssh_cmd("-o", "BatchMode=yes") + ["true"],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
                return True
            time.sleep(5)
        return False

    def poweroff(self, timeout=180):
        pid = self.pid()
        if not pid:
            return
        subprocess.run(self.ssh_cmd() + ["systemctl poweroff"],
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        end = time.time() + timeout
        while time.time() < end and self.pid():
            time.sleep(2)
        if self.pid():
            os.kill(pid, 15)
            time.sleep(2)

    # ---- serial console (first boot only) --------------------------------
    def console(self):
        for _ in range(30):
            try:
                s = socket.socket(socket.AF_UNIX)
                s.connect(self.serial)
                s.settimeout(1)
                return s
            except OSError:
                time.sleep(1)
        die("no serial console")

    @staticmethod
    def strip_codes(raw):
        return re.sub(r"\x1b\[[0-9;?!]*[a-zA-Z]", "", raw.decode("utf-8", "replace")).replace("\r", "")

    def read_until(self, s, pattern, timeout, buf=b""):
        """Reads the console until `pattern` is found in the text read since
        the call began. Returns (cleaned text, raw bytes)."""
        end = time.time() + timeout
        rx = re.compile(pattern)
        while time.time() < end:
            try:
                data = s.recv(65536)
                if data:
                    buf += data
                    text = self.strip_codes(buf)
                    if rx.search(text):
                        return text, buf
            except socket.timeout:
                pass
        die(f"console: timed out waiting for {pattern!r}; last output:\n{self.strip_codes(buf)[-600:]}")

    def first_boot(self, password, pubkey):
        """Answers systemd-firstboot (empty answers, a root password), logs
        in on the console and prepares SSH and the disk."""
        s = self.console()
        info("first boot: answering the setup questions on the console")
        # Every firstboot question is a line "‣ ...: ". Scan all new output
        # (escape codes can be split across reads) and answer each once.
        buf, done, answered, quiet_since = b"", 0, 0, time.time()
        end = time.time() + 1800
        while time.time() < end:
            try:
                data = s.recv(65536)
            except socket.timeout:
                data = b""
            if data:
                buf += data
                quiet_since = time.time()
            text = self.strip_codes(buf)
            prompts = list(re.finditer(r"‣ ([^\n]*?): (\(press TAB[^)]*\) )?", text))
            for m in prompts[done:]:
                if "password" in m.group(1).lower():
                    s.sendall((password + "\n").encode())
                    answered += 1
                else:
                    s.sendall(b"\n")  # timezone, keymap, locale, ...: skip
                time.sleep(0.5)
            done = len(prompts)
            last_prompt = prompts[-1].end() if prompts else 0
            if re.search(r"login: ", text[last_prompt:]):
                break
            if time.time() - quiet_since > 120:  # output came before we connected
                s.sendall(b"\n")
                quiet_since = time.time()
        else:
            die("console: no login prompt after 30 minutes")
        if answered < 2:
            die(f"first boot asked for the root password {answered} time(s), expected 2:\n{text[-800:]}")
        s.sendall(b"root\n")
        self.read_until(s, r"Password: ", 120)
        s.sendall((password + "\n").encode())
        text, _ = self.read_until(s, r"(root@[^\n]*# |Login incorrect)", 120)
        if "Login incorrect" in text:
            die("console login as root failed: the password was not set")
        info("first boot: SSH key, openssh-server, disk size")
        # Some build sandboxes only pass plain HTTP to the VM.
        mirrors = ("sed -i 's#https://#http://#' /etc/apt/mirrors/*.list; "
                   if os.environ.get("OPTES_VM_HTTP_MIRRORS") == "1" else "")
        script = (
            "dmesg -n 1; " + mirrors + "mkdir -p /root/.ssh && chmod 700 /root/.ssh && "
            f"echo '{pubkey}' > /root/.ssh/authorized_keys && chmod 600 /root/.ssh/authorized_keys && "
            "export DEBIAN_FRONTEND=noninteractive && apt-get update -q >/dev/null && "
            "apt-get install -y -q openssh-server fdisk >/dev/null && "
            "echo ', +' | sfdisk -N 1 /dev/vda --force --no-reread >/dev/null 2>&1; "
            "partx -u /dev/vda; resize2fs /dev/vda1 >/dev/null 2>&1; "
            "systemctl enable --now ssh >/dev/null 2>&1; echo FIRSTBOOT-DONE-$?\n"
        )
        s.sendall(script.encode())
        text, _ = self.read_until(s, r"FIRSTBOOT-DONE-\d+\s", 1800)
        if "FIRSTBOOT-DONE-0" not in text[-200:]:
            die(f"first boot setup failed:\n{text[-800:]}")
        s.close()

    # ---- commands --------------------------------------------------------
    def create(self):
        for tool in ("qemu-system-x86_64", "qemu-img", "ssh", "ssh-keygen", "scp"):
            if not shutil.which(tool):
                die(f"missing tool: {tool}")
        if os.path.exists(self.clean):
            die(f"{self.clean} exists already; delete it to make a new one")
        os.makedirs(self.dir, mode=0o700, exist_ok=True)

        info("find the newest Debian 13 nocloud image")
        index = urllib.request.urlopen(IMAGE_BASE + "/", timeout=60).read().decode()
        dated = sorted(set(re.findall(r'href="(\d{8}-\d+)/"', index)), reverse=True)
        name = sums = None
        for d in dated[:6]:
            try:
                text = urllib.request.urlopen(f"{IMAGE_BASE}/{d}/SHA512SUMS", timeout=60).read().decode()
            except OSError:
                continue
            m = re.search(r"^([0-9a-f]{128})\s+(debian-13-nocloud-amd64-\S+\.qcow2)$", text, re.M)
            if m:
                sums, name, folder = m.group(1), m.group(2), d
                break
        if not name:
            die("no nocloud amd64 image found")
        image = os.path.join(self.dir, name)
        if not os.path.exists(image):
            info(f"download {name}")
            tmp = image + ".part"
            urllib.request.urlretrieve(f"{IMAGE_BASE}/{folder}/{name}", tmp)
            os.rename(tmp, image)
        h = hashlib.sha512()
        with open(image, "rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
        if h.hexdigest() != sums:
            os.remove(image)
            die("SHA512 of the image does not match Debian's SHA512SUMS; deleted it")
        info("image checksum OK (SHA512SUMS fetched over HTTPS)")

        if not os.path.exists(self.key):
            subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-C", "optes-vm",
                            "-f", self.key], check=True)
        pubkey = open(self.key + ".pub").read().strip()
        password = secrets.token_urlsafe(18)
        with open(os.path.join(self.dir, "root-password"), "w", opener=lambda p, f: os.open(p, f, 0o600)) as f:
            f.write(password + "\n")

        prep = os.path.join(self.dir, "prepare.qcow2")
        shutil.copyfile(image, prep)
        subprocess.run(["qemu-img", "resize", "-q", prep, DISK_SIZE], check=True)
        self.boot(prep)
        self.first_boot(password, pubkey)
        if not self.wait_ssh(300):
            die("SSH does not answer after first boot")
        info("SSH works; power off and keep the clean copy")
        self.poweroff()
        os.rename(prep, self.clean)
        info(f"done: {self.clean}. Next: vm.py start")

    def start(self, keep):
        if not os.path.exists(self.clean):
            die("no clean disk yet: run vm.py create")
        if not keep or not os.path.exists(self.work):
            shutil.copyfile(self.clean, self.work)
        self.boot(self.work)
        if not self.wait_ssh(900):
            die("SSH does not answer; see " + self.log)
        info("up: vm.py ssh")

    def wake(self):
        if not self.pid():
            die("the VM is not running")
        s = socket.socket(socket.AF_UNIX)
        s.connect(self.monitor)
        s.settimeout(5)
        time.sleep(0.5)
        s.recv(4096)  # banner
        s.sendall(b"system_wakeup\n")
        time.sleep(0.5)
        s.close()
        if not self.wait_ssh(120):
            die("SSH does not answer after wakeup")
        info("awake")

    def status(self):
        print(f"state dir:  {self.dir}")
        print(f"clean disk: {'yes' if os.path.exists(self.clean) else 'no (vm.py create)'}")
        print(f"running:    {'yes, pid %d' % self.pid() if self.pid() else 'no'}")
        print(f"kvm:        {'yes' if os.access('/dev/kvm', os.R_OK | os.W_OK) else 'no'}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--dir", default="~/.cache/optes-vm")
    ap.add_argument("--port", type=int, default=2222)
    ap.add_argument("--memory", type=int, default=2048)
    ap.add_argument("--cpus", type=int, default=2)
    ap.add_argument("--name", default="main")
    ap.add_argument("command", choices=["create", "start", "stop", "ssh", "copy", "wake", "status"])
    ap.add_argument("rest", nargs=argparse.REMAINDER)
    args = ap.parse_args()
    if not re.fullmatch(r"[a-z0-9-]{1,20}", args.name):
        die("--name: 1-20 chars of a-z 0-9 -")
    vm = VM(args)
    if args.command == "create":
        vm.create()
    elif args.command == "start":
        vm.start("--keep" in args.rest)
    elif args.command == "stop":
        vm.poweroff()
        info("stopped")
    elif args.command == "ssh":
        os.execvp("ssh", vm.ssh_cmd() + args.rest)
    elif args.command == "copy":
        if not args.rest:
            die("copy needs files")
        cmd = ["scp", "-q", "-i", vm.key, "-P", str(vm.port), "-o", "StrictHostKeyChecking=no",
               "-o", "UserKnownHostsFile=/dev/null", "-o", "LogLevel=ERROR", *args.rest,
               "root@127.0.0.1:/root/"]
        sys.exit(subprocess.run(cmd).returncode)
    elif args.command == "wake":
        vm.wake()
    else:
        vm.status()


if __name__ == "__main__":
    main()
