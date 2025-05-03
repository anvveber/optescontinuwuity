{ inputs, ... }:
{
  continuwuity =
    {
      config,
      pkgs,
      lib,
      ...
    }:
    let
      cfg = config.services.continuwuity;
    in
    {

      options.services.continuwuity = {
        enable = lib.mkEnableOption "continuwuity";
        nginx.enable = lib.mkEnableOption "nginx configuration for continuwuity";

        package = lib.mkOption {
          description = "The package containing the continuwuity binary";
          type = lib.types.package;
          default = inputs.self.packages.${pkgs.system}.all-features;
        };

        port = lib.mkOption {
          description = ''The local port of the continuwuity instance'';
          type = lib.types.int;
          example = 3000;
        };
        address = lib.mkOption {
          description = ''The local address of the continuwuity instance'';
          type = lib.types.str;
          example = "::1";
        };

        domain = lib.mkOption {
          description = ''The base domain of the continuwuity instance'';
          type = lib.types.str;
          example = "example.com";
        };
        subdomain = lib.mkOption {
          description = ''An optional subdomain of the continuwuity instance'';
          type = lib.types.nullOr lib.types.str;
          example = "example.com";
          default = null;
        };

        user = lib.mkOption {
          description = "The user that is running the continuwuity server";
          type = lib.types.str;
          default = "continuwuity";
          example = "continuwuity";
        };

        group = lib.mkOption {
          description = "The group of the user that is running the continuwuity server";
          type = lib.types.str;
          default = "continuwuity";
          example = "continuwuity";
        };

        dataDir = lib.mkOption {
          description = "The dataDir of the continuwuity server";
          type = lib.types.str;
          default = "/var/lib/continuwuity";
          example = "/var/lib/continuwuity";
        };

        settings = lib.mkOption {
          description = ''
            The continuwuity.toml config in nix format as an attrset. This
            gets directly translated into a toml file.
          '';
          type = lib.types.submodule (import ./config.nix { inherit config lib; });
          default = { };
        };
      };

      config =
        let
          # optional prepend the subdomain
          fullDomain = builtins.concatStringsSep "." (
            lib.flatten [
              # only use subdomain if not null
              (lib.optional (cfg.subdomain != null) cfg.subdomain)
              # always use the domain
              [ cfg.domain ]
            ]
          );
          baseUrl = "https://${fullDomain}";
        in
        lib.mkIf cfg.enable {

          users.users.${cfg.user} = {
            inherit (cfg) group;
            isSystemUser = true;
            home = cfg.dataDir;
            createHome = true;
            shell = "${lib.getExe pkgs.bash}";
          };

          users.groups.${cfg.group} = { };

          systemd.services.continuwuity =
            let
              # these options shouldn't be set-able by endusers via the config but are derived from the other nix options
              defaultConfigOptions = {
                server_name = cfg.domain;
                inherit (cfg) address port;
                database_path = cfg.dataDir;

                well_known = {
                  client = baseUrl;
                  server = "${fullDomain}:443";
                };
              };
              mergedConfig =
                builtins.foldl' lib.recursiveUpdate (builtins.removeAttrs cfg.settings [ "extraConfig" ])
                  [
                    defaultConfigOptions
                    cfg.settings.extraConfig
                  ];
              configFile = (pkgs.formats.toml { }).generate "continuwuity.toml" { global = mergedConfig; };
            in
            {
              description = "The continuwuity matrix server";

              documentation = [ "https://forgejo.ellis.link/continuwuation/continuwuity" ];

              wantedBy = [ "multi-user.target" ];
              wants = [ "network-online.target" ];
              after = [ "network-online.target" ];

              aliases = [ "matrix-continuwuity.service" ];

              environment = lib.mkMerge [
                # This has not yet been renamed
                { CONDUWUIT_CONFIG = configFile; }
                # include more stuff here ... if needed
              ];
              # this is derived from
              #
              # https://forgejo.ellis.link/continuwuation/continuwuity/src/commit/4158c1cf623a83b96d6a2d3cabb9f6aa1d618b4b/debian/conduwuit.service
              #
              # and
              #
              # https://github.com/NixOS/nixpkgs/blob/bf3287dac860542719fe7554e21e686108716879/nixos/modules/services/matrix/conduit.nix
              #
              # and
              #
              # https://github.com/NixOS/nixpkgs/blob/bf3287dac860542719fe7554e21e686108716879/nixos/modules/services/matrix/synapse.nix
              serviceConfig = {
                # maybe something more simple makes more sense here
                Type = "notify";

                DynamicUser = true;
                User = cfg.user;
                Group = cfg.group;

                WorkingDirectoy = cfg.dataDir;
                RuntimeDirectory = "continuwuity";
                RuntimeDirectoryPreserve = true;

                ExecStart = "${lib.getExe cfg.package}";
                UMask = "0077";

                DevicePolicy = "closed";
                LockPersonality = true;
                MemoryDenyWriteExecute = true;
                NoNewPrivileges = true;

                ProtectClock = true;
                ProtectControlGroups = true;
                ProtectHome = true;
                ProtectHostname = true;
                ProtectKernelLogs = true;
                ProtectKernelModules = true;
                ProtectKernelTunables = true;
                ProtectProc = "invisible";
                ProtectSystem = "strict";

                PrivateDevices = true;
                PrivateMounts = true;
                PrivateTmp = true;
                PrivateUsers = true;
                PrivateIPC = true;
                RemoveIPC = true;

                RestrictAddressFamilies = [
                  "AF_INET"
                  "AF_INET6"
                  "AF_UNIX"
                ];

                ReadWritePaths = [ cfg.dataDir ];

                RestrictNamespaces = true;
                RestrictRealtime = true;
                RestrictSUIDSGID = true;

                SystemCallArchitectures = "native";
                SystemCallFilter = [
                  "@system-service @resources"
                  "~@clock @debug @module @mount @reboot @swap @cpu-emulation @obsolete @timer @chown @setuid @privileged @keyring @ipc"
                ];
                SystemCallErrorNumber = "EPERM";

                Restart = "on-failure";
                RestartSec = 10;

                TimeoutStopSec = "2m";
                TimeoutStartSec = "2m";

                StartLimitBurst = 5;
              };
            };

          services.nginx =
            let
              clientConfig = {
                "m.homeserver".base_url = baseUrl;
              };
              serverConfig."m.server" = "${fullDomain}:443";
              mkWellKnown = data: ''
                add_header Content-Type application/json;
                add_header Access-Control-Allow-Origin *;
                return 200 '${builtins.toJSON data}';
              '';
            in
            lib.mkIf cfg.nginx.enable {
              enable = true;
              recommendedTlsSettings = true;
              recommendedOptimisation = true;
              recommendedGzipSettings = true;
              recommendedProxySettings = true;
              virtualHosts = {
                "${cfg.domain}" = {
                  enableACME = true;
                  forceSSL = true;
                  locations = {
                    "= /.well-known/matrix/server".extraConfig = mkWellKnown serverConfig;
                    "= /.well-known/matrix/client".extraConfig = mkWellKnown clientConfig;
                  };
                };
                "${fullDomain}" = {
                  enableACME = true;
                  forceSSL = true;
                  locations = {
                    "/" =
                      let
                        # really naive predicate to differentiate between v4 and v6
                        isIp6 = builtins.match cfg.address ".*:.*";
                        # make sure to "guard" the ip6 addrs by surrounding them with braces
                        addr = "${lib.optionalString isIp6 "["}${cfg.address}${lib.optionalString isIp6 "]"}";
                      in
                      {
                        proxyPass = "http://${addr}:${builtins.toString cfg.port}";
                        proxyWebsockets = true;
                      };
                  };
                };
              };
            };
        };
    };
}
