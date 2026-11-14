# SPDX-License-Identifier: MIT
# Copyright (c) 2026 Max Boksem

# Runs `maxd run` as a systemd user service, so the agents on this machine
# and in its Incus workspaces show up in MaxHQ. On other Linux machines
# `maxd service install` writes a user unit. On NixOS this module's unit
# takes its place, and `maxd service install` leaves it alone. The unit
# runs the Nix-built maxd unless `services.maxd.binary` points at another,
# such as the one MaxHQ deploys to ~/.local/bin/maxd. A user service gets
# the same XDG_RUNTIME_DIR as the user's SSH sessions, so `maxd proxy`
# finds the daemon's socket there.
#
# With `services.maxd.host` set from a maxhq-host.toml, it also sets up
# what `maxd host apply --system` writes on Debian and Ubuntu. Whatever is
# computed from the spec (the egress rules, the prepare script and its
# cgroup limits, MemoryLow) comes from `maxd host render` at build time,
# so this file only wires units and options.
{ config, lib, pkgs, ... }:

let
  cfg = config.services.maxd;
  # The package's maxd, which renders the spec at build time.
  maxd = lib.getExe cfg.package;
  inIncusAdmin =
    lib.elem "incus-admin" config.users.users.${cfg.user}.extraGroups
    || lib.elem cfg.user (config.users.groups.incus-admin.members or [ ]);

  # The spec, read the way maxd reads it. The defaults here are the ones
  # in crates/core/src/host_spec.rs. maxd host render refuses an item the
  # spec doesn't have, so the build fails if this side wires one that
  # maxd wouldn't.
  spec = if cfg.host == null then { } else cfg.host;
  get = path: default: lib.attrByPath path default spec;
  hasHost = cfg.host != null;
  incusOn = get [ "incus" "enable" ] true;
  pool = get [ "incus" "pool" ] null;
  bridge = get [ "incus" "bridge" ] null;
  manageParent = get [ "caps" "parent" ] null != null && get [ "caps" "manage_parent" ] false;
  egressOn = incusOn && bridge != null;
  memoryLowOn = get [ "caps" "system_memory_low" ] null != null;
  dataDir = get [ "service" "data_dir" ] null;
  specUser = get [ "service" "user" ] null;
  window = get [ "shutdown" "quiesce_window" ] "20m";

  uid = config.users.users.${cfg.user}.uid;
  group = config.users.users.${cfg.user}.group;

  # The files maxd computes from the spec, checked and rendered once.
  specFile = pkgs.writeText "maxhq-host.json" (builtins.toJSON spec);
  rendered = pkgs.runCommand "maxhq-host" { } (
    ''
      mkdir -p $out/lib/systemd/system/system.slice.d
      ${maxd} host render json --spec ${specFile} > $out/host.json
    ''
    + lib.optionalString manageParent ''
      ${maxd} host render prepare --spec ${specFile} > $out/prepare.sh
    ''
    + lib.optionalString egressOn ''
      ${maxd} host render egress --spec ${specFile} > $out/egress.nft
    ''
    + lib.optionalString memoryLowOn ''
      ${maxd} host render memory-low --spec ${specFile} \
        > $out/lib/systemd/system/system.slice.d/50-maxhq-memory-low.conf
    ''
  );
  nft = "${pkgs.nftables}/bin/nft";
