# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Boots a NixOS VM with services.maxd.host set from the example spec, then
# checks the units, the egress table, /etc/maxhq/host.json and that
# `maxd host check` finds nothing to fix. The VM has no network, so the
# spec here names no user tools, and its pool is a plain folder.
{ lib, ... }:

let
  example = lib.importTOML ../maxhq-host.example.toml;
  spec = example // {
    incus = example.incus // {
      pool = {
        name = "agents";
        driver = "dir";
      };
    };
    tools = {
      user = [ ];
      system = [ "git" ];
    };
  };
in
{
  name = "maxd-host";

  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ ../module.nix ];

      virtualisation.memorySize = 2048;
      virtualisation.diskSize = 4096;

      # Incus on NixOS runs on nftables.
      networking.nftables.enable = true;

      users.users.dev = {
        isNormalUser = true;
        uid = 1000;
      };

      environment.systemPackages = [ pkgs.git ];

      services.maxd = {
        enable = true;
        host = spec;
        incusAdmin = true;
        declarePool = true;
        declareBridge = true;
      };
    };

  testScript =
    { nodes, ... }:
    let
      maxd = lib.getExe nodes.machine.services.maxd.package;
    in
    ''
      import json

      machine.wait_for_unit("multi-user.target")
      for unit in [
          "maxd-prepare.service",
          "maxd-egress.service",
          "maxd-shutdown.service",
          "maxd-egress-check.timer",
          "incus.service",
          "incus-preseed.service",
          "user@1000.service",
      ]:
          machine.wait_for_unit(unit)

      with subtest("the prepare unit runs before Incus and sets the limits"):
          machine.succeed("systemctl show -p Before maxd-prepare.service | grep -q incus.service")
          machine.succeed("test -d /sys/fs/cgroup/maxhq-workspaces.monitor")
          machine.succeed("grep -qx '800000 100000' /sys/fs/cgroup/maxhq-workspaces/cpu.max")
          machine.succeed("grep -qx 50 /sys/fs/cgroup/maxhq-workspaces/cpu.weight")
          machine.succeed("grep -qx 12884901888 /sys/fs/cgroup/maxhq-workspaces/memory.high")
          machine.succeed("grep -qx 4294967296 /sys/fs/cgroup/system.slice/memory.low")

      with subtest("the shutdown unit waits on Incus and the user, and quiesces"):
          after = machine.succeed("systemctl show -p After maxd-shutdown.service")
          assert "incus.service" in after and "user@1000.service" in after, after
          stop = machine.succeed("systemctl show -p ExecStop maxd-shutdown.service")
          assert "host quiesce --window 20m" in stop, stop
          machine.succeed("systemctl show -p User maxd-shutdown.service | grep -qx User=dev")

      with subtest("the egress table is loaded for the bridge"):
          rules = machine.succeed("nft list table inet maxhq-egress")
          assert 'iifname "agentbr0" drop' in rules, rules
          machine.succeed("systemctl start maxd-egress-check.service")

      with subtest("zram, linger, the data folder and nix-ld"):
          machine.wait_until_succeeds("grep -q '^/dev/zram' /proc/swaps")
          machine.succeed("test -e /var/lib/systemd/linger/dev")
          machine.succeed("test \"$(stat -c %U /srv/maxhq)\" = dev")
          machine.succeed("test -e /run/current-system/sw/share/nix-ld/lib/ld.so")

      with subtest("host.json is the spec as maxd reads it"):
          have = json.loads(machine.succeed("cat /etc/maxhq/host.json"))
          want = json.loads(r"""${builtins.toJSON spec}""")
          assert have == want, have

      with subtest("maxd host check finds nothing to fix"):
          status, out = machine.execute("su - dev -c '${maxd} host check'")
          print(out)
          assert status == 0, "maxd host check found items to fix"
          assert "from the NixOS module" in out, out
    '';
}