in
{
  options.services.maxd = {
    enable = lib.mkEnableOption "maxd, the MaxHQ daemon";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
      description = "The maxd package to run.";
    };

    binary = lib.mkOption {
      type = lib.types.str;
      default = maxd;
      defaultText = lib.literalExpression "lib.getExe config.services.maxd.package";
      example = "/home/alice/.local/bin/maxd";
      description = ''
        The maxd the user service and maxd-shutdown.service run. Set it to
        the full path of the user's ~/.local/bin/maxd to run the maxd MaxHQ
        deploys and updates there, so an update from the app takes effect
        without a rebuild. systemd doesn't expand ~, so spell out the home
        folder. The package still builds either way, since maxd host render
        runs it at build time.
      '';
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = lib.attrByPath [ "service" "user" ] "max" spec;
      defaultText = lib.literalExpression ''services.maxd.host.service.user or "max"'';
      description = ''
        The user maxd runs as. It watches that user's agent sessions, and
        it needs the incus-admin group to watch Incus workspaces.
      '';
    };

    host = lib.mkOption {
      type = lib.types.nullOr (pkgs.formats.json { }).type;
      default = null;
      example = lib.literalExpression "lib.importTOML ./maxhq-host.toml";
      description = ''
        The host spec, read from maxhq-host.toml with lib.importTOML. When
        set, the module turns Incus on, keeps the user lingering, adds the
        prepare, shutdown and egress units, zram, MemoryLow, the data
        folder and nix-ld, and puts the spec at /etc/maxhq/host.json, where
        maxd host check reads it. Keys merge, so single keys can be set
        beside the file.
      '';
    };

    incusAdmin = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Whether to add the user to incus-admin, which maxd needs to make
        and watch Incus workspaces. Off unless set, since anyone in the
        group can run anything as root through Incus.
      '';
    };

    declarePool = lib.mkEnableOption (
      "the spec's Incus storage pool, through virtualisation.incus.preseed. "
      + "Leave it off where the host's configuration declares the pool already"
    );

    declareBridge = lib.mkEnableOption (
      "the spec's Incus bridge, through virtualisation.incus.preseed. "
      + "Leave it off where the host's configuration declares the bridge already"
    );
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      # The group grants root through Incus, so it's only added when
      # incusAdmin says so.
      warnings = lib.optional (config.virtualisation.incus.enable && !inIncusAdmin)
        "services.maxd: ${cfg.user} isn't in incus-admin, so maxd can't see the Incus workspaces. Set services.maxd.incusAdmin = true to add it.";

      users.users.${cfg.user} = {
        # Lingering starts the user's service manager at boot, so maxd runs
        # with nobody logged in and comes back after a reboot.
        linger = true;
        extraGroups = lib.mkIf cfg.incusAdmin [ "incus-admin" ];
      };

      systemd.user.services.maxd = {
        description = "MaxHQ daemon";
        wantedBy = [ "default.target" ];
        unitConfig.ConditionUser = cfg.user;
        # Agent CLIs installed for the user or the system, such as codex,
        # are found the way a login shell finds them.
        path = [
          "/etc/profiles/per-user/${cfg.user}"
          "/run/current-system/sw"
        ];
        serviceConfig = {
          ExecStart = "${cfg.binary} run";
          Restart = "always";
          RestartSec = 2;
          NoNewPrivileges = true;
        };
      };
    }

    (lib.mkIf hasHost {
      assertions = [
        {
          assertion = specUser == null || specUser == cfg.user;
          message = "services.maxd: the spec says ${toString specUser} runs maxd, and services.maxd.user is ${cfg.user}.";
        }
        {
          assertion = uid != null;
          message = "services.maxd.host needs users.users.${cfg.user}.uid set, since maxd-shutdown.service waits on that user's service manager.";
        }
      ];

      environment.etc."maxhq/host.json".source = "${rendered}/host.json";

      virtualisation.incus.enable = lib.mkIf incusOn true;

      # The tools MaxHQ installs for the user are built for other Linux, so
      # they need a loader at the usual path.
      programs.nix-ld.enable = lib.mkDefault true;

      # Half the memory, up to 8 GiB, as maxd host apply --system sets it.
      zramSwap = lib.mkIf (get [ "caps" "zram" ] false) {
        enable = true;
        algorithm = "zstd";
        memoryPercent = 50;
        memoryMax = 8 * 1024 * 1024 * 1024;
      };

      # The system.slice drop-in that holds MemoryLow.
      systemd.packages = lib.optional memoryLowOn rendered;

      # optionalAttrs, not mkIf, since mkIf reads the folder's name even
      # when there's none.
      systemd.tmpfiles.settings = lib.optionalAttrs (dataDir != null) {
        "10-maxd".${dataDir}.d = {
          user = cfg.user;
          group = if group == "" then "users" else group;
          mode = "0700";
        };
      };

      # Stops the workspaces cleanly at shutdown, while Incus and the
      # user's maxd still run. It's ordered after both, so systemd stops it
      # first. It never restarts, so a rebuild never runs its stop. The
      # stop runs as root, with `+`, since maxd winds its agents down only
      # for root or a proven app; it finds the user's socket by the
      # runtime folder.
      systemd.services.maxd-shutdown = {
        description = "Stop MaxHQ's workspaces cleanly at shutdown";
        after = [ "incus.service" "user@${toString uid}.service" ];
        wants = [ "user@${toString uid}.service" ];
        wantedBy = [ "multi-user.target" ];
        restartIfChanged = false;
        environment.XDG_RUNTIME_DIR = "/run/user/${toString uid}";
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          User = cfg.user;
          ExecStart = "${pkgs.coreutils}/bin/true";
          ExecStop = "+${cfg.binary} host quiesce --window ${window}";
          # The window and a minute more. The spec's s, m, h and d mean
          # the same to systemd.
          TimeoutStopSec = "${window} 60s";
        };
      };

      # Makes the parent cgroup and sets its limits before Incus starts
      # any container in it.
      systemd.services.maxd-prepare = lib.mkIf manageParent {
        description = "The cgroup MaxHQ's workspaces share";
        before = [ "incus.service" ];
        wantedBy = [ "multi-user.target" "incus.service" ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStart = "${pkgs.runtimeShell} ${rendered}/prepare.sh";
        };
      };

      # Loads the inet maxhq-egress table before Incus starts. The rules
      # are a store path, so a changed spec restarts this and reloads them.
      systemd.services.maxd-egress = lib.mkIf egressOn {
        description = "Keep MaxHQ's workspaces from reaching anything private";
        after = [ "nftables.service" ];
        before = [ "incus.service" ];
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStart = "${nft} -f ${rendered}/egress.nft";
          ExecStop = "-${nft} delete table inet maxhq-egress";
        };
      };

      # Puts the table back if something flushed it.
      systemd.services.maxd-egress-check = lib.mkIf egressOn {
        description = "Check MaxHQ's egress table is loaded";
        serviceConfig.Type = "oneshot";
        script = "${nft} list table inet maxhq-egress > /dev/null || ${nft} -f ${rendered}/egress.nft";
      };

      systemd.timers.maxd-egress-check = lib.mkIf egressOn {
        description = "Check MaxHQ's egress table every five minutes";
        wantedBy = [ "timers.target" ];
        timerConfig = {
          OnBootSec = "1min";
          OnUnitActiveSec = "5min";
        };
      };

      # The pool and bridge, for a host that doesn't declare them itself.
      # maxd host apply makes them through Incus otherwise.
      virtualisation.incus.preseed = lib.mkMerge [
        (lib.mkIf (cfg.declarePool && pool != null) {
          storage_pools = [
            {
              inherit (pool) name;
              driver = pool.driver or "btrfs";
              config =
                lib.optionalAttrs (pool ? image) { source = pool.image; }
                // lib.optionalAttrs (pool ? size) { inherit (pool) size; };
            }
          ];
        })
        (lib.mkIf (cfg.declareBridge && bridge != null) {
          networks = [
            {
              inherit (bridge) name;
              type = "bridge";
              config = {
                "ipv4.nat" = "true";
                "ipv6.address" = "none";
              } // lib.optionalAttrs (bridge ? subnet) { "ipv4.address" = bridge.subnet; };
            }
          ];
        })
      ];
    })
  ]);
}
